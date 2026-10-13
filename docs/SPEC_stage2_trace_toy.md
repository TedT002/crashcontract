# Aşama 2 — `cc-trace` + `cc-adapter` + `toy-correct` — Teknik Sözleşme

> Bağlam: `/CLAUDE.md` (kararlar 1–5), `docs/PLAN_crashcontract.md` §5 Aşama 2, `docs/SPEC_stage1_model.md`.
> **Bağlayıcıdır.** Public imzalar, olay sırası, bayt formatları ve fsync düzeni birebir uygulanır; değişiklik
> gerekirse implementer durur, SPEC revizyonu ister. Private yardımcılar serbesttir. Stil kuralları
> (doküman yorumları, `expect("değişmez: …")`, Türkçe öğretici yorumlar, `HashMap` yasağı) Aşama 1 ile aynıdır.

## 0. Kararlar (gerekçeli)

1. **Olay kimliği: veri işlemleri `FileId` (inode kimliği), ad işlemleri yol bazlı.** POSIX çökme semantiği
   tam bu ayrımı yapar: dosya içeriği `fsync(fd)` ile (inode'a bağlı), ad girdileri dizin `fsync`'iyle
   kalıcılaşır. Saf yol bazlı iz, açık dosya yeniden adlandırılınca/silinince yanlış dosyaya yazar; saf fd
   bazlı iz, fd numaraları yeniden kullanıldığı ve `rename/unlink` fd almadığı için yetmez. Aşama 7 shim'i
   aynı formatı üretebilir: `open` sonrası `fstat` → (dev, ino) → ilk görülme sırasıyla yoğun `FileId`;
   `O_CREAT` ile yeni oluşturma her zaman yeni `FileId` alır (inode yeniden kullanımına karşı).
2. **`write` ve `pwrite` tek `Write{offset}` olayıdır.** Çökme semantiği aynıdır; kaydedici mutlak ofseti
   çözer (shim `write()` için ofseti `lseek(fd,0,SEEK_CUR)` ile alır — Aşama 7'de doğrulanacak).
3. **Okuma, `list`, `close` kaydedilmez** (dayanıklı durumu değiştirmezler). **Başarısız çağrı kaydedilmez**
   (olay yalnız alttaki çağrı başarılı dönünce eklenir). fsync hatası semantiği ("fsyncgate") v1 kapsamı dışı.
4. **Düz ad uzayı:** Vfs kökü tek dizindir; adlar tek bileşen. `SyncDir` kök dizinin fsync'idir.
   Alt dizin gerekirse Aşama 7 SPEC'i ekler (Aşama 6 JSON şemasından önce ucuz değişiklik).
5. **İz boş dizinde başlar.** Kayıtlı `DirVfs` yalnız boş köke bağlanır; böylece başlangıç görüntüsü
   = boş dizin ve iz kendi başına yeterlidir (ayrı başlangıç anlık görüntüsü yok).
6. **İşlem↔iz bağı ayrı `spans` listesiyle** (olay akışına işaretçi karıştırılmaz): `spans[i]` = geçmişin
   `i`. işleminin olay aralığı. `Trace::crash_point` bu aralıklardan `History::crash_view` argümanlarını
   üretir (Aşama 4 riski "işlem↔iz eşlemesi" burada birim testleriyle kapanır).
7. **`DirVfs` gerçek fsync ÇAĞIRMAZ;** fsync/fdatasync/dizin fsync'i yalnız kaydedilir. Gerekçe (karar 1):
   çökme durumları OS'tan değil izden üretilir; gerçek fsync araca doğruluk katmaz, 1.000-seed testini
   dakikalara çıkarır. Sonuç: toy motorlar gerçek diskte dayanıklı DEĞİLDİR (README "Doğrulanmayanlar").
8. **PLAN sapması:** `MemVfs` eklenmez — Aşama 3 durumları gerçek dizine materyalize edeceği için tek
   gerçek uygulama (`DirVfs`) yeterli; ikinci uygulama gerçek FS'ten sapma riski taşır.
9. **Serileştirme yok.** `serde` Aşama 6'da; `TraceEvent`/`Trace`'e derive'lar o zaman eklemeli olarak gelir.
10. **WAL yeniden kullanılmaz:** her checkpoint (açılıştaki kurtarma checkpoint'i dahil) yeni nesil WAL
    dosyası açar; kayıtlar ayrıca dosya içi ardışık LSN taşır. Gerekçe §4.4.
11. **CRC32 elle** (~25 satır, `const fn` tablo); dış crate yok (bağımlılık minimum). Aşama 2'de workspace'e
    hiçbir dış crate EKLENMEZ.

## 1. Dizin Yapısı (birebir)

```
Cargo.toml                       members += "crates/cc-trace", "crates/cc-adapter", "engines/toy-correct"
crates/cc-trace/
  Cargo.toml                     bağımlılık yok
  src/lib.rs                     crate doc + re-export
  src/event.rs                   FileId, TraceEvent, OpSpan, Trace, CrashPoint, TraceLog
  src/vfs.rs                     Vfs, OpenMode
  src/dir_vfs.rs                 DirVfs
  tests/common/mod.rs            fresh_dir yardımcısı
  tests/trace.rs                 T01–T09
crates/cc-adapter/
  Cargo.toml                     deps: cc-model, cc-trace (path)
  src/lib.rs                     StorageAdapter, AdapterError, apply_op
  tests/apply.rs                 A01–A03 (+ test içi MapAdapter)
engines/toy-correct/
  Cargo.toml                     package "toy-correct"; deps: cc-model, cc-trace, cc-adapter (path)
  src/lib.rs                     crate doc (fsync düzeni tablosu §4.3) + re-export
  src/crc32.rs                   CRC-32/IEEE
  src/format.rs                  pub mod format: WAL kaydı + checkpoint kodlama/çözme
  src/db.rs                      ToyDb, ToyConfig, SyncPolicy, StorageAdapter impl
  tests/common/mod.rs            fresh_dir, SplitMix64, gen_ops, run_seed
  tests/format.rs                Y01–Y04
  tests/engine.rs                Y05–Y21
  tests/seeds.rs                 Y22–Y25
```
Her `Cargo.toml`: `version/edition/rust-version/publish.workspace = true`, `[lints] workspace = true`,
her bağımlılığın yanında neden yorumu. `cc-trace` re-export: `pub use dir_vfs::DirVfs;
pub use event::{CrashPoint, FileId, OpSpan, Trace, TraceEvent, TraceLog}; pub use vfs::{OpenMode, Vfs};`
`cc-adapter` ve `toy-correct` öğeleri crate kökünde; `toy_correct::format` public modüldür.
Testlerdeki geçici dizin: `env!("CARGO_TARGET_TMPDIR")/<crate>/<test_adı>` (varsa önce silinir); `tempfile`
crate'i KULLANILMAZ.

## 2. `cc-trace` — Public API (birebir)

```rust
// event.rs
/// İz içindeki dosya (inode) kimliği. Kaydedici başına 0'dan ardışık atanır.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u32);

/// Kaydedilen tek dosya sistemi olayı. Adlar kök dizine göre tek bileşendir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceEvent {
    /// Dosya açıldı. `created`: bu çağrı dosyayı yarattı (yeni dizin girdisi + yeni FileId).
    Open { name: String, file: FileId, created: bool },
    /// `data`, `offset`'ten itibaren yazıldı (write/pwrite). `data` boş olamaz.
    Write { file: FileId, offset: u64, data: Vec<u8> },
    /// ftruncate.
    SetLen { file: FileId, len: u64 },
    /// fsync(fd): içerik + meta veri.
    Fsync { file: FileId },
    /// fdatasync(fd): içerik + veriyi okumak için gereken meta veri (dosya boyutu dahil).
    Fdatasync { file: FileId },
    /// rename(2): `to` varsa atomik olarak değiştirilir.
    Rename { from: String, to: String },
    /// unlink(2).
    Unlink { name: String },
    /// Kök dizinin fsync'i.
    SyncDir,
}

/// Geçmişin bir işleminin iz aralığı: `start` = işlem gönderilirken olay sayısı,
/// `end` = işlem döndüğünde olay sayısı (None: henüz dönmedi). Değişmez: start ≤ end ≤ sonraki.start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpSpan { pub start: usize, pub end: Option<usize> }

/// Tam iz. `spans[i]` ↔ `History` indeksi `i`. İlk span'den önceki olaylar açılış (open) fazıdır.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Trace { pub events: Vec<TraceEvent>, pub spans: Vec<OpSpan> }

/// `History::crash_view(completed, in_flight)` argümanları.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrashPoint { pub completed: usize, pub in_flight: bool }

impl Trace {
    /// `cut` = "ilk `cut` olay gerçekleşti, `cut` indeksli olay gerçekleşmedi" kesme noktası (0..=events.len()).
    /// completed = baştan itibaren `end == Some(e) && e <= cut` olan ardışık span sayısı;
    /// in_flight = `spans[completed]` var ve `start < cut` (en az bir olayı gerçekleşmiş ama dönmemiş).
    /// Olayı hiç gerçekleşmemiş işlem gönderilmemiş sayılır (etkisi diskte olamaz).
    /// # Panics
    /// `cut > events.len()`.
    pub fn crash_point(&self, cut: usize) -> CrashPoint;
}

/// Paylaşılan iz günlüğü (tek iş parçacıklı: `Rc<RefCell<..>>`). Klonu AYNI günlüğü paylaşır.
#[derive(Debug, Clone, Default)]
pub struct TraceLog { /* private: Rc<RefCell<{ trace: Trace, attached: bool }>> */ }

impl TraceLog {
    pub fn new() -> TraceLog;
    /// Olay ekler (DirVfs; Aşama 7'de shim okuyucusu).
    pub fn push(&self, event: TraceEvent);
    /// `spans.push(OpSpan{start: event_count(), end: None})`.
    /// # Panics
    /// `op_index != spans.len()` ya da son span hâlâ açık.
    pub fn begin_op(&self, op_index: usize);
    /// Açık son span'i `end = Some(event_count())` ile kapatır.
    /// # Panics
    /// `op_index` açık son span değil.
    pub fn end_op(&self, op_index: usize);
    pub fn event_count(&self) -> usize;
    /// İzin kopyası.
    pub fn snapshot(&self) -> Trace;
}

// vfs.rs
/// Açma kipi. Her iki kip de okuma+yazma açar; O_TRUNC yoktur (kısaltma yalnız `set_len` ile, kayıtlı).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// O_CREAT|O_EXCL: ad varsa `AlreadyExists`.
    CreateNew,
    /// Var olan dosya: yoksa `NotFound`.
    Existing,
}

/// Toy motorların dosya sistemine TEK erişim yolu (nesne-güvenli; motor `Box<dyn Vfs>` tutar).
pub trait Vfs {
    fn open(&mut self, name: &str, mode: OpenMode) -> io::Result<FileId>;
    /// Tanıtıcıyı kapatır (kaydedilmez; close dayanıklılık sağlamaz).
    fn close(&mut self, file: FileId) -> io::Result<()>;
    /// Dosyanın tamamını okur (kaydedilmez).
    fn read_all(&mut self, file: FileId) -> io::Result<Vec<u8>>;
    /// Boş `data` → olay yok, Ok.
    fn write_at(&mut self, file: FileId, offset: u64, data: &[u8]) -> io::Result<()>;
    fn set_len(&mut self, file: FileId, len: u64) -> io::Result<()>;
    fn fsync(&mut self, file: FileId) -> io::Result<()>;
    fn fdatasync(&mut self, file: FileId) -> io::Result<()>;
    fn rename(&mut self, from: &str, to: &str) -> io::Result<()>;
    fn unlink(&mut self, name: &str) -> io::Result<()>;
    fn sync_dir(&mut self) -> io::Result<()>;
    /// Kökteki adlar, bayt sırasıyla (kaydedilmez).
    fn list(&mut self) -> io::Result<Vec<String>>;
}

// dir_vfs.rs
/// Gerçek bir dizine yazan, `log` verilmişse her başarılı değiştirici çağrıyı kaydeden Vfs.
#[derive(Debug)]
pub struct DirVfs { /* root, log: Option<TraceLog>, open: BTreeMap<FileId, File>,
                       names: BTreeMap<String, FileId>, next_id: u32 */ }
impl DirVfs {
    /// Hatalar: kök yok/dizin değil → OS hatası aynen; `log` Some iken kök boş değil →
    /// `DirectoryNotEmpty`; `log` zaten bir DirVfs'e bağlı → `InvalidInput`.
    pub fn new(root: impl Into<PathBuf>, log: Option<TraceLog>) -> io::Result<DirVfs>;
    pub fn root(&self) -> &Path;
}
impl Vfs for DirVfs { /* §2.1 */ }
```

### 2.1 `DirVfs` davranışı (bağlayıcı)
| Çağrı | Gerçek etki (std) | Olay (yalnız başarıda) |
|-------|-------------------|------------------------|
| open CreateNew | `OpenOptions` read+write+`create_new` | `Open{name, file: yeni id, created: true}`; `names[name]=id` |
| open Existing | read+write | `Open{name, file, created: false}`; id = `names[name]`, yoksa yeni id atanıp eklenir |
| write_at | `FileExt::write_all_at` | `Write{file, offset, data}` |
| set_len | `File::set_len` | `SetLen{file, len}` |
| fsync / fdatasync | **yok** (karar 7) | `Fsync{file}` / `Fdatasync{file}` |
| rename | `fs::rename` | `Rename{from,to}`; `names[to] = names.remove(from)` |
| unlink | `fs::remove_file` | `Unlink{name}`; `names.remove(name)` |
| sync_dir | **yok** (karar 7) | `SyncDir` |
| close / read_all / list | `open`'dan çıkar / okuma / `read_dir` sıralı | yok |

Hatalar: ad boş, `/` veya NUL içerir, `.`/`..` → `InvalidInput` (yol kaçışı engeli). Açık olmayan/bilinmeyen
`FileId` → `InvalidInput`. Zaten açık bir ada `Existing` → `InvalidInput` (bir inode = bir tanıtıcı).
UTF-8 olmayan ad `list`'te → `InvalidData`. FileId'ler kayıtlı/kayıtsız her kipte aynı kuralla atanır.

## 3. `cc-adapter` — Public API (birebir)

```rust
use cc_model::{Guarantees, History, HistoryError, Op, State, Value, Write};
use cc_trace::TraceLog;

/// Adaptör hatası.
#[derive(Debug)]
pub enum AdapterError {
    /// Alttaki G/Ç hatası.
    Io(std::io::Error),
    /// Kalıcı veri, motorun kendi biçim kurallarına göre tutarsız (kurtarma reddedildi).
    Corruption(String),
}
// impl Display (Türkçe tek satır), std::error::Error, From<std::io::Error>.

/// Test edilen motorun sözleşmesi. Tek iş parçacıklı; her çağrı dönene kadar engeller.
pub trait StorageAdapter: Sized {
    /// Motor yapılandırması (Aşama 6 raporuna girer).
    type Config: Clone + std::fmt::Debug;
    /// Bildirilen garantiler YAPILANDIRMANIN fonksiyonudur (açık örnek gerektirmez: açılamayan
    /// çökme durumunda da hüküm verilebilsin).
    fn guarantees(config: &Self::Config) -> Guarantees;
    /// `dir` mevcut olmalı. `log` Some ise motorun tüm dosya sistemi olayları ona eklenir
    /// (toy: `DirVfs`; gerçek motor: Aşama 7 shim). Açılış olayları ilk span'den önce yer alır.
    fn open(dir: &Path, config: &Self::Config, log: Option<TraceLog>) -> Result<Self, AdapterError>;
    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), AdapterError>;
    fn get(&mut self, key: &[u8]) -> Result<Option<Value>, AdapterError>;
    fn delete(&mut self, key: &[u8]) -> Result<(), AdapterError>;
    /// `Op::Txn` karşılığı (begin…commit tek çağrıda). Yerel txn'i olmayan motor yazmaları sırayla
    /// uygular ve `atomic_txn = false` bildirir — cc-model semantiği tam budur; ayrı yetenek bayrağı yok.
    fn txn(&mut self, writes: &[Write]) -> Result<(), AdapterError>;
    /// Dayanıklılık bariyeri (`Op::Sync`).
    fn sync(&mut self) -> Result<(), AdapterError>;
    /// Motorun gördüğü tam mantıksal durum (`check_recovered`'a girer; hayalet anahtarları da yakalar).
    fn scan(&mut self) -> Result<State, AdapterError>;
    /// Motorun kendi bütünlük kontrolü; None = motor desteklemiyor ("geçti" iddiası değil).
    fn integrity_check(&mut self) -> Option<Result<(), AdapterError>> { None }
    /// Temiz kapanış. Dayanıklılık vaadi EKLEMEZ (yalnız kaynakları bırakır).
    fn close(self) -> Result<(), AdapterError>;
}

/// Runner'ın (Aşama 4) tek uygulama noktası: `history.issue(op)` → `log.begin_op(i)` → adaptör çağrısı →
/// `log.end_op(i)` → Put/Delete/Txn/Sync başarı: `ack(i)`; Get başarı: `ack_read(i, v)`;
/// adaptör hatası: `fail(i, err.to_string())`. `i` döner.
/// Hata: yalnız `issue` hatası (ör. `Closed`) — o durumda adaptör çağrılmaz, span açılmaz.
/// `ack*`/`fail` hataları değişmezle imkânsızdır → `expect("değişmez: az önce issue edildi")`.
pub fn apply_op<A: StorageAdapter>(adapter: &mut A, history: &mut History,
                                   log: Option<&TraceLog>, op: Op) -> Result<usize, HistoryError>;
```

## 4. `engines/toy-correct`

### 4.1 Public API (birebir)
```rust
/// Senkronizasyon politikası → bildirilen Durability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPolicy {
    /// Her Put/Delete/Txn kaydı ACK'ten önce fdatasync'lenir → `Durability::OnAck`.
    EveryCommit,
    /// Kayıtlar yalnız `sync()` ile fdatasync'lenir (grup commit) → `Durability::OnSync`.
    OnSyncOnly,
}

/// Testler `ToyConfig { .., ..ToyConfig::default() }` kullanır (Aşama 5 alan eklerse kırılmaz).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToyConfig {
    pub sync_policy: SyncPolicy,
    /// Mevcut WAL'daki kayıt sayısı bu değere ulaşınca (işlemin içinde, kaydı yazdıktan sonra)
    /// checkpoint. 0 = otomatik checkpoint yok (açılış checkpoint'i her zaman yapılır).
    pub checkpoint_every: u32,
}
impl Default for ToyConfig { /* EveryCommit, 64 */ }

/// WAL + checkpoint'li küçük KV motoru. Dosya sistemine YALNIZ `Box<dyn Vfs>` üzerinden erişir.
pub struct ToyDb { /* vfs: Box<dyn Vfs>, cfg, mem: State, gen: u64, wal: FileId, wal_len: u64,
                      next_lsn: u64, wal_records: u32 */ }
impl StorageAdapter for ToyDb { type Config = ToyConfig; /* §4.3–4.5 */ }
```
`guarantees`: `Guarantees { durability: OnAck | OnSync (politikaya göre), atomic_txn: true, prefix_order: true }`.
Gerekçe: Txn = tek CRC'li kayıt (ya hep ya hiç); WAL sıralı + ilk bozuk kayıtta durma + atomik checkpoint
değişimi → kurtarılan durum her zaman bir prefix. `get`/`scan` bellekteki `mem`'den okur (G/Ç yok).
`integrity_check` → varsayılan (None). `close` → tanıtıcıları kapatır, G/Ç olayı yok.

### 4.2 Dosyalar ve bayt formatları (`toy_correct::format`, tümü little-endian)
```rust
pub const CHECKPOINT_FILE: &str = "CHECKPOINT";
pub const CHECKPOINT_TMP_FILE: &str = "CHECKPOINT.tmp";
pub const CHECKPOINT_MAGIC: [u8; 8] = *b"CCTOYCP1";
pub const WAL_HEADER_LEN: usize = 16;
pub const MAX_RECORD_PAYLOAD: u32 = 1 << 24;
pub fn wal_file_name(generation: u64) -> String;           // format!("wal-{generation:016x}")
pub fn parse_wal_file_name(name: &str) -> Option<u64>;     // "wal-" + tam 16 küçük hex, aksi None
pub fn crc32(data: &[u8]) -> u32;                          // CRC-32/IEEE (yansıtılmış 0xEDB88320, init/xorout 0xFFFFFFFF)
pub fn encode_wal_record(lsn: u64, writes: &[Write]) -> Vec<u8>;
pub fn decode_wal(bytes: &[u8]) -> Result<WalReplay, AdapterError>;
pub fn encode_checkpoint(generation: u64, state: &State) -> Vec<u8>;
pub fn decode_checkpoint(bytes: &[u8]) -> Result<(u64, State), AdapterError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalReplay { pub records: Vec<Vec<Write>>, pub valid_len: u64, pub stop: WalStop }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalStop { CleanEnd, TornHeader, LsnMismatch { expected: u64, found: u64 },
                   TooLong { len: u32 }, Truncated, BadCrc }
```
**WAL kaydı:** `lsn u64 | len u32 | crc u32 | payload[len]`; `crc = crc32(lsn_bytes ‖ len_bytes ‖ payload)`.
LSN dosya içinde 1'den ardışık. **Payload:** `count u32`, sonra her yazma: `tag u8` (1=Put, 2=Delete),
`key_len u32`, `key`, Put ise `val_len u32`, `val`. Bir işlem (Put/Delete/Txn) = tam BİR kayıt = tam BİR
`write_at` çağrısı (başlık+payload tek tamponda). Payload > `MAX_RECORD_PAYLOAD` olacak işlem G/Ç'den önce
`AdapterError::Io(InvalidInput)` döner. Boş Txn → kayıt yazılmaz, G/Ç yok, Ok.

**`decode_wal` (ilk bozukta dur), off=0, beklenen=1, her adımda sırayla:**
kalan==0 → `CleanEnd`; kalan<16 → `TornHeader`; lsn≠beklenen → `LsnMismatch`; len>MAX → `TooLong`;
kalan<16+len → `Truncated`; crc uyuşmaz → `BadCrc`; payload çözülemez/artık bayt (CRC geçtiği halde) →
`Err(Corruption)`; aksi kaydı ekle, beklenen+=1, off+=16+len. `valid_len` = son geçerli kaydın sonu.
Sıfır dolu başlık lsn=0 olduğundan asla geçerli değildir (boşluk/delik koruması).

**Checkpoint:** `magic[8] | generation u64 | count u64 | count × (key_len u32, key, val_len u32, val) | crc u32`;
crc önceki tüm baytlar üzerinde; anahtarlar kesin artan. `decode_checkpoint`: uzunluk<28, magic, crc, artık
bayt, sıralama ihlali → `Err(Corruption)`. Bootstrap'ta ilk nesil = 1.

### 4.3 Yazma yolu ve fsync düzeni (birebir; her satır kodda öğretici yorumla kavramına bağlanır)
| # | Nokta | Çağrı dizisi | Kavram / NEDEN |
|---|-------|--------------|----------------|
| W1 | put/delete/txn, `EveryCommit` | `write_at(wal, wal_len, rec)` → `fdatasync(wal)` → `mem`'e uygula → [C?] → ACK | **fsync sınırı:** ACK ≥ dayanıklılık (OnAck). fdatasync yeter: ekleme dosya boyutunu değiştirir, boyut "veriyi okumak için gereken meta veri"dir |
| W2 | put/delete/txn, `OnSyncOnly` | `write_at` → `mem`'e uygula → [C?] → ACK | ACK ≠ dayanıklılık; kayıt sayfa önbelleğinde |
| W3 | `sync()` (her iki politika) | `fdatasync(wal)` | grup commit bariyeri |
| C1 | checkpoint g→g+1 | `open(wal-(g+1), CreateNew)` → `sync_dir` | yeni WAL'ın ad girdisi, ona referans veren CHECKPOINT'ten ÖNCE kalıcı ("referanstan önce var ol") |
| C2 | | `open(CHECKPOINT.tmp, CreateNew)` → `write_at(tmp, 0, encode_checkpoint(g+1, mem))` → `fsync(tmp)` | **rename öncesi fsync:** yoksa rename kalıcı olup içerik olmayabilir (sıfır uzunluklu dosya) |
| C3 | | `rename(CHECKPOINT.tmp, CHECKPOINT)` → `sync_dir` → `close(tmp)` | **atomik değiştirme;** rename yalnız dizin fsync'iyle kalıcı |
| C4 | | (g ≥ 1 ise) `close(wal-g)`, `unlink(wal-g)`; dizin fsync'i YOK | eski WAL ancak yeni CHECKPOINT kalıcıyken silinir; unlink kaybolursa açılışta GC (§4.4 R3) |
| C5 | | `gen=g+1`, `wal_len=0`, `next_lsn=1`, `wal_records=0` | LSN her dosyada 1'den başlar |
[C?] = `checkpoint_every > 0 && wal_records == checkpoint_every` ise C1–C5. Bellek ancak WAL `write_at` başarılı
olduktan sonra değişir; G/Ç hatası → `AdapterError::Io`, `mem` değişmez.

### 4.4 Açılış / kurtarma (birebir sıra)
- R1 `vfs.list()`. Tanınan adlar: `CHECKPOINT`, `CHECKPOINT.tmp`, `parse_wal_file_name` geçerli olanlar;
  başka ad → `Err(Corruption)`.
- R2 `CHECKPOINT` yoksa (bootstrap ya da bootstrap sırasında çökme — henüz hiçbir işlem ACK'lenmemiştir):
  tüm tanınan dosyaları `unlink` et (sıralı), `mem={}`, g=0 ile C1–C3, C5 uygula (C4 yok). Bitti.
- R3 `CHECKPOINT`'i `Existing` aç, `read_all`, `close`; `decode_checkpoint` → (g, S). Hata → `Err(Corruption)`
  (C2 sayesinde CHECKPOINT her zaman bütündür; bozuksa bu gerçek bir hatadır, gizlenmez).
  `CHECKPOINT.tmp` ve nesli ≠ g olan tüm `wal-*` dosyalarını sıralı `unlink` et (dizin fsync'i yok; idempotent GC).
  g+1 nesilli WAL, tamamlanmamış checkpoint'e aittir: ona hiç kayıt yazılmamıştır → silmek güvenli.
- R4 `wal-g` yoksa → `Err(Corruption)` (C1 sayesinde imkânsız olmalı). Varsa `Existing` aç, `read_all`, `close`,
  `decode_wal` (Err → Err); `records`'ı sırayla S'ye uygula (Delete → anahtarı kaldır). `mem = S`.
- R5 **Kurtarma checkpoint'i:** C1–C5 (g→g+1). Böylece kurtarılan WAL'a ASLA ekleme yapılmaz.

**Eski-geçerli-kayıt problemi ve seçim:** "İlk bozukta dur", yarım kalmış kuyruğun ardında eski ama CRC'si
geçerli kayıt yoksa doğrudur. Kurtarmadan sonra aynı WAL'a ekleme yapılsaydı (ya da WAL `ftruncate` ile yeniden
kullanılsaydı ve kısaltma kalıcılaşmasaydı) yeni kısa bir kayıt, eski bir kaydın tam önüne hizalanıp onu
"diriltebilirdi". **Seçim: nesil başına yeni WAL dosyası (birincil) + dosya içi ardışık LSN (savunma).** Taze
dosyada her ofset ya bu nesilde bir kez yazılmıştır ya hiç yazılmamıştır (sıfır/EOF); yalnız-ekleme + yeniden
kullanmama eski kayıt kaynağını yapısal olarak yok eder. Yalnız epoch/LSN seçeneği reddedildi: LSN tek başına
yetmez (diriltilen kaydın LSN'i yeni kaydınkini izleyebilir), açılış başına epoch artırımı + "epoch azalmaz"
kuralı gerekir — daha karmaşık, daha çok mutasyon yüzeyi. LSN yine de ucuz ikinci savunmadır (atlanmış kayıt,
sıfır başlık).

### 4.5 Aşama 5 için mutasyon noktaları (öneri; bu aşamada YALNIZ yorum)
Kodda şu noktalara `// MUTASYON NOKTASI (Aşama 5): <ad>` yorumu konur: M1 fsync atla → W1 `fdatasync`;
M2 fsync sırası → C4'ü C3'ün `sync_dir`'inden önceye al; M3 son kaydı at → R4 son kaydı uygulama;
M4 tombstone yoksay → R4'te Delete'i atla; M5 rename öncesi fsync yok → C2 `fsync(tmp)`; M6 kısmi kaydı
geçerli say → `decode_wal` `Truncated`/`BadCrc` dalları. **Öneri:** Aşama 5'te `ToyConfig`'e
`pub mutation: Option<Mutation>` alanı (varsayılan None) eklenir; `engines/toy-mutants` yalnız mutant
listesi + yakalama testlerini içeren ince crate olur. Ayrı kod kopyası REDDEDİLİR (sürüklenme: mutant ile
doğru motor arasındaki tek fark mutasyon olmalı). Cargo feature REDDEDİLİR (workspace feature birleşmesi).

## 5. Değişmezler ve kenar durumlar
- Kayıtlı koşuda `trace.spans.len() == history.len()`; Get için `start == end`; Put/Delete/Txn (boş değil)
  için span içinde tam bir `Write` (wal'a) ve `EveryCommit`'te hemen ardından aynı FileId'ye `Fdatasync`.
- Aynı seed + aynı yapılandırma → `Trace` (`PartialEq`) eşit; kök yolu ize girmez.
- `put` boş anahtar/değer geçerlidir. `delete` olmayan anahtar için yine kayıt yazar (tombstone, etkisiz).
- `close` dayanıklılık eklemez; ama aynı süreçte yeniden açış sayfa önbelleğini gördüğü için tam durumu görür.
- Kütüphane kodunda `panic` yalnız `TraceLog` kontrat ihlali ve `crash_point` aralık dışı (`# Panics` dokümanlı).

## 6. Kabul Kriterleri

### 6.1 Testler (ID başına en az bir `#[test]`; panic beklenenler ayrı `#[should_panic]` fonksiyon)
Kısaltmalar: `F0..`=FileId, `W0001`=`wal_file_name(1)`, `CP`=`CHECKPOINT`, `TMP`=`CHECKPOINT.tmp`,
`rec(l, ws)`=`encode_wal_record(l, ws)`, `cp(g, s)`=`encode_checkpoint(g, s)`, `apply`=`apply_op`.

**`crates/cc-trace/tests/trace.rs`**
| ID | Senaryo | Beklenen |
|----|---------|----------|
| T01 | log'lu DirVfs: open(a,CreateNew), write_at(0,"xy"), set_len(1), fsync, fdatasync, sync_dir, rename(a,b), unlink(b) | olaylar birebir: Open{a,F0,true}, Write{F0,0,"xy"}, SetLen{F0,1}, Fsync{F0}, Fdatasync{F0}, SyncDir, Rename{a,b}, Unlink{b} |
| T02 | read_all, list, close, write_at boş veri | olay sayısı değişmez; read_all doğru bayt; list sıralı |
| T03 | open(a,CreateNew)=F0, open(b,CreateNew)=F1, close(F0), rename(a,c), open(c,Existing) | F0, F1, son açılış F0 ve `Open{c,F0,false}` |
| T04 | open(a,CreateNew) iki kez; rename(yok,x); open(yok,Existing) | AlreadyExists / NotFound…; başarısızlarda olay EKLENMEZ |
| T05 | ad "", "a/b", ".", ".." ; kapalı FileId'ye write_at; açık ada Existing | InvalidInput; olay yok |
| T06 | log'lu `DirVfs::new` dolu dizinde; aynı log ile ikinci `new` | DirectoryNotEmpty; InvalidInput |
| T07 | gerçek etki: write_at/set_len/rename/unlink sonrası `std::fs` ile okuma | içerik/adlar beklenen |
| T08 | elle Trace: 6 olay, spans [{2,Some 4},{4,Some 4},{4,Some 6}]; crash_point(0,2,3,4,5,6); açık span [{1,None}] ile crash_point(1),(2) | (0,f),(0,f),(0,t),(2,f),(2,t),(3,f); (0,f),(0,t) |
| T09 | begin_op(1) ilk çağrı; end_op(0) açık span yokken; crash_point(len+1) | üçü de panic (ayrı fonksiyonlar) |

**`crates/cc-adapter/tests/apply.rs`** (test içi `MapAdapter`: BTreeMap; her yazmada `log.push(SyncDir)`;
`fail_on: Option<usize>` çağrı sırası → `Io` hatası; guarantees OnAck/true/true)
| ID | Senaryo | Beklenen |
|----|---------|----------|
| A01 | apply P(a,1), G(a), T[wput(b,2),wdel(a)], S, G(a) | outcomes [Acked, Read{some 1}, Acked, Acked, Read{None}]; `check_reads` Allowed; scan {b:2} |
| A02 | fail_on=1: apply P, P, P | ikinci Failed{reason içerir}; üçüncü `Err(Closed)` ve adaptör çağrılmaz, span eklenmez |
| A03 | A01 log'lu | spans [{0,1},{1,1},{1,3},{3,3},{3,3}] (Some'larla); crash_point(2) = (2, true) |

**`engines/toy-correct/tests/format.rs`**
| ID | Senaryo | Beklenen |
|----|---------|----------|
| Y01 | crc32("123456789"), crc32("") | 0xCBF43926, 0 |
| Y02 | rec(1,[P a 1]) ‖ rec(2,[D a]) ‖ rec(3,[P b 2, D c]) ‖ rec(4,[]) decode | 4 kayıt birebir, CleanEnd, valid_len=toplam |
| Y03 | 2 geçerli kayıt + sırayla: 10 çöp bayt; 16 sıfır; rec(5,..); başlık len=MAX+1; rec(3,..)'in son 3 baytı eksik (başlık tam — kısa kayıtta "yarısı" < 16 bayt TornHeader verir); rec(3,..) payload baytı çevrilmiş | TornHeader; LsnMismatch{3,0}; LsnMismatch{3,5}; TooLong; Truncated; BadCrc — hepsinde records=2, valid_len=ilk iki kaydın uzunluğu |
| Y04 | cp(7,{a:1,b:2}) gidiş-dönüş; bayt çevir; sona 1 bayt ekle; 27 bayt; `parse_wal_file_name` ("wal-0000000000000001"→1, "wal-1"→None, büyük harf hex→None) | (7, durum); Err(Corruption)×3; isimler beklenen |

**`engines/toy-correct/tests/engine.rs`** (log'lu açılış boş dizinde; aksi belirtilmedikçe `checkpoint_every: 0`)
| ID | Senaryo | Beklenen |
|----|---------|----------|
| Y05 | her iki politika: apply P(a,1), P(b,2), D(a), T[wput(c,3),wdel(b)], G(c), G(a), S | scan {c:3}; reads Allowed; `guarantees` = OnAck/OnSync, true, true |
| Y06 | EveryCommit açılış + P(a,1): tam olay listesi | Open{W0001,F0,true}, SyncDir, Open{TMP,F1,true}, Write{F1,0,cp(1,{})}, Fsync{F1}, Rename{TMP,CP}, SyncDir, Write{F0,0,rec(1,[P a 1])}, Fdatasync{F0}; spans [{7,Some 9}] |
| Y07 | OnSyncOnly: P(a,1), G(a), T[] , S | açılıştan sonra: Write{F0,0,..}, Fdatasync{F0}; G ve boş T span'leri olaysız; Fdatasync yalnız S span'inde |
| Y08 | T[wput(a,1),wput(b,2),wdel(a)] | span içinde tek Write, data = rec(1, o üç yazma) |
| Y09 | checkpoint_every=2, EveryCommit: P(a,1), P(b,2), P(c,3) | P(b) span'i: Write{F0},Fdatasync{F0}, Open{wal-2,F2,true}, SyncDir, Open{TMP,F3,true}, Write{F3,0,cp(2,{a,b})}, Fsync{F3}, Rename{TMP,CP}, SyncDir, Unlink{W0001}; P(c): Write{F2,0,rec(1,[P c 3])}, Fdatasync{F2} |
| Y10 | P(a,1), P(b,2), D(b); close; log'suz yeniden aç | scan {a:1}; dizin [CP, wal-2]; CP nesli 2 |
| Y11 | checkpoint_every=3, 10 Put farklı anahtar; close; yeniden aç | scan 10 anahtar; dizinde tek wal |
| Y12 | P(a,1), D(a) close/aç; sonra checkpoint_every=1 ile P(b,1), D(b) close/aç | iki açılışta da silinen anahtar YOK (tombstone replay + checkpoint) |
| Y13 | P(a,1), P(b,2) close; wal'ın sonuna rec(3,[P c 3])'ün ilk yarısını ekle; aç | {a:1,b:2}; eski wal silinmiş, yeni nesil açılmış |
| Y14 | P a,b,c close; rec(2)'nin crc baytını boz (rec(3) geçerli kalır); aç | {a:1} (prefix: geçerli c uygulanmaz) |
| Y15 | **Eski-geçerli-kayıt:** Y14'ten sonra P(d,4) (değer uzunluğu b'ninkiyle aynı, kayıt boyları eşit), close, aç | {a:1,d:4}; c ASLA görünmez; P(d) yeni nesil WAL'a yazılmıştır |
| Y16 | wal'ın sonuna rec(5,[P z 9]) (LSN atlaması, CRC geçerli) ekle; aç | z yok |
| Y17 | CP baytı bozuk; CP var ama wal-g yok; dizinde "junk" dosyası | üçünde `open` → Err(Corruption) |
| Y18 | GC: geçerli kapanıştan sonra TMP (çöp içerik) ve wal-0000000000000009 ekle; aç | açılır, durum korunur, ikisi silinmiş |
| Y19 | bootstrap çökmesi: dizinde yalnız W0001 (boş) ve yarım TMP; aç | Ok, scan {}; dizin [CP, W0002 değil W0001]* |
| Y20 | 2^24+1 baytlık değerle put | Err(Io InvalidInput); iz ve durum değişmez |
| Y21 | apply_op ile 5 karışık işlem | spans.len()==5; Get span'leri start==end; crash_point(events.len())==(5,false) |
*Y19: R2 tüm tanınan dosyaları siler, g=0'dan bootstrap → dizin [CP, W0001], CP nesli 1.

**`engines/toy-correct/tests/seeds.rs`** — `common::gen_ops(seed)`: SplitMix64 (elle, Aşama 1 testindekiyle
aynı sabitler), işlem sayısı 20 + r%100; anahtar 8'li havuzdan (`k0`..`k7`), değer 0–16 bayt; dağılım Put %40,
Delete %15, Get %25, Txn %15 (0–4 yazma), Sync %5. `run_seed(seed, cfg, dir)` log'lu çalışır.
| ID | Senaryo | Beklenen |
|----|---------|----------|
| Y22 | seed 0..1000, iki politika × checkpoint_every ∈ {0, 7} (seed%4 ile seçilir) | her seed: `check_reads` Allowed; scan == `state_at(Cut::at_op(n))`; close + log'suz aç → scan aynı |
| Y23 | seed 0..20 iki farklı dizinde iki kez | `Trace` ve `History` eşit (determinizm) |
| Y24 | seed 0..50 iz yapısı | EveryCommit'te wal'a her Write'ı hemen aynı FileId Fdatasync izler; her Rename{TMP,CP}'den önce TMP'nin FileId'sine Fsync, sonra (Unlink'ten önce) SyncDir; her Unlink(wal) bir Rename+SyncDir'den sonra; her `Open{wal-*,created}`'ı bir SyncDir izler ve o SyncDir Rename'den önce gelir |
| Y25 | seed 0..50 | spans.len()==history.len(); crash_point(0)==(0,false) |

Toplam 9 + 3 + 21 + 4 = **37 senaryo**.

### 6.2 "Bitti" ölçütü (proje kökünde)
1. `cargo fmt --all -- --check` → 0. 2. `cargo clippy --workspace --all-targets --locked -- -D warnings` → 0 uyarı.
3. `cargo test --workspace --locked` → Aşama 1 testleri + §6.1'in tamamı geçer, 0 ignored.
4. `cargo tree --workspace --edges normal --prefix none | sort -u` → yalnız `cc-model`, `cc-trace`, `cc-adapter`,
   `toy-correct` satırları (dış crate yok). `cargo tree -p cc-trace --edges normal` → yalnız cc-trace.
5. `grep -rn "unsafe\|HashMap\|HashSet" crates/cc-trace/src crates/cc-adapter/src engines/toy-correct/src` → boş.
6. `grep -rn "sync_all\|sync_data" crates/cc-trace/src` → boş (karar 7).
7. `grep -c "MUTASYON NOKTASI" engines/toy-correct/src/*.rs` toplamı ≥ 6.
8. Öğretici yorumlar: cc-trace crate doc (karar 1: kill yerine kaydet-oynat; FileId/yol ayrımı; neden okuma ve
   başarısız çağrı kaydedilmez; neden gerçek fsync yok); toy-correct crate doc'unda §4.3 tablosu; §4.4 eski-kayıt
   gerekçesi `format.rs`/`db.rs`'de. CI dosyası değişmez (workspace geneli koşar).

## 7. Açık Sorular (cc-model kontratı DEĞİŞTİRİLMEDİ)
1. **Kurtarma reddi hükmü:** M5 (rename öncesi fsync yok) bozuk CHECKPOINT → `open` = `Err(Corruption)` üretir;
   cc-model'de "motor açılamadı" ihlal sınıfı yok. Öneri (Aşama 4 SPEC'i): eklemeli `ViolationKind::RecoveryFailed`
   (alanlar: `key = vec![]`, `found = None`). Alternatif "boş durum say" teşhisi bulandırır.
2. **Aşama 3'e devredilen FS varsayımları** (Aşama 3 SPEC'i sabitler): `Fdatasync` = içerik + boyut kalıcı
   (fdatasync(2) man sayfası); `Rename` atomik; `Open{created}`/`Rename`/`Unlink` yalnız `SyncDir` sonrası kesin
   kalıcı; FileId içeriği ada bağlı değil inode'a bağlı. Aşama 7'de strace ölçümüyle karşılaştırılır.
3. Düz ad uzayı ve "boş dizinde başla" Aşama 7 gerçek motoru için yetmeyebilir (alt dizin, önceden var olan
   dosya) → orada `Mkdir` vb. eklenir; Aşama 6 JSON şemasından önce yapılmalı.
4. PLAN'daki `MemVfs` kaldırıldı (karar 8); PLAN Aşama 2 metni güncellenmeli (mimar).

## 8. Dosya Sahipliği
| Dosya / Dizin | Sahip |
|---------------|-------|
| kök `Cargo.toml` (members), `Cargo.lock` | general-implementer |
| `crates/cc-trace/{Cargo.toml,src/**}`, `crates/cc-adapter/{Cargo.toml,src/**}` | general-implementer |
| `engines/toy-correct/{Cargo.toml,src/**}` | general-implementer |
| `crates/cc-trace/tests/**`, `crates/cc-adapter/tests/**`, `engines/toy-correct/tests/**` | test-engineer |
| `docs/**`, `crates/cc-model/**`, `.github/**` | DOKUNULMAZ (mimar / Aşama 1) |

## Görev Dağılımı (orchestrator buna göre delege eder)
| Alt Görev | Sorumlu Uygulayıcı (sonnet) | Dosya Kapsamı | Bağımlılık |
|-----------|------------------------------|----------------|------------|
| `cc-trace` (olay, TraceLog, Vfs, DirVfs) + workspace members | general-implementer | `Cargo.toml`, `crates/cc-trace/` (tests hariç) | Aşama 1 |
| `cc-adapter` (trait, AdapterError, apply_op) | general-implementer (aynı, sıralı) | `crates/cc-adapter/` (tests hariç) | cc-trace |
| `toy-correct` (crc32, format, db, mutasyon noktası yorumları) | general-implementer (aynı, sıralı) | `engines/toy-correct/` (tests hariç) | cc-adapter |
| §6.1 testleri (37 senaryo + test yardımcıları) | test-engineer | tüm `tests/**` | yalnız §2–§4 public API (kütüphaneyle paralel yazılır, koşum sonra) |

Not: Üç crate tek Rust kütüphane zinciri (trace → adapter → toy); tip imzaları birbirine bağlı olduğundan tek
uygulayıcı sıralı çalışır. Testler SPEC'ten bağımsız yazılsın diye test-engineer'da.

## Doğrulama Planı
| Ne Doğrulanacak | Doğrulayıcı Ajan | Yöntem |
|------------------|------------------|--------|
| §6.1 37 senaryo, 1.000 seed (Y22), determinizm (Y23) | test-engineer | `cargo test --workspace --locked` |
| §6.2 madde 1, 2, 4–7 | test-engineer | komutları koş, çıktıyı raporla |
| Y06/Y09 olay listelerinin §4.3 tablosuyla elle karşılaştırılması | test-engineer | test kodu ile SPEC satır satır |

ui-simulator (web yok) ve sim-runner (araç/robot yok) uygulanamaz.

## Denetim Planı
| Alan | Denetçi | Neden |
|------|---------|-------|
| SPEC uyumu, fsync düzeni ↔ öğretici yorumlar, determinizm, eski-kayıt çözümü, mutasyon noktaları | code-reviewer | her zaman |
| — | security-auditor | Gerekmez: `unsafe` yok, ağ/kimlik yok; yol kaçışı §2.1 ad doğrulamasıyla kapalı (code-reviewer kontrol eder). Aşama 7'de zorunlu. |
