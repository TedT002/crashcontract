//! `ToyDb`: WAL + checkpoint'li KV motoru ve `StorageAdapter` gerçeklemesi.

use std::io;
use std::path::Path;

use cc_adapter::{AdapterError, StorageAdapter};
use cc_model::{Durability, Guarantees, State, Value, Write};
use cc_trace::{DirVfs, FileId, OpenMode, TraceLog, Vfs};

use crate::format::{
    CHECKPOINT_FILE, CHECKPOINT_TMP_FILE, MAX_RECORD_PAYLOAD, WAL_HEADER_LEN, decode_checkpoint,
    decode_wal, encode_checkpoint, encode_wal_record, parse_wal_file_name, wal_file_name,
};

/// Senkronizasyon politikası → bildirilen Durability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPolicy {
    /// Her Put/Delete/Txn kaydı ACK'ten önce fdatasync'lenir → `Durability::OnAck`.
    EveryCommit,
    /// Kayıtlar yalnız `sync()` ile fdatasync'lenir (grup commit) → `Durability::OnSync`.
    OnSyncOnly,
}

/// Motor yapılandırması. Testler `ToyConfig { .., ..ToyConfig::default() }` kullanır
/// (Aşama 5 alan eklerse kırılmaz).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToyConfig {
    /// Ne zaman fdatasync yapılır.
    pub sync_policy: SyncPolicy,
    /// Mevcut WAL'daki kayıt sayısı bu değere ulaşınca (işlemin içinde, kaydı yazdıktan sonra)
    /// checkpoint. 0 = otomatik checkpoint yok (açılış checkpoint'i her zaman yapılır).
    pub checkpoint_every: u32,
}

impl Default for ToyConfig {
    fn default() -> Self {
        ToyConfig {
            sync_policy: SyncPolicy::EveryCommit,
            checkpoint_every: 64,
        }
    }
}

/// WAL + checkpoint'li küçük KV motoru. Dosya sistemine YALNIZ `Box<dyn Vfs>` üzerinden erişir.
pub struct ToyDb {
    vfs: Box<dyn Vfs>,
    cfg: ToyConfig,
    /// Bellekteki tam durum; okumalar G/Ç yapmaz.
    mem: State,
    /// Geçerli nesil: CHECKPOINT bu nesli taşır, ekleme `wal-<gen>` dosyasına yapılır.
    generation: u64,
    wal: FileId,
    wal_len: u64,
    next_lsn: u64,
    wal_records: u32,
}

impl std::fmt::Debug for ToyDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToyDb")
            .field("cfg", &self.cfg)
            .field("generation", &self.generation)
            .field("keys", &self.mem.len())
            .finish_non_exhaustive()
    }
}

fn apply_writes(state: &mut State, writes: &[Write]) {
    for w in writes {
        match w {
            Write::Put { key, value } => {
                state.insert(key.clone(), value.clone());
            }
            Write::Delete { key } => {
                state.remove(key);
            }
        }
    }
}

/// Checkpoint g → g+1 (C1–C4, SPEC §4.3). Yeni WAL'ın tanıtıcısını döndürür.
///
/// `old_wal`: hâlâ açık olan `wal-g` tanıtıcısı (kurtarma checkpoint'inde zaten kapalıdır).
fn checkpoint(
    vfs: &mut dyn Vfs,
    mem: &State,
    g: u64,
    old_wal: Option<FileId>,
) -> io::Result<FileId> {
    // C1 — "referanstan önce var ol": yeni WAL'ın ad girdisi, ona referans verecek CHECKPOINT
    // kalıcılaşmadan ÖNCE dizin fsync'iyle sabitlenir. Aksi halde çökmeden sonra CHECKPOINT nesil
    // g+1'i gösterip wal-(g+1) hiç var olmayabilirdi.
    let new_wal = vfs.open(&wal_file_name(g + 1), OpenMode::CreateNew)?;
    vfs.sync_dir()?;

    // C2 — rename öncesi fsync: içerik rename'den önce sabitlenmezse, rename kalıcı olup içerik
    // olmayabilir (sıfır uzunluklu ya da yarım CHECKPOINT) — klasik "rename ile atomik değiştirme"
    // tuzağı.
    let tmp = vfs.open(CHECKPOINT_TMP_FILE, OpenMode::CreateNew)?;
    vfs.write_at(tmp, 0, &encode_checkpoint(g + 1, mem))?;
    // MUTASYON NOKTASI (Aşama 5): M5 rename öncesi fsync yok — bu fsync'i atla.
    vfs.fsync(tmp)?;

    // C3 — atomik değiştirme: rename, eski ya da yeni CHECKPOINT'i bütün olarak bırakır; ama ad
    // değişikliği ancak dizin fsync'iyle kalıcıdır.
    vfs.rename(CHECKPOINT_TMP_FILE, CHECKPOINT_FILE)?;
    // MUTASYON NOKTASI (Aşama 5): M2 fsync sırası — C4'teki unlink'i bu sync_dir'den önceye al.
    vfs.sync_dir()?;
    vfs.close(tmp)?;

    // C4 — eski WAL ancak yeni CHECKPOINT kalıcıyken silinir. Unlink'in kendisi için dizin
    // fsync'i yok: kaybolursa açılıştaki GC (R3) dosyayı yeniden siler.
    if g >= 1 {
        if let Some(w) = old_wal {
            vfs.close(w)?;
        }
        vfs.unlink(&wal_file_name(g))?;
    }
    Ok(new_wal)
}

impl ToyDb {
    /// Checkpoint yap ve nesil durumunu güncelle (C1–C5).
    fn do_checkpoint(&mut self) -> io::Result<()> {
        let new_wal = checkpoint(
            self.vfs.as_mut(),
            &self.mem,
            self.generation,
            Some(self.wal),
        )?;
        // C5 — LSN her dosyada 1'den başlar.
        self.generation += 1;
        self.wal = new_wal;
        self.wal_len = 0;
        self.next_lsn = 1;
        self.wal_records = 0;
        Ok(())
    }

    /// Bir işlemi (Put/Delete/Txn) tek WAL kaydı olarak yazar (W1/W2).
    fn commit(&mut self, writes: &[Write]) -> Result<(), AdapterError> {
        if writes.is_empty() {
            // Boş Txn: etkisiz, kayıt yazılmaz.
            return Ok(());
        }
        let rec = encode_wal_record(self.next_lsn, writes);
        if rec.len() - WAL_HEADER_LEN > MAX_RECORD_PAYLOAD as usize {
            // G/Ç'den ÖNCE reddet: kurtarma bu kaydı TooLong sayıp orada dururdu.
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "kayıt çok büyük").into());
        }
        // Tek kayıt = tek write_at: Txn atomikliği kaydın CRC'sinden gelir.
        self.vfs.write_at(self.wal, self.wal_len, &rec)?;
        self.wal_len += rec.len() as u64;
        self.next_lsn += 1;
        self.wal_records += 1;
        if self.cfg.sync_policy == SyncPolicy::EveryCommit {
            // W1 — fsync sınırı: OnAck vaadi için kayıt ACK'ten önce kalıcı olmalı. fdatasync
            // yeter: ekleme dosya boyutunu değiştirir ve boyut, veriyi okumak için gereken meta
            // veridir (fdatasync(2)).
            // MUTASYON NOKTASI (Aşama 5): M1 fsync atla — bu fdatasync'i atla.
            self.vfs.fdatasync(self.wal)?;
        }
        // W2 (OnSyncOnly): ACK ≠ dayanıklılık — kayıt sayfa önbelleğinde, sync()'i bekliyor.
        apply_writes(&mut self.mem, writes);
        if self.cfg.checkpoint_every > 0 && self.wal_records == self.cfg.checkpoint_every {
            self.do_checkpoint()?;
        }
        Ok(())
    }
}

/// Bir dizini açar / kurtarır (R1–R5, SPEC §4.4).
fn recover(mut vfs: Box<dyn Vfs>, cfg: &ToyConfig) -> Result<ToyDb, AdapterError> {
    // R1 — tanınmayan dosya, başka bir şeyin dizine yazdığını gösterir: tahmin etme, reddet.
    let names = vfs.list()?;
    if let Some(bad) = names.iter().find(|n| {
        *n != CHECKPOINT_FILE && *n != CHECKPOINT_TMP_FILE && parse_wal_file_name(n).is_none()
    }) {
        return Err(AdapterError::Corruption(format!("tanınmayan dosya: {bad}")));
    }

    if !names.iter().any(|n| n == CHECKPOINT_FILE) {
        // R2 — bootstrap (ya da bootstrap sırasında çökme). CHECKPOINT ancak bootstrap'ın sonunda
        // kalıcı olur ve işlemler ondan sonra başlar; yani henüz hiçbir işlem ACK'lenmemiştir,
        // geride kalan her şey güvenle silinebilir.
        for n in &names {
            vfs.unlink(n)?;
        }
        let mem = State::new();
        let wal = checkpoint(vfs.as_mut(), &mem, 0, None)?;
        return Ok(ToyDb {
            vfs,
            cfg: cfg.clone(),
            mem,
            generation: 1,
            wal,
            wal_len: 0,
            next_lsn: 1,
            wal_records: 0,
        });
    }

    // R3 — CHECKPOINT her zaman bütündür (C2); bozuksa gerçek bir hatadır, gizlenmez.
    let cp = vfs.open(CHECKPOINT_FILE, OpenMode::Existing)?;
    let bytes = vfs.read_all(cp)?;
    vfs.close(cp)?;
    let (g, mut state) = decode_checkpoint(&bytes)?;
    // İdempotent GC: yarım checkpoint'in TMP'si ve nesli ≠ g olan WAL'lar. g+1 nesilli WAL
    // tamamlanmamış bir checkpoint'e aittir, ona hiç kayıt yazılmamıştır → silmek güvenli.
    for n in &names {
        let stale = n == CHECKPOINT_TMP_FILE || parse_wal_file_name(n).is_some_and(|ng| ng != g);
        if stale {
            vfs.unlink(n)?;
        }
    }

    // R4 — WAL replay: ilk bozuk kayıtta dur (prefix tutarlılığı).
    let wal_name = wal_file_name(g);
    if !names.contains(&wal_name) {
        return Err(AdapterError::Corruption(format!(
            "CHECKPOINT nesil {g} ama {wal_name} yok"
        )));
    }
    let w = vfs.open(&wal_name, OpenMode::Existing)?;
    let bytes = vfs.read_all(w)?;
    vfs.close(w)?;
    let replay = decode_wal(&bytes)?;
    // MUTASYON NOKTASI (Aşama 5): M3 son kaydı at — `replay.records`'ın sonuncusunu uygulama.
    for writes in &replay.records {
        // MUTASYON NOKTASI (Aşama 5): M4 tombstone yoksay — Delete yazmalarını atla.
        apply_writes(&mut state, writes);
    }

    // R5 — kurtarma checkpoint'i: kurtarılan WAL'a ASLA ekleme yapılmaz; yeni nesil açılır.
    // Böylece geçersiz kuyruğun ardındaki eski-geçerli kayıtlar asla diriltilemez.
    let wal = checkpoint(vfs.as_mut(), &state, g, None)?;
    Ok(ToyDb {
        vfs,
        cfg: cfg.clone(),
        mem: state,
        generation: g + 1,
        wal,
        wal_len: 0,
        next_lsn: 1,
        wal_records: 0,
    })
}

impl StorageAdapter for ToyDb {
    type Config = ToyConfig;

    /// Txn = tek CRC'li kayıt (ya hep ya hiç); WAL sıralı + ilk bozuk kayıtta durma + atomik
    /// checkpoint değişimi → kurtarılan durum her zaman bir prefix.
    fn guarantees(config: &ToyConfig) -> Guarantees {
        Guarantees {
            durability: match config.sync_policy {
                SyncPolicy::EveryCommit => Durability::OnAck,
                SyncPolicy::OnSyncOnly => Durability::OnSync,
            },
            atomic_txn: true,
            prefix_order: true,
        }
    }

    fn open(dir: &Path, config: &ToyConfig, log: Option<TraceLog>) -> Result<Self, AdapterError> {
        let vfs = DirVfs::new(dir, log)?;
        recover(Box::new(vfs), config)
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), AdapterError> {
        self.commit(&[Write::Put {
            key: key.to_vec(),
            value: value.to_vec(),
        }])
    }

    fn get(&mut self, key: &[u8]) -> Result<Option<Value>, AdapterError> {
        Ok(self.mem.get(key).cloned())
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), AdapterError> {
        // Olmayan anahtar için de kayıt (tombstone) yazılır: etkisizdir ama iz yapısı tekdüze kalır.
        self.commit(&[Write::Delete { key: key.to_vec() }])
    }

    fn txn(&mut self, writes: &[Write]) -> Result<(), AdapterError> {
        self.commit(writes)
    }

    fn sync(&mut self) -> Result<(), AdapterError> {
        // W3 — grup commit bariyeri: o ana kadarki tüm kayıtlar kalıcı olur.
        self.vfs.fdatasync(self.wal)?;
        Ok(())
    }

    fn scan(&mut self) -> Result<State, AdapterError> {
        Ok(self.mem.clone())
    }

    fn close(mut self) -> Result<(), AdapterError> {
        // Yalnız tanıtıcıyı bırakır; close dayanıklılık sağlamaz.
        self.vfs.close(self.wal)?;
        Ok(())
    }
}
