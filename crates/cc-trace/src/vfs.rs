//! Toy motorların dosya sistemine tek erişim yolu.

use std::io;

use crate::FileId;

/// Açma kipi. Her iki kip de okuma+yazma açar; O_TRUNC yoktur (kısaltma yalnız `set_len` ile,
/// böylece kayıtlıdır — gizli bir kısaltma izde görünmeyen bir durum değişikliği olurdu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// O_CREAT|O_EXCL: ad varsa `AlreadyExists`.
    CreateNew,
    /// Var olan dosya: yoksa `NotFound`.
    Existing,
}

/// Toy motorların dosya sistemine TEK erişim yolu (nesne-güvenli; motor `Box<dyn Vfs>` tutar).
///
/// Motor `std::fs`'e doğrudan dokunmaz: dokunsaydı o çağrılar izde görünmez, çökme durumu
/// üreteci (Aşama 3) onları hesaba katamazdı.
pub trait Vfs {
    /// Kökteki `name` dosyasını açar.
    fn open(&mut self, name: &str, mode: OpenMode) -> io::Result<FileId>;
    /// Tanıtıcıyı kapatır (kaydedilmez; close dayanıklılık sağlamaz).
    fn close(&mut self, file: FileId) -> io::Result<()>;
    /// Dosyanın tamamını okur (kaydedilmez).
    fn read_all(&mut self, file: FileId) -> io::Result<Vec<u8>>;
    /// `data`'yı `offset`'e yazar. Boş `data` → olay yok, Ok.
    fn write_at(&mut self, file: FileId, offset: u64, data: &[u8]) -> io::Result<()>;
    /// ftruncate.
    fn set_len(&mut self, file: FileId, len: u64) -> io::Result<()>;
    /// fsync(fd).
    fn fsync(&mut self, file: FileId) -> io::Result<()>;
    /// fdatasync(fd).
    fn fdatasync(&mut self, file: FileId) -> io::Result<()>;
    /// rename(2).
    fn rename(&mut self, from: &str, to: &str) -> io::Result<()>;
    /// unlink(2).
    fn unlink(&mut self, name: &str) -> io::Result<()>;
    /// Kök dizinin fsync'i.
    fn sync_dir(&mut self) -> io::Result<()>;
    /// Kökteki adlar, bayt sırasıyla (kaydedilmez).
    fn list(&mut self) -> io::Result<Vec<String>>;
}
