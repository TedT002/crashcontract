//! # toy-correct — doğru çalışan referans motor
//!
//! WAL + checkpoint'li küçük bir KV deposu. Amacı hızlı olmak değil, **doğru** olmak ve her
//! dayanıklılık kararını açıkça göstermek: mutantlar (`toy-mutants`) bu motordan tek bir kararı
//! bozarak türetilir ve araç onları yakalamak zorundadır.
//!
//! Motor dosya sistemine yalnız [`cc_trace::Vfs`] üzerinden erişir; böylece her çağrı ize girer.
//!
//! ## fsync düzeni
//!
//! | # | Nokta | Çağrı dizisi | Neden |
//! |---|-------|--------------|-------|
//! | W1 | put/delete/txn, `EveryCommit` | `write_at(wal)` → `fdatasync(wal)` → belleğe uygula → ACK | **fsync sınırı:** ACK ≥ dayanıklılık (`OnAck`) |
//! | W2 | put/delete/txn, `OnSyncOnly` | `write_at(wal)` → belleğe uygula → ACK | ACK ≠ dayanıklılık; kayıt sayfa önbelleğinde |
//! | W3 | `sync()` | `fdatasync(wal)` | grup commit bariyeri |
//! | C1 | checkpoint g→g+1 | `open(wal-(g+1), CreateNew)` → `sync_dir` | yeni WAL'ın adı, ona referans veren CHECKPOINT'ten önce kalıcı |
//! | C2 | | `open(TMP)` → `write_at(TMP)` → `fsync(TMP)` | **rename öncesi fsync:** yoksa rename kalıcı olup içerik olmayabilir |
//! | C3 | | `rename(TMP, CHECKPOINT)` → `sync_dir` | **atomik değiştirme;** rename yalnız dizin fsync'iyle kalıcı |
//! | C4 | | `unlink(wal-g)` (dizin fsync'i yok) | eski WAL yalnız yeni CHECKPOINT kalıcıyken silinir; kaybolursa açılışta GC |
//!
//! ## Kurtarma
//!
//! CHECKPOINT okunur, nesli ≠ g olan WAL'lar ve TMP silinir, `wal-g` **ilk bozuk kayıtta durarak**
//! replay edilir, ardından hemen yeni bir nesle checkpoint yapılır: kurtarılan WAL'a asla ekleme
//! yapılmaz (eski-geçerli-kayıt problemi, bkz. [`format`]).
//!
//! ## Doğrulanmayanlar
//!
//! [`cc_trace::DirVfs`] gerçek `fsync` çağırmaz; bu motor gerçek diskte dayanıklı **değildir**.
//! Doğruluk iddiası yalnız iz üzerinden üretilen çökme durumları içindir.

mod crc32;
mod db;
pub mod format;
mod mutation;

pub use db::{SyncPolicy, ToyConfig, ToyDb};
pub use mutation::Mutation;
