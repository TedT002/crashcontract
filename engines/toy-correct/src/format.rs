//! Disk üzeri bayt formatları: WAL kaydı ve checkpoint. Tüm tamsayılar little-endian.
//!
//! ## WAL kaydı
//! `lsn u64 | len u32 | crc u32 | payload[len]`, `crc = crc32(lsn ‖ len ‖ payload)`.
//! Payload: `count u32`, sonra her yazma: `tag u8` (1=Put, 2=Delete), `key_len u32`, `key`,
//! Put ise `val_len u32`, `val`. Bir işlem = tam bir kayıt: Txn atomikliği buradan gelir — CRC
//! ya bütün kaydı doğrular ya hiçbirini.
//!
//! ## Neden LSN, neden nesil başına yeni WAL dosyası?
//! Replay "ilk bozuk kayıtta dur" kuralıyla çalışır. Bu kural ancak yarım kalmış kuyruğun
//! **ardında eski ama CRC'si geçerli kayıt yoksa** doğrudur. Aynı WAL'a kurtarmadan sonra ekleme
//! yapılsaydı (ya da WAL `ftruncate` ile yeniden kullanılsaydı ve kısaltma kalıcılaşmasaydı), yeni
//! kısa bir kayıt eski bir kaydın tam önüne hizalanıp onu "diriltebilirdi". Birincil çözüm: her
//! checkpoint yeni nesil bir WAL dosyası açar, kurtarılan WAL'a asla ekleme yapılmaz — taze dosyada
//! her ofset ya bu nesilde bir kez yazılmıştır ya hiç yazılmamıştır. İkinci savunma: dosya içi
//! ardışık LSN (1'den). Sıfırlarla dolu bir başlık (delik) `lsn = 0` taşır ve asla geçerli değildir.

use cc_adapter::AdapterError;
use cc_model::{State, Write};

pub use crate::crc32::crc32;
use crate::crc32::crc32_parts;

/// Güncel checkpoint dosyasının adı.
pub const CHECKPOINT_FILE: &str = "CHECKPOINT";
/// Checkpoint yazılırken kullanılan geçici ad (fsync + rename ile atomik değiştirme).
pub const CHECKPOINT_TMP_FILE: &str = "CHECKPOINT.tmp";
/// Checkpoint dosyasının sihirli baytları.
pub const CHECKPOINT_MAGIC: [u8; 8] = *b"CCTOYCP1";
/// WAL kayıt başlığının uzunluğu (lsn + len + crc).
pub const WAL_HEADER_LEN: usize = 16;
/// Bir kaydın en büyük payload uzunluğu; daha büyük `len` bozuk başlık sayılır.
pub const MAX_RECORD_PAYLOAD: u32 = 1 << 24;

const TAG_PUT: u8 = 1;
const TAG_DELETE: u8 = 2;
/// magic + generation + count + crc.
const CHECKPOINT_MIN_LEN: usize = 8 + 8 + 8 + 4;

/// `generation` nesilli WAL dosyasının adı: `wal-` + 16 küçük hex hane.
pub fn wal_file_name(generation: u64) -> String {
    format!("wal-{generation:016x}")
}

/// `wal-` + tam 16 küçük hex hane ise nesli döndürür; aksi None.
pub fn parse_wal_file_name(name: &str) -> Option<u64> {
    let hex = name.strip_prefix("wal-")?;
    let well_formed =
        hex.len() == 16 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !well_formed {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_bytes(out: &mut Vec<u8>, b: &[u8]) {
    let len = u32::try_from(b.len()).expect("değişmez: anahtar/değer 4 GiB'den küçük");
    push_u32(out, len);
    out.extend_from_slice(b);
}

fn encode_payload(writes: &[Write]) -> Vec<u8> {
    let mut p = Vec::new();
    push_u32(
        &mut p,
        u32::try_from(writes.len()).expect("değişmez: yazma sayısı u32'ye sığar"),
    );
    for w in writes {
        match w {
            Write::Put { key, value } => {
                p.push(TAG_PUT);
                push_bytes(&mut p, key);
                push_bytes(&mut p, value);
            }
            Write::Delete { key } => {
                p.push(TAG_DELETE);
                push_bytes(&mut p, key);
            }
        }
    }
    p
}

/// Tek bir WAL kaydını (başlık + payload) tek tamponda kodlar.
///
/// Tek tampon önemli: motor kaydı tek `write_at` ile yazar; çökme üreteci tek bir yazıyı yırtabilir
/// (torn write) ama başlık ile payload'ı ayrı olaylar olarak sıralayamaz.
pub fn encode_wal_record(lsn: u64, writes: &[Write]) -> Vec<u8> {
    let payload = encode_payload(writes);
    // Payload > u32::MAX ise kayıt zaten MAX_RECORD_PAYLOAD'u aşar; motor bunu G/Ç'den önce
    // reddeder. Burada doyurarak kodlarız ki uzunluk kontrolü motorda yapılabilsin.
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let lsn_b = lsn.to_le_bytes();
    let len_b = len.to_le_bytes();
    let crc = crc32_parts(&[&lsn_b, &len_b, &payload]);
    let mut out = Vec::with_capacity(WAL_HEADER_LEN + payload.len());
    out.extend_from_slice(&lsn_b);
    out.extend_from_slice(&len_b);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

/// WAL çözümünde durma nedeni.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalStop {
    /// Dosya tam bir kayıt sınırında bitti.
    CleanEnd,
    /// Kalan bayt sayısı bir başlığa yetmiyor (yırtık başlık).
    TornHeader,
    /// Beklenmeyen LSN (eski kayıt, atlama ya da sıfır dolu delik).
    LsnMismatch {
        /// Beklenen LSN.
        expected: u64,
        /// Bulunan LSN.
        found: u64,
    },
    /// Başlıktaki uzunluk sınırı aşıyor.
    TooLong {
        /// Başlıktaki uzunluk.
        len: u32,
    },
    /// Payload dosya sonunda yarım kalmış.
    Truncated,
    /// CRC uyuşmuyor (yırtık ya da bozuk kayıt).
    BadCrc,
}

/// WAL çözüm sonucu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalReplay {
    /// Geçerli kayıtlar, sırayla; her biri bir işlemin yazmaları.
    pub records: Vec<Vec<Write>>,
    /// Son geçerli kaydın bittiği ofset.
    pub valid_len: u64,
    /// Çözüm neden durdu.
    pub stop: WalStop,
}

/// Bayt okuyucu: taşma yerine `None` döner (bozuk veri panic ettirmemeli).
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bayt")))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|b| u64::from_le_bytes(b.try_into().expect("8 bayt")))
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let len = self.u32()? as usize;
        self.take(len).map(<[u8]>::to_vec)
    }
    fn at_end(&self) -> bool {
        self.pos == self.buf.len()
    }
}

fn decode_payload(payload: &[u8]) -> Option<Vec<Write>> {
    let mut r = Reader {
        buf: payload,
        pos: 0,
    };
    let count = r.u32()?;
    let mut writes = Vec::new();
    for _ in 0..count {
        let w = match r.u8()? {
            TAG_PUT => Write::Put {
                key: r.bytes()?,
                value: r.bytes()?,
            },
            TAG_DELETE => Write::Delete { key: r.bytes()? },
            _ => return None,
        };
        writes.push(w);
    }
    r.at_end().then_some(writes)
}

/// WAL'ı baştan çözer; **ilk bozuk kayıtta durur** (sıralı replay → prefix tutarlılığı).
///
/// CRC'si geçen ama payload'ı çözülemeyen kayıt `Err(Corruption)` döner: bu yırtık yazma değil,
/// motorun kendi kodlama hatasıdır ve gizlenmemelidir.
pub fn decode_wal(bytes: &[u8]) -> Result<WalReplay, AdapterError> {
    decode_wal_checked(bytes, true)
}

/// [`decode_wal`]; `verify_crc = false` yalnız M6 mutantı içindir.
pub(crate) fn decode_wal_checked(
    bytes: &[u8],
    verify_crc: bool,
) -> Result<WalReplay, AdapterError> {
    let mut records = Vec::new();
    let mut off = 0usize;
    let mut expected = 1u64;
    let stop = loop {
        let rest = &bytes[off..];
        if rest.is_empty() {
            break WalStop::CleanEnd;
        }
        if rest.len() < WAL_HEADER_LEN {
            break WalStop::TornHeader;
        }
        let lsn = u64::from_le_bytes(rest[0..8].try_into().expect("8 bayt"));
        let len = u32::from_le_bytes(rest[8..12].try_into().expect("4 bayt"));
        let crc = u32::from_le_bytes(rest[12..16].try_into().expect("4 bayt"));
        if lsn != expected {
            break WalStop::LsnMismatch {
                expected,
                found: lsn,
            };
        }
        if len > MAX_RECORD_PAYLOAD {
            break WalStop::TooLong { len };
        }
        let total = WAL_HEADER_LEN + len as usize;
        if rest.len() < total {
            break WalStop::Truncated;
        }
        let payload = &rest[WAL_HEADER_LEN..total];
        // MUTASYON NOKTASI M6: CRC denetimi kapatılırsa yırtık kayıt geçerli sayılır.
        if verify_crc && crc32_parts(&[&rest[0..8], &rest[8..12], payload]) != crc {
            break WalStop::BadCrc;
        }
        let writes = decode_payload(payload).ok_or_else(|| {
            AdapterError::Corruption(format!("LSN {lsn}: CRC geçerli ama payload çözülemiyor"))
        })?;
        records.push(writes);
        expected += 1;
        off += total;
    };
    Ok(WalReplay {
        records,
        valid_len: off as u64,
        stop,
    })
}

/// Checkpoint: `magic[8] | generation u64 | count u64 | count × (key_len u32, key, val_len u32,
/// val) | crc u32`; crc önceki tüm baytlar üzerinde; anahtarlar kesin artan (BTreeMap sırası).
pub fn encode_checkpoint(generation: u64, state: &State) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&CHECKPOINT_MAGIC);
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&(state.len() as u64).to_le_bytes());
    for (k, v) in state {
        push_bytes(&mut out, k);
        push_bytes(&mut out, v);
    }
    let crc = crc32(&out);
    push_u32(&mut out, crc);
    out
}

/// Checkpoint'i çözer. Kısa dosya, yanlış magic, CRC uyuşmazlığı, artık bayt ya da sıralama ihlali
/// → `Err(Corruption)`. Checkpoint rename öncesi fsync'lendiği için her zaman bütün olmalıdır;
/// bozuksa bu gerçek bir hatadır.
pub fn decode_checkpoint(bytes: &[u8]) -> Result<(u64, State), AdapterError> {
    let corrupt = |why: &str| AdapterError::Corruption(format!("checkpoint: {why}"));
    if bytes.len() < CHECKPOINT_MIN_LEN {
        return Err(corrupt("çok kısa"));
    }
    let (body, crc_b) = bytes.split_at(bytes.len() - 4);
    if body[..8] != CHECKPOINT_MAGIC {
        return Err(corrupt("magic uyuşmuyor"));
    }
    if crc32(body) != u32::from_le_bytes(crc_b.try_into().expect("4 bayt")) {
        return Err(corrupt("CRC uyuşmuyor"));
    }
    let mut r = Reader { buf: body, pos: 8 };
    let generation = r.u64().ok_or_else(|| corrupt("nesil eksik"))?;
    let count = r.u64().ok_or_else(|| corrupt("sayı eksik"))?;
    let mut state = State::new();
    for _ in 0..count {
        let k = r.bytes().ok_or_else(|| corrupt("anahtar eksik"))?;
        let v = r.bytes().ok_or_else(|| corrupt("değer eksik"))?;
        if state.last_key_value().is_some_and(|(last, _)| *last >= k) {
            return Err(corrupt("anahtarlar kesin artan değil"));
        }
        state.insert(k, v);
    }
    if !r.at_end() {
        return Err(corrupt("artık bayt"));
    }
    Ok((generation, state))
}
