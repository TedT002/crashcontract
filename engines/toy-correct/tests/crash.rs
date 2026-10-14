//! Duman testi: seed'li koşuların HER kesme noktasındaki HER çökme
//! durumunda `toy-correct` izinli bir durum kurtarmalı. Doğru motorda tek bir ihlal ya da açılış
//! hatası, üreticinin (ya da modelin) aşırı muhafazakâr olduğunu gösterir → yanlış alarm.

mod common;

use std::path::Path;

use cc_adapter::StorageAdapter;
use cc_model::{Verdict, check_recovered};
use cc_trace::{CrashConfig, crash_states, materialize};
use common::{fresh_dir, run_seed};
use toy_correct::{SyncPolicy, ToyConfig, ToyDb};

/// Denenen durum sayıları.
#[derive(Debug, Default)]
struct Tried {
    all: usize,
    /// Yırtık yazmalı durumlar.
    torn: usize,
    /// Yırtık kaydın ARDINDAN gelen bir bekleyen işlemin de kalıcı olduğu durumlar (yalnız
    /// checksum'ın reddedebildiği "ortada yırtık kayıt").
    torn_mid: usize,
}

/// Bir seed'in tüm kesme × durum kombinasyonlarını dener.
fn check_seed(seed: u64, cfg: &ToyConfig, crash_cfg: &CrashConfig, scratch: &Path) -> Tried {
    let (_, history, trace) = run_seed(seed, cfg, &fresh_dir(&format!("crash-run-{seed}")));
    let guarantees = ToyDb::guarantees(cfg);
    let mut tried = Tried::default();
    for cut in 0..=trace.events.len() {
        let cp = trace.crash_point(cut);
        let view = history.crash_view(cp.completed, cp.in_flight);
        for state in crash_states(&trace.events, cut, seed, crash_cfg).expect("geçerli iz") {
            let _ = std::fs::remove_dir_all(scratch);
            std::fs::create_dir_all(scratch).unwrap();
            materialize(&state.image, scratch).unwrap();
            let ctx = || format!("seed {seed} {cfg:?} kesme {cut} seçim {:?}", state.choice);
            let mut db = ToyDb::open(scratch, cfg, None)
                .unwrap_or_else(|e| panic!("açılış hatası (yanlış alarm): {e} — {}", ctx()));
            let recovered = db.scan().unwrap();
            let verdict = check_recovered(&view, guarantees, &recovered);
            assert!(
                matches!(verdict, Verdict::Allowed { .. }),
                "yanlış alarm: {verdict:?} — {}",
                ctx()
            );
            tried.all += 1;
            if let Some(t) = &state.choice.torn {
                tried.torn += 1;
                if state.choice.persisted.iter().any(|&i| i > t.event) {
                    tried.torn_mid += 1;
                }
            }
        }
    }
    tried
}

/// Her iki politika × checkpoint açık/kapalı, yırtık yazmasız.
#[test]
fn toy_correct_survives_every_crash_state() {
    let scratch = fresh_dir("crash-scratch");
    let mut total = 0;
    for seed in 0..12u64 {
        let cfg = ToyConfig {
            sync_policy: if seed.is_multiple_of(2) {
                SyncPolicy::EveryCommit
            } else {
                SyncPolicy::OnSyncOnly
            },
            checkpoint_every: if (seed / 2).is_multiple_of(2) { 0 } else { 5 },
            mutation: None,
        };
        total += check_seed(seed, &cfg, &CrashConfig::default(), &scratch).all;
    }
    eprintln!("denenen çökme durumu: {total}");
    assert!(total > 1000, "çok az durum denendi: {total}");
}

/// Yırtık yazmalar açık: CRC yırtık kaydı reddetmeli, replay orada durmalı.
///
/// Otomatik checkpoint KAPALI: WAL büyüsün ve kayıtlar sektör sınırlarını aşsın — aksi halde hiç
/// yırtılabilir yazma oluşmaz ve test boş yere geçer (ölçüldü: checkpoint_every=5 ile 0 yırtık durum).
/// 512 B'nin yanında 64 B sektörle de koşulur (daha sık yırtılma). Test, yırtık ve "ortada yırtık"
/// durumların gerçekten üretildiğini de doğrular.
#[test]
fn toy_correct_survives_torn_writes() {
    let scratch = fresh_dir("crash-scratch-torn");
    let mut total = Tried::default();
    for sector_size in [512, 64] {
        let crash_cfg = CrashConfig {
            torn_writes: true,
            sector_size,
            ..CrashConfig::default()
        };
        for seed in 100..106u64 {
            let cfg = ToyConfig {
                sync_policy: if seed.is_multiple_of(2) {
                    SyncPolicy::EveryCommit
                } else {
                    SyncPolicy::OnSyncOnly
                },
                checkpoint_every: 0,
                mutation: None,
            };
            let t = check_seed(seed, &cfg, &crash_cfg, &scratch);
            total.all += t.all;
            total.torn += t.torn;
            total.torn_mid += t.torn_mid;
        }
    }
    eprintln!("denenen çökme durumu (yırtık geçişi): {total:?}");
    assert!(total.torn > 100, "çok az yırtık durum: {total:?}");
    assert!(
        total.torn_mid > 20,
        "ortada yırtık kayıt durumu üretilmedi: {total:?}"
    );
}
