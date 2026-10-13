//! Test yardımcıları: geçici dizin, seed'li kayıtlı koşu.
// Her test dosyası her yardımcıyı kullanmaz; dead_code uyarısı -D warnings ile CI'ı kırar.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use cc_adapter::{StorageAdapter, apply_op};
use cc_model::History;
use cc_trace::{Trace, TraceLog};
use cc_workload::{WorkloadConfig, generate};
use toy_correct::{ToyConfig, ToyDb};

/// `CARGO_TARGET_TMPDIR/toy-correct/<ad>`: varsa silinir, boş olarak yeniden yaratılır.
pub fn fresh_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("toy-correct")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("test dizini yaratılamadı");
    dir
}

pub fn b(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

/// Kayıtlı koşu: boş `dir`'de aç, `cc_workload::generate(seed, varsayılan)`'ı uygula.
pub fn run_seed(seed: u64, cfg: &ToyConfig, dir: &Path) -> (ToyDb, History, Trace) {
    let log = TraceLog::new();
    let mut db = ToyDb::open(dir, cfg, Some(log.clone())).expect("açılış");
    let mut h = History::new();
    for op in generate(seed, &WorkloadConfig::default()) {
        apply_op(&mut db, &mut h, Some(&log), op).expect("issue");
    }
    (db, h, log.snapshot())
}
