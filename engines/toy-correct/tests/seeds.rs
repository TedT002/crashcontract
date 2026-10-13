//! toy-correct seed'li testler (SPEC_stage2 §6.1 Y22–Y25): çökmesiz doğruluk, determinizm ve
//! iz yapısının fsync düzenine uyumu.

mod common;

use std::collections::BTreeMap;

use cc_adapter::StorageAdapter;
use cc_model::{Cut, Verdict, check_reads};
use cc_trace::{CrashPoint, FileId, Trace, TraceEvent as E};
use common::{fresh_dir, run_seed};
use toy_correct::format::{CHECKPOINT_FILE as CP, CHECKPOINT_TMP_FILE as TMP};
use toy_correct::{SyncPolicy, ToyConfig, ToyDb};

/// seed%4 → (politika, checkpoint_every) kombinasyonu.
fn cfg_for(seed: u64) -> ToyConfig {
    ToyConfig {
        sync_policy: if seed.is_multiple_of(2) {
            SyncPolicy::EveryCommit
        } else {
            SyncPolicy::OnSyncOnly
        },
        checkpoint_every: if (seed / 2).is_multiple_of(2) { 0 } else { 7 },
        mutation: None,
    }
}

/// Y22: 1.000 seed, çökme yok — okumalar modelle tutarlı, son durum tüm geçmişin uygulanmış hali,
/// temiz yeniden açılış aynı durumu verir.
#[test]
fn y22_thousand_seeds_without_crash() {
    for seed in 0..1000 {
        let c = cfg_for(seed);
        let dir = fresh_dir(&format!("y22-{}", seed % 8));
        let (mut db, h, _) = run_seed(seed, &c, &dir);
        assert_eq!(
            check_reads(&h),
            Verdict::Allowed { witness: None },
            "seed {seed}"
        );
        let expected = h.state_at(Cut::at_op(h.len()));
        assert_eq!(db.scan().unwrap(), expected, "seed {seed}");
        db.close().unwrap();
        let mut db = ToyDb::open(&dir, &c, None).unwrap();
        assert_eq!(db.scan().unwrap(), expected, "seed {seed} yeniden açılış");
    }
}

/// Y23: aynı seed + yapılandırma → aynı iz ve geçmiş (kök yolu ize girmez).
#[test]
fn y23_determinism() {
    for seed in 0..20 {
        let c = cfg_for(seed);
        let (_, h1, t1) = run_seed(seed, &c, &fresh_dir("y23-a"));
        let (_, h2, t2) = run_seed(seed, &c, &fresh_dir("y23-b"));
        assert_eq!(t1, t2, "seed {seed}");
        assert_eq!(h1, h2, "seed {seed}");
    }
}

/// İz yapısını §4.3 fsync düzenine göre denetler.
fn check_structure(t: &Trace, every_commit: bool, seed: u64) {
    let ev = &t.events;
    // FileId → son açıldığı ad.
    let mut names: BTreeMap<FileId, String> = BTreeMap::new();
    let mut last_rename: Option<usize> = None;
    let mut sync_after_rename = false;
    let mut pending_wal_open: Option<usize> = None;
    for (i, e) in ev.iter().enumerate() {
        match e {
            E::Open {
                name,
                file,
                created,
            } => {
                names.insert(*file, name.clone());
                if *created && name.starts_with("wal-") {
                    // Yeni WAL'ın adı bir sonraki Rename'den önce SyncDir ile sabitlenmeli.
                    assert_eq!(ev.get(i + 1), Some(&E::SyncDir), "seed {seed} olay {i}");
                    pending_wal_open = Some(i);
                }
            }
            E::Write { file, .. } if every_commit && names[file].starts_with("wal-") => {
                assert_eq!(
                    ev.get(i + 1),
                    Some(&E::Fdatasync { file: *file }),
                    "seed {seed} olay {i}"
                );
            }
            E::Rename { from, to } if from == TMP && to == CP => {
                assert!(
                    pending_wal_open.take().is_some(),
                    "seed {seed}: Rename'den önce yeni WAL yok"
                );
                // TMP'nin son Write'ı ile Rename arasında TMP'ye Fsync olmalı.
                let tmp = names
                    .iter()
                    .find(|(_, n)| *n == TMP)
                    .map(|(f, _)| *f)
                    .unwrap();
                let last_write = ev[..i]
                    .iter()
                    .rposition(|x| matches!(x, E::Write { file, .. } if *file == tmp))
                    .expect("TMP'ye yazılmadan rename");
                assert!(
                    ev[last_write..i].contains(&E::Fsync { file: tmp }),
                    "seed {seed}: rename öncesi fsync yok (olay {i})"
                );
                last_rename = Some(i);
                sync_after_rename = false;
            }
            E::SyncDir if last_rename.is_some() => sync_after_rename = true,
            E::Unlink { name } if name.starts_with("wal-") => {
                assert!(
                    last_rename.is_some() && sync_after_rename,
                    "seed {seed}: WAL, yeni CHECKPOINT kalıcı olmadan silindi (olay {i})"
                );
            }
            _ => {}
        }
    }
}

/// Y24: iz yapısı fsync düzenine uyuyor (50 seed, her iki politika).
#[test]
fn y24_trace_structure_follows_fsync_order() {
    for seed in 0..50 {
        let c = cfg_for(seed);
        let (_, _, t) = run_seed(seed, &c, &fresh_dir("y24"));
        check_structure(&t, c.sync_policy == SyncPolicy::EveryCommit, seed);
    }
}

/// Y25: span sayısı = geçmiş uzunluğu; hiçbir olay yokken hiçbir işlem gönderilmemiştir.
#[test]
fn y25_spans_match_history() {
    for seed in 0..50 {
        let (_, h, t) = run_seed(seed, &cfg_for(seed), &fresh_dir("y25"));
        assert_eq!(t.spans.len(), h.len(), "seed {seed}");
        assert_eq!(
            t.crash_point(0),
            CrashPoint {
                completed: 0,
                in_flight: false
            }
        );
    }
}
