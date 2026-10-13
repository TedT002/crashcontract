//! Çökme durumu üreteci: iz + kesme noktası → izinli dizin görüntüleri (SPEC_stage3 §0).
//!
//! ## Neden "bekleyen işlemlerin herhangi bir alt kümesi"?
//!
//! Çekirdek, `fsync` ile sabitlenmemiş veriyi ve dizin fsync'i ile sabitlenmemiş ad değişikliklerini
//! istediği sırayla diske yazabilir — ya da hiç yazmadan çökebilir. POSIX bu yazılar arasında bir sıra
//! vaat etmez (ext4'ün `data=ordered` davranışı bir uygulama ayrıntısıdır). Muhafazakâr model bu yüzden
//! bekleyen işlemlerin her alt kümesini mümkün sayar. Seçilen alt küme yine **program sırasıyla**
//! uygulanır: sayfa önbelleği her baytın en son değerini tutar, geri yazma eski bir değeri yenisinin
//! üstüne yazmaz.
//!
//! ## Neden dosya fsync'i adı kalıcı kılmaz?
//!
//! İçerik inode'a, ad dizine aittir. `fsync(fd)` inode'u sabitler; yeni oluşturulan bir dosyanın,
//! yeniden adlandırılan ya da silinen bir girdinin kalıcılığı için dizinin kendisi fsync'lenmelidir.
//! ext4 yeni dosyada bunu fiilen yapsa da POSIX garanti etmez; gerçek bir motor buna güveniyorsa bu
//! model yanlış alarm verebilir → Aşama 7'de strace ile ölçülüp not düşülecek.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use crate::{FileId, TraceEvent};

/// Ad → içerik: materyalize edilecek dizin görüntüsü.
pub type Image = BTreeMap<String, Vec<u8>>;

/// Üretici ayarları.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CrashConfig {
    /// Bekleyen işlem sayısı ≤ bu ise TÜM alt kümeler denenir (2^n).
    pub exhaustive_limit: u32,
    /// Tam olmayan modda eklenen seed'li rastgele alt küme sayısı.
    pub random_samples: u32,
    /// Bekleyen yazmaların sektör seviyesinde yırtılmasını da dene.
    pub torn_writes: bool,
    /// Atomik yazılan sektör boyu (bayt).
    pub sector_size: u64,
}

impl Default for CrashConfig {
    fn default() -> Self {
        CrashConfig {
            exhaustive_limit: 8,
            random_samples: 8,
            torn_writes: false,
            sector_size: 512,
        }
    }
}

/// Yırtık bir yazma: yalnız `pieces` indeksli sektör parçaları kalıcı oldu.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct TornWrite {
    /// Yazmanın olay indeksi (bekleyen bir `Write` olmalı).
    pub event: usize,
    /// Kalıcı parçaların indeksleri (artan, tekrarsız).
    pub pieces: Vec<usize>,
}

/// Bir çökme durumunu tek başına belirleyen seçim: hangi bekleyen işlemler kalıcı oldu.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Choice {
    /// Kalıcı olan bekleyen işlemlerin olay indeksleri (artan).
    pub persisted: Vec<usize>,
    /// Varsa yırtık yazma (o olay `persisted` içinde OLMAZ).
    pub torn: Option<TornWrite>,
}

/// Üretilen tek çökme durumu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashState {
    /// Kesme noktası: ilk `cut` olay gerçekleşti.
    pub cut: usize,
    /// Durumu üreten seçim (`build_image(events, cut, &choice) == image`).
    pub choice: Choice,
    /// Dizin görüntüsü.
    pub image: Image,
}

/// İz ya da seçim tutarsız.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrashError {
    /// `cut > events.len()`.
    CutOutOfRange {
        /// İstenen kesme.
        cut: usize,
        /// Olay sayısı.
        len: usize,
    },
    /// Olay, izde yaratılmamış bir dosyaya başvuruyor.
    UnknownFile {
        /// Olay indeksi.
        event: usize,
        /// Bilinmeyen kimlik.
        file: FileId,
    },
    /// Olay, o anda var olmayan bir ada başvuruyor.
    UnknownName {
        /// Olay indeksi.
        event: usize,
        /// Bilinmeyen ad.
        name: String,
    },
    /// Seçimdeki olay, kesme anında bekleyen bir işlem değil.
    NotPending {
        /// Olay indeksi.
        event: usize,
    },
    /// Yırtık yazma geçersiz (bekleyen Write değil, parça aralık dışı ya da `persisted` içinde).
    BadTorn {
        /// Olay indeksi.
        event: usize,
    },
}

impl fmt::Display for CrashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CrashError::CutOutOfRange { cut, len } => {
                write!(f, "kesme {cut} olay sayısını ({len}) aşıyor")
            }
            CrashError::UnknownFile { event, file } => {
                write!(f, "olay {event}: bilinmeyen dosya {file:?}")
            }
            CrashError::UnknownName { event, name } => {
                write!(f, "olay {event}: bilinmeyen ad {name:?}")
            }
            CrashError::NotPending { event } => write!(f, "olay {event} bekleyen işlem değil"),
            CrashError::BadTorn { event } => write!(f, "olay {event}: geçersiz yırtık yazma"),
        }
    }
}

impl std::error::Error for CrashError {}

/// Bekleyen veri işlemi.
#[derive(Debug, Clone)]
enum DataOp {
    Write { offset: u64, data: Vec<u8> },
    SetLen(u64),
}

/// Bekleyen ad işlemi. Rename hedef inode'u iz anında çözülür: kalıcı olduğunda `to` o inode'u
/// gösterir — `from`'u yaratan işlem kalıcı olmasa bile (rename inode'u taşır, adı değil).
#[derive(Debug, Clone)]
enum DirOp {
    Link {
        name: String,
        file: FileId,
    },
    Rename {
        from: String,
        to: String,
        file: FileId,
    },
    Unlink {
        name: String,
    },
}

fn apply_data(content: &mut Vec<u8>, op: &DataOp) {
    match op {
        DataOp::Write { offset, data } => write_into(content, *offset, data),
        DataOp::SetLen(len) => content.resize(*len as usize, 0),
    }
}

/// `offset`'e yaz; dosya kısaysa aradaki delik sıfırla dolar (kalıcı olmayan yazmanın yeri).
fn write_into(content: &mut Vec<u8>, offset: u64, data: &[u8]) {
    let off = offset as usize;
    let end = off + data.len();
    if content.len() < end {
        content.resize(end, 0);
    }
    content[off..end].copy_from_slice(data);
}

fn apply_dir(dir: &mut BTreeMap<String, FileId>, op: &DirOp) {
    match op {
        DirOp::Link { name, file } => {
            dir.insert(name.clone(), *file);
        }
        DirOp::Rename { from, to, file } => {
            dir.remove(from);
            dir.insert(to.clone(), *file);
        }
        DirOp::Unlink { name } => {
            dir.remove(name);
        }
    }
}

/// `events[0..cut)` oynatıldıktan sonraki dayanıklı + bekleyen durum.
#[derive(Debug, Default)]
struct Replay {
    durable_data: BTreeMap<FileId, Vec<u8>>,
    pending_data: BTreeMap<FileId, Vec<(usize, DataOp)>>,
    durable_dir: BTreeMap<String, FileId>,
    pending_dir: Vec<(usize, DirOp)>,
}

impl Replay {
    fn run(events: &[TraceEvent], cut: usize) -> Result<Replay, CrashError> {
        if cut > events.len() {
            return Err(CrashError::CutOutOfRange {
                cut,
                len: events.len(),
            });
        }
        let mut r = Replay::default();
        // Uçucu (çökmesiz) dizin: rename/unlink'in hangi inode'a dokunduğunu çözmek için.
        let mut live: BTreeMap<String, FileId> = BTreeMap::new();
        for (i, e) in events[..cut].iter().enumerate() {
            let known = |r: &Replay, file: FileId| {
                if r.durable_data.contains_key(&file) {
                    Ok(())
                } else {
                    Err(CrashError::UnknownFile { event: i, file })
                }
            };
            let unknown_name = |name: &str| CrashError::UnknownName {
                event: i,
                name: name.to_string(),
            };
            match e {
                TraceEvent::Open {
                    name,
                    file,
                    created: true,
                } => {
                    r.durable_data.insert(*file, Vec::new());
                    live.insert(name.clone(), *file);
                    r.pending_dir.push((
                        i,
                        DirOp::Link {
                            name: name.clone(),
                            file: *file,
                        },
                    ));
                }
                TraceEvent::Open {
                    name,
                    file,
                    created: false,
                } => {
                    if live.get(name) != Some(file) {
                        return Err(unknown_name(name));
                    }
                }
                TraceEvent::Write { file, offset, data } => {
                    known(&r, *file)?;
                    let op = DataOp::Write {
                        offset: *offset,
                        data: data.clone(),
                    };
                    r.pending_data.entry(*file).or_default().push((i, op));
                }
                TraceEvent::SetLen { file, len } => {
                    known(&r, *file)?;
                    let op = DataOp::SetLen(*len);
                    r.pending_data.entry(*file).or_default().push((i, op));
                }
                // fsync ≡ fdatasync bu modelde: ikisi de içerik + boyutu sabitler; ad girdisini DEĞİL.
                TraceEvent::Fsync { file } | TraceEvent::Fdatasync { file } => {
                    known(&r, *file)?;
                    let content = r
                        .durable_data
                        .get_mut(file)
                        .expect("değişmez: known() az önce doğruladı");
                    for (_, op) in r.pending_data.remove(file).unwrap_or_default() {
                        apply_data(content, &op);
                    }
                }
                TraceEvent::Rename { from, to } => {
                    let file = live.remove(from).ok_or_else(|| unknown_name(from))?;
                    live.insert(to.clone(), file);
                    r.pending_dir.push((
                        i,
                        DirOp::Rename {
                            from: from.clone(),
                            to: to.clone(),
                            file,
                        },
                    ));
                }
                TraceEvent::Unlink { name } => {
                    live.remove(name).ok_or_else(|| unknown_name(name))?;
                    r.pending_dir
                        .push((i, DirOp::Unlink { name: name.clone() }));
                }
                TraceEvent::SyncDir => {
                    for (_, op) in std::mem::take(&mut r.pending_dir) {
                        apply_dir(&mut r.durable_dir, &op);
                    }
                }
            }
        }
        Ok(r)
    }

    /// Tüm bekleyen işlemlerin olay indeksleri, artan.
    fn pending(&self) -> Vec<usize> {
        let mut p: Vec<usize> = self
            .pending_data
            .values()
            .flat_map(|ops| ops.iter().map(|(i, _)| *i))
            .chain(self.pending_dir.iter().map(|(i, _)| *i))
            .collect();
        p.sort_unstable();
        p
    }

    /// Bekleyen bir yazmanın sektör parçaları (mutlak ofsete hizalı).
    fn pending_write(&self, event: usize) -> Option<(FileId, u64, &[u8])> {
        self.pending_data.iter().find_map(|(file, ops)| {
            ops.iter().find_map(|(i, op)| match op {
                DataOp::Write { offset, data } if *i == event => {
                    Some((*file, *offset, data.as_slice()))
                }
                _ => None,
            })
        })
    }

    fn image(&self, choice: &Choice, sector: u64) -> Result<Image, CrashError> {
        let pending: BTreeSet<usize> = self.pending().into_iter().collect();
        let persisted: BTreeSet<usize> = choice.persisted.iter().copied().collect();
        if let Some(bad) = persisted.iter().find(|i| !pending.contains(i)) {
            return Err(CrashError::NotPending { event: *bad });
        }
        if let Some(t) = &choice.torn {
            let bad = CrashError::BadTorn { event: t.event };
            let (_, offset, data) = self.pending_write(t.event).ok_or(bad.clone())?;
            let n = pieces(offset, data, sector).len();
            if persisted.contains(&t.event) || t.pieces.iter().any(|&p| p >= n) {
                return Err(bad);
            }
        }

        let mut dir = self.durable_dir.clone();
        for (i, op) in &self.pending_dir {
            if persisted.contains(i) {
                apply_dir(&mut dir, op);
            }
        }
        // Yalnız görüntüde bir adı olan inode'lar materyalize edilir; adı kalıcı olmayan dosyanın
        // içeriği kalıcı olsa bile erişilemez.
        let mut contents: BTreeMap<FileId, Vec<u8>> = BTreeMap::new();
        let mut image = Image::new();
        for (name, file) in dir {
            let c = contents
                .entry(file)
                .or_insert_with(|| self.content(file, choice, &persisted, sector));
            image.insert(name, c.clone());
        }
        Ok(image)
    }

    /// Bir inode'un çökme sonrası içeriği: dayanıklı içerik + kalıcı seçilen bekleyen işlemler,
    /// program sırasıyla.
    fn content(
        &self,
        file: FileId,
        choice: &Choice,
        persisted: &BTreeSet<usize>,
        sector: u64,
    ) -> Vec<u8> {
        let mut c = self.durable_data[&file].clone();
        for (i, op) in self.pending_data.get(&file).into_iter().flatten() {
            match (&choice.torn, op) {
                (Some(t), DataOp::Write { offset, data }) if t.event == *i => {
                    let parts = pieces(*offset, data, sector);
                    for &p in &t.pieces {
                        let (off, bytes) = parts[p];
                        write_into(&mut c, off, bytes);
                    }
                }
                _ if persisted.contains(i) => apply_data(&mut c, op),
                _ => {}
            }
        }
        c
    }
}

/// Bir yazmayı mutlak ofsete hizalı sektör parçalarına böler: sektör yazımı atomiktir, yırtılma
/// ancak sektör sınırlarında olur.
fn pieces(offset: u64, data: &[u8], sector: u64) -> Vec<(u64, &[u8])> {
    let mut out = Vec::new();
    let mut off = offset;
    let mut rest = data;
    while !rest.is_empty() {
        let room = (sector - off % sector) as usize;
        let take = room.min(rest.len());
        out.push((off, &rest[..take]));
        off += take as u64;
        rest = &rest[take..];
    }
    out
}

/// Kesme anında bekleyen (kalıcılığı belirsiz) işlemlerin olay indeksleri, artan.
pub fn pending_ops(events: &[TraceEvent], cut: usize) -> Result<Vec<usize>, CrashError> {
    Ok(Replay::run(events, cut)?.pending())
}

/// `(events, cut, choice)` → tek dizin görüntüsü. Aşama 6 `replay` bu üçlüyü saklar.
/// Yırtık yazmada sektör boyu [`CrashConfig::default`]'tan (512) farklıysa [`crash_states`]'in
/// ürettiği seçimler için aynı boyla [`build_image_with_sector`] kullanılmalıdır.
pub fn build_image(
    events: &[TraceEvent],
    cut: usize,
    choice: &Choice,
) -> Result<Image, CrashError> {
    build_image_with_sector(events, cut, choice, CrashConfig::default().sector_size)
}

/// [`build_image`], açık sektör boyuyla.
pub fn build_image_with_sector(
    events: &[TraceEvent],
    cut: usize,
    choice: &Choice,
    sector_size: u64,
) -> Result<Image, CrashError> {
    Replay::run(events, cut)?.image(choice, sector_size)
}

/// SplitMix64 — iş yükü üreteciyle aynı sabitler; algoritma sürümler arası sabit kalmalı
/// (seed → durum listesi eşlemesi raporların replay'i için kontrattır).
struct SplitMix64(u64);
impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Kesme noktasındaki çökme durumları (SPEC_stage3 §2 sırasıyla, görüntüye göre tekrarsız).
///
/// Neden tam alt küme listesi her zaman değil? `n` bekleyen işlem 2^n durum verir; grup commit'li
/// bir motorda n kolayca 50'yi geçer. Bu yüzden küçük n'de tam liste, büyük n'de yapısal aileler
/// (önekler = "sırayla kalıcı oldu", biri-hariç = "tam ortadaki bir yazı kayboldu") + seed'li
/// rastgele örnekler kullanılır.
pub fn crash_states(
    events: &[TraceEvent],
    cut: usize,
    seed: u64,
    cfg: &CrashConfig,
) -> Result<Vec<CrashState>, CrashError> {
    let replay = Replay::run(events, cut)?;
    let p = replay.pending();
    let n = p.len();
    let subset = |pick: &dyn Fn(usize) -> bool| -> Vec<usize> {
        (0..n).filter(|&i| pick(i)).map(|i| p[i]).collect()
    };

    let mut choices: Vec<Choice> = Vec::new();
    let plain = |persisted| Choice {
        persisted,
        torn: None,
    };
    // n < 32: maske taşmasın (exhaustive_limit büyük verilse bile 2^32 durum zaten anlamsız).
    if n <= cfg.exhaustive_limit as usize && n < 32 {
        for mask in 0u64..(1u64 << n) {
            choices.push(plain(subset(&|i| mask >> i & 1 == 1)));
        }
    } else {
        for k in 0..=n {
            choices.push(plain(subset(&|i| i < k)));
        }
        for j in 0..n {
            choices.push(plain(subset(&|i| i != j)));
        }
        let mut rng = SplitMix64(seed ^ (cut as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        for _ in 0..cfg.random_samples {
            let bits: Vec<bool> = (0..n).map(|_| rng.next() & 1 == 1).collect();
            choices.push(plain(subset(&|i| bits[i])));
        }
    }
    if cfg.torn_writes {
        for (k, &event) in p.iter().enumerate() {
            let Some((_, offset, data)) = replay.pending_write(event) else {
                continue;
            };
            let count = pieces(offset, data, cfg.sector_size).len();
            if count < 2 {
                continue;
            }
            // Sonraki bekleyenler hem düşmüş hem kalıcı olmuş haliyle denenir. İkincisi kritik:
            // yırtık kayıt dosyanın sonunda değil ortasındadır (ardından gelen kayıt diske ulaşmış),
            // uzunluk alanı yeterli görünür ve kaydı yalnız checksum reddedebilir.
            let before = p[..k].to_vec();
            let all_but = [&p[..k], &p[k + 1..]].concat();
            for persisted in [before, all_but] {
                for kept in [vec![0], (1..count).collect()] {
                    choices.push(Choice {
                        persisted: persisted.clone(),
                        torn: Some(TornWrite {
                            event,
                            pieces: kept,
                        }),
                    });
                }
            }
        }
    }

    let mut seen: BTreeSet<Image> = BTreeSet::new();
    let mut out = Vec::new();
    for choice in choices {
        let image = replay.image(&choice, cfg.sector_size)?;
        if seen.insert(image.clone()) {
            out.push(CrashState { cut, choice, image });
        }
    }
    Ok(out)
}

/// Görüntüyü boş bir dizine yazar (motor buradan yeniden açılır).
pub fn materialize(image: &Image, dir: &Path) -> io::Result<()> {
    if fs::read_dir(dir)?.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::DirectoryNotEmpty,
            "materialize: hedef dizin boş olmalı",
        ));
    }
    for (name, content) in image {
        fs::write(dir.join(name), content)?;
    }
    Ok(())
}
