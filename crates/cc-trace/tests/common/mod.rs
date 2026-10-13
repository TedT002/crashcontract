//! Test yardımcıları.
// Her test dosyası her yardımcıyı kullanmaz; dead_code uyarısı -D warnings ile CI'ı kırar.
#![allow(dead_code)]

use std::path::PathBuf;

/// `CARGO_TARGET_TMPDIR/<crate>/<ad>`: varsa silinir, boş olarak yeniden yaratılır.
/// (`tempfile` crate'i yerine: bağımlılık minimum.)
pub fn fresh_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("cc-trace")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("test dizini yaratılamadı");
    dir
}
