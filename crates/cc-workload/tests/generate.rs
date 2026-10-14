//! cc-workload testleri: determinizm, sınırlar, dağılım.

use cc_model::{Op, Write};
use cc_workload::{OpWeights, SplitMix64, WorkloadConfig, generate};

/// SplitMix64 bilinen ilk çıktılar (seed 0) — algoritma sabitlerinin değişmediğini kilitler.
#[test]
fn splitmix64_reference_values() {
    let mut r = SplitMix64(0);
    assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
    assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
}

/// Aynı seed → aynı iş yükü; farklı seed → (pratikte) farklı.
#[test]
fn deterministic() {
    let cfg = WorkloadConfig::default();
    for seed in 0..50 {
        assert_eq!(generate(seed, &cfg), generate(seed, &cfg));
    }
    assert_ne!(generate(1, &cfg), generate(2, &cfg));
}

/// Uzunluk, anahtar havuzu, değer uzunluğu ve txn boyutu sınırları.
#[test]
fn respects_bounds() {
    let cfg = WorkloadConfig {
        min_ops: 5,
        extra_ops: 10,
        key_space: 3,
        max_value_len: 4,
        max_txn_writes: 2,
        weights: OpWeights::default(),
    };
    let key_ok = |k: &[u8]| [b"k0".as_slice(), b"k1", b"k2"].contains(&k);
    for seed in 0..500 {
        let ops = generate(seed, &cfg);
        assert!((5..15).contains(&ops.len()), "seed {seed}: {}", ops.len());
        for op in &ops {
            match op {
                Op::Put { key, value } => assert!(key_ok(key) && value.len() <= 4),
                Op::Delete { key } | Op::Get { key } => assert!(key_ok(key)),
                Op::Txn { writes } => {
                    assert!(writes.len() <= 2);
                    for w in writes {
                        match w {
                            Write::Put { key, value } => assert!(key_ok(key) && value.len() <= 4),
                            Write::Delete { key } => assert!(key_ok(key)),
                        }
                    }
                }
                Op::Sync => {}
            }
        }
    }
    let fixed = WorkloadConfig {
        extra_ops: 0,
        ..cfg
    };
    assert_eq!(generate(9, &fixed).len(), 5);
}

/// Dağılım ağırlıklara ±%3 içinde uyar (yaklaşık 60k işlem).
#[test]
fn distribution_matches_weights() {
    let cfg = WorkloadConfig::default();
    let mut counts = [0usize; 5];
    for seed in 0..1000 {
        for op in generate(seed, &cfg) {
            let i = match op {
                Op::Put { .. } => 0,
                Op::Delete { .. } => 1,
                Op::Get { .. } => 2,
                Op::Txn { .. } => 3,
                Op::Sync => 4,
            };
            counts[i] += 1;
        }
    }
    let total: usize = counts.iter().sum();
    for (c, w) in counts.iter().zip([40.0, 15.0, 25.0, 15.0, 5.0]) {
        let pct = *c as f64 * 100.0 / total as f64;
        assert!(
            (pct - w).abs() < 3.0,
            "beklenen %{w}, bulunan %{pct:.1} ({counts:?})"
        );
    }
}
