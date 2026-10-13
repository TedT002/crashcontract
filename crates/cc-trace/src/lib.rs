//! # cc-trace — I/O izi ve kaydeden dosya sistemi katmanı
//!
//! ## Neden süreci öldürmek yerine kaydet-ve-oynat? (CLAUDE.md karar 1)
//!
//! Dışarıdan `kill` deterministik değildir; daha önemlisi süreç öldüğünde OS sayfa önbelleği
//! yazıları korur, yani `fsync`'i unutan bir motor `kill` testinden **temiz geçer**. Gerçek kayıp
//! ancak sistem çökmesinde / güç kaybında olur. Bu yüzden motorun dosya sistemi çağrılarını bir
//! **iz** olarak kaydediyoruz; Aşama 3 bu izden "fsync ile sabitlenmemiş yazılar düşmüş olabilir"
//! kuralıyla çökme durumlarını üretir. Aynı iz → aynı durumlar: yeniden üretilebilirlik buradan gelir.
//!
//! ## Neden veri olayları `FileId`, ad olayları yol bazlı?
//!
//! POSIX çökme semantiği tam bu ayrımı yapar: dosya **içeriği** `fsync(fd)` ile inode'a bağlı
//! olarak kalıcılaşır, **ad girdileri** (oluşturma, `rename`, `unlink`) ise dizinin `fsync`'iyle.
//! Saf yol bazlı bir iz, açık dosya yeniden adlandırıldığında yazıyı yanlış dosyaya bağlar; saf fd
//! bazlı iz ise fd numaraları yeniden kullanıldığı ve `rename`/`unlink` fd almadığı için yetmez.
//! [`FileId`] bu yüzden "inode kimliği"dir: aynı dosyanın her açılışı aynı kimliği alır.
//!
//! ## Neyi kaydetmiyoruz ve neden?
//!
//! * **Okuma, `list`, `close`:** dayanıklı durumu değiştirmezler. (`close` dayanıklılık sağlamaz!)
//! * **Başarısız çağrılar:** olay yalnız alttaki çağrı başarıyla dönünce eklenir; başarısız çağrının
//!   diskte etkisi olmadığı varsayılır. fsync hata semantiği ("fsyncgate") v1 kapsamı dışıdır.
//!
//! ## Neden [`DirVfs`] gerçek `fsync` çağırmaz?
//!
//! Çökme durumları işletim sisteminden değil izden üretilir; gerçek `fsync` araca doğruluk
//! katmaz, yalnız testleri yavaşlatır. Sonuç: toy motorlar gerçek diskte dayanıklı **değildir** —
//! bu, README'nin "Doğrulanmayanlar" bölümüne yazılır.

pub mod crash;
mod dir_vfs;
mod event;
mod vfs;

pub use crash::{
    Choice, CrashConfig, CrashError, CrashState, Image, TornWrite, build_image,
    build_image_with_sector, crash_states, materialize, pending_ops,
};
pub use dir_vfs::DirVfs;
pub use event::{CrashPoint, FileId, OpSpan, Trace, TraceEvent, TraceLog};
pub use vfs::{OpenMode, Vfs};
