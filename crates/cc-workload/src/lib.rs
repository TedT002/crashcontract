//! # cc-workload — seed'li iş yükü üreteci
//!
//! Tek iş parçacıklı, sıralı bir işlem dizisi üretir (CLAUDE.md karar 5). Aynı seed + aynı
//! yapılandırma her zaman aynı diziyi verir; bir hata raporu yalnız seed'i saklayarak iş yükünü
//! yeniden üretebilir.
//!
//! ## Neden `rand` değil de elle SplitMix64?
//!
//! `rand`'ın `StdRng` algoritması sürümler arası garanti edilmez; değişirse eski raporlar aynı iş
//! yükünü üretemez. SplitMix64 birkaç satırdır ve sabit kalır. Dağılım ya da üretim sırası
//! değişirse rapor şeması sürümü artırılmalıdır (Aşama 6).
//!
//! ## Neden küçük anahtar uzayı?
//!
//! Varsayılan 8 anahtar: aynı anahtara üst üste yazma, silip yeniden yazma ve txn içinde çakışma
//! sık olsun. Çökme hatalarının çoğu tam bu etkileşimlerde (eski değerin geri gelmesi, tombstone
//! kaybı) ortaya çıkar; geniş anahtar uzayında bu durumlar nadirleşir.

use cc_model::{Op, Write};

/// SplitMix64 PRNG (Steele, Lea, Flood 2014). Sabitler değişmez — seed eşlemesi kontrattır.
#[derive(Debug, Clone)]
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    /// Sonraki 64 bit.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `[0, n)` aralığında sayı (`n > 0`). Mod yanlılığı küçük `n` için ihmal edilebilir; test
    /// iş yükünde istatistiksel kusursuzluk değil determinizm önemlidir.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0)");
        self.next_u64() % n
    }
}

/// İşlem türlerinin göreli ağırlıkları.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpWeights {
    /// Put.
    pub put: u32,
    /// Delete.
    pub delete: u32,
    /// Get.
    pub get: u32,
    /// Txn.
    pub txn: u32,
    /// Sync.
    pub sync: u32,
}

impl Default for OpWeights {
    fn default() -> Self {
        OpWeights {
            put: 40,
            delete: 15,
            get: 25,
            txn: 15,
            sync: 5,
        }
    }
}

/// İş yükü yapılandırması.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkloadConfig {
    /// En az işlem sayısı.
    pub min_ops: u32,
    /// İşlem sayısı `min_ops + [0, extra_ops)`; 0 ise tam `min_ops`.
    pub extra_ops: u32,
    /// Anahtar havuzu: `k0` … `k{key_space-1}`.
    pub key_space: u32,
    /// Değer uzunluğu `[0, max_value_len]` bayt.
    pub max_value_len: u32,
    /// Txn başına yazma sayısı `[0, max_txn_writes]`.
    pub max_txn_writes: u32,
    /// İşlem türü ağırlıkları.
    pub weights: OpWeights,
}

impl Default for WorkloadConfig {
    fn default() -> Self {
        WorkloadConfig {
            min_ops: 20,
            extra_ops: 100,
            key_space: 8,
            max_value_len: 16,
            max_txn_writes: 4,
            weights: OpWeights::default(),
        }
    }
}

fn key(r: &mut SplitMix64, cfg: &WorkloadConfig) -> Vec<u8> {
    format!("k{}", r.below(u64::from(cfg.key_space.max(1)))).into_bytes()
}

fn value(r: &mut SplitMix64, cfg: &WorkloadConfig) -> Vec<u8> {
    let len = r.below(u64::from(cfg.max_value_len) + 1);
    (0..len).map(|_| r.below(256) as u8).collect()
}

/// Seed'li iş yükü. Saf fonksiyon: aynı girdi → aynı dizi.
///
/// # Panics
/// Tüm ağırlıklar 0 ise.
pub fn generate(seed: u64, cfg: &WorkloadConfig) -> Vec<Op> {
    let w = &cfg.weights;
    let total = u64::from(w.put + w.delete + w.get + w.txn + w.sync);
    assert!(total > 0, "en az bir işlem ağırlığı pozitif olmalı");
    let mut r = SplitMix64(seed);
    let extra = if cfg.extra_ops == 0 {
        0
    } else {
        r.below(u64::from(cfg.extra_ops))
    };
    let n = u64::from(cfg.min_ops) + extra;
    // Kümülatif eşikler: [put, +delete, +get, +txn, +sync).
    let t_put = u64::from(w.put);
    let t_del = t_put + u64::from(w.delete);
    let t_get = t_del + u64::from(w.get);
    let t_txn = t_get + u64::from(w.txn);
    (0..n)
        .map(|_| {
            let x = r.below(total);
            if x < t_put {
                Op::Put {
                    key: key(&mut r, cfg),
                    value: value(&mut r, cfg),
                }
            } else if x < t_del {
                Op::Delete {
                    key: key(&mut r, cfg),
                }
            } else if x < t_get {
                Op::Get {
                    key: key(&mut r, cfg),
                }
            } else if x < t_txn {
                let k = r.below(u64::from(cfg.max_txn_writes) + 1);
                let writes = (0..k)
                    .map(|_| {
                        // Txn içinde silme oranı ~1/3: tombstone + aynı txn'de yeniden yazma sık olsun.
                        if r.below(3) == 0 {
                            Write::Delete {
                                key: key(&mut r, cfg),
                            }
                        } else {
                            Write::Put {
                                key: key(&mut r, cfg),
                                value: value(&mut r, cfg),
                            }
                        }
                    })
                    .collect();
                Op::Txn { writes }
            } else {
                Op::Sync
            }
        })
        .collect()
}
