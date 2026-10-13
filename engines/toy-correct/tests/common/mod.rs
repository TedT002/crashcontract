//! Test yardımcıları: geçici dizin, deterministik PRNG, seed'li iş yükü.
// Her test dosyası her yardımcıyı kullanmaz; dead_code uyarısı -D warnings ile CI'ı kırar.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use cc_adapter::{StorageAdapter, apply_op};
use cc_model::{History, Op, Write};
use cc_trace::{Trace, TraceLog};
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

/// SplitMix64 — cc-model kaba kuvvet testindekiyle aynı sabitler. `rand` yerine: algoritması
/// sürümler arası sabit olmalı (seed → iş yükü eşlemesi raporların replay'i için kontrattır).
pub struct SplitMix64(pub u64);
impl SplitMix64 {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

pub fn b(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

fn rand_key(r: &mut SplitMix64) -> Vec<u8> {
    format!("k{}", r.below(8)).into_bytes()
}

fn rand_value(r: &mut SplitMix64) -> Vec<u8> {
    let len = r.below(17) as usize;
    (0..len).map(|_| r.below(256) as u8).collect()
}

/// Seed'li iş yükü: 20 + r%100 işlem; Put %40, Delete %15, Get %25, Txn %15 (0–4 yazma), Sync %5.
pub fn gen_ops(seed: u64) -> Vec<Op> {
    let mut r = SplitMix64(seed);
    let n = 20 + r.below(100);
    (0..n)
        .map(|_| match r.below(100) {
            0..40 => Op::Put {
                key: rand_key(&mut r),
                value: rand_value(&mut r),
            },
            40..55 => Op::Delete {
                key: rand_key(&mut r),
            },
            55..80 => Op::Get {
                key: rand_key(&mut r),
            },
            80..95 => {
                let k = r.below(5);
                let writes = (0..k)
                    .map(|_| {
                        if r.below(3) == 0 {
                            Write::Delete {
                                key: rand_key(&mut r),
                            }
                        } else {
                            Write::Put {
                                key: rand_key(&mut r),
                                value: rand_value(&mut r),
                            }
                        }
                    })
                    .collect();
                Op::Txn { writes }
            }
            _ => Op::Sync,
        })
        .collect()
}

/// Kayıtlı koşu: boş `dir`'de aç, `gen_ops(seed)`'i uygula.
pub fn run_seed(seed: u64, cfg: &ToyConfig, dir: &Path) -> (ToyDb, History, Trace) {
    let log = TraceLog::new();
    let mut db = ToyDb::open(dir, cfg, Some(log.clone())).expect("açılış");
    let mut h = History::new();
    for op in gen_ops(seed) {
        apply_op(&mut db, &mut h, Some(&log), op).expect("issue");
    }
    (db, h, log.snapshot())
}
