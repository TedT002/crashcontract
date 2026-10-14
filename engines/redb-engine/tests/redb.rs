//! redb adaptörü testleri (Aşama 7). `libcc_shim.so` ve `redb-driver` derlenmiş olmalı:
//! `cargo build -p cc-shim -p redb-engine` (CI bunu testlerden önce yapar). Eksikse testler açık bir
//! mesajla başarısız olur — sessizce atlanmaz.

use std::path::PathBuf;

use cc_adapter::runner::{CutSelection, Finding, RunConfig, run_case};
use cc_adapter::{StorageAdapter, apply_op};
use cc_model::{History, Op, ViolationKind};
use cc_trace::{CrashConfig, FileId, TraceEvent, TraceLog};
use cc_workload::{WorkloadConfig, generate};
use redb_engine::{REDB_VERSION, RedbConfig, RedbProc};

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("redb")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rc(linux: bool, cuts: CutSelection, skip_open_phase: bool) -> RunConfig {
    RunConfig {
        crash: CrashConfig {
            fsync_persists_new_dirent: linux,
            ..CrashConfig::default()
        },
        cuts,
        skip_open_phase,
    }
}

/// Belgelenen sürüm, gerçekten derlenen sürümdür (Cargo.lock).
#[test]
fn redb_version_matches_lockfile() {
    let lock =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).unwrap();
    let needle = format!("name = \"redb\"\nversion = \"{REDB_VERSION}\"");
    assert!(
        lock.contains(&needle),
        "Cargo.lock redb sürümü {REDB_VERSION} değil"
    );
}

/// Shim'in kaydettiği iz, strace ölçümüyle aynı biçimde: yeni dosya, ön tahsis (setlen), konumlu
/// yazmalar, commit başına fdatasync — ve dizin fsync'i YOK (redb yapmıyor).
#[test]
fn shim_records_redb_io() {
    let d = dir("record");
    let log = TraceLog::new();
    let mut db = RedbProc::open(&d, &RedbConfig::default(), Some(log.clone()))
        .expect("kayıtlı açılış (shim ve sürücü derlendi mi?)");
    let mut h = History::new();
    let put = Op::Put {
        key: b"k".to_vec(),
        value: b"v".to_vec(),
    };
    apply_op(&mut db, &mut h, Some(&log), put).unwrap();
    db.close().unwrap();
    let t = log.snapshot();
    assert_eq!(
        t.events.first(),
        Some(&TraceEvent::Open {
            name: "data.redb".into(),
            file: FileId(0),
            created: true
        })
    );
    assert!(
        t.events
            .iter()
            .any(|e| matches!(e, TraceEvent::SetLen { .. }))
    );
    assert!(
        t.events
            .iter()
            .any(|e| matches!(e, TraceEvent::Write { .. }))
    );
    assert!(
        t.events
            .iter()
            .any(|e| matches!(e, TraceEvent::Fdatasync { .. }))
    );
    assert!(
        !t.events.contains(&TraceEvent::SyncDir),
        "redb dizin fsync'i yapmaz (ölçüldü)"
    );
    // Put'un span'i en az bir yazma ve bir fdatasync içerir (OnAck: ACK'ten önce dayanıklı).
    let s = t.spans[0];
    let span = &t.events[s.start..s.end.unwrap()];
    assert!(
        span.iter()
            .any(|e| matches!(e, TraceEvent::Fdatasync { .. }))
    );
}

/// Linux modeli + açılış fazı atlanmış: işlem fazında yanlış alarm yok (küçük örnek; büyük ölçüm
/// docs/MEASUREMENTS.md'de).
#[test]
fn linux_model_no_false_alarm() {
    let d = dir("linux");
    for seed in 0..3 {
        let ops = generate(seed, &WorkloadConfig::default());
        let o = run_case::<RedbProc>(
            ops,
            &RedbConfig::default(),
            &d,
            seed,
            &rc(true, CutSelection::Sample(12), true),
        )
        .unwrap();
        assert!(o.states > 0);
        assert_eq!(o.failure.map(|f| f.finding), None, "seed {seed}");
    }
}

/// Katı POSIX: redb dizini fsync'lemediği için ilk ACK'li yazma bile kaybolabilir (dosyanın adı
/// kalıcı değil) — bu modelde gerçek bir dayanıklılık boşluğu.
#[test]
fn posix_model_finds_missing_dir_fsync() {
    let d = dir("posix");
    let ops = vec![Op::Put {
        key: b"a".to_vec(),
        value: Vec::new(),
    }];
    let o = run_case::<RedbProc>(
        ops,
        &RedbConfig::default(),
        &d,
        0,
        &rc(false, CutSelection::All, true),
    )
    .unwrap();
    let f = o.failure.expect("bulgu bekleniyordu");
    match f.finding {
        Finding::Violation(v) => assert_eq!(v.kind, ViolationKind::LostDurableWrite),
        other => panic!("LostDurableWrite bekleniyordu: {other:?}"),
    }
    assert_eq!(
        f.recovered,
        Some(Default::default()),
        "veritabanı dosyası tümden kayıp"
    );
}

/// redb 4.3.0 hatası: `Database::create` ön tahsisten (ftruncate) sonra, magic numaralı başlık
/// yazılmadan çökerse dosya kalıcı olarak açılamaz. İşlem gerekmez; her iki FS modelinde de çıkar
/// (süreç öldürmeyle de doğrulandı — docs/MEASUREMENTS.md).
#[test]
fn create_crash_leaves_unopenable_file() {
    let d = dir("create");
    for linux in [false, true] {
        let o = run_case::<RedbProc>(
            Vec::new(),
            &RedbConfig::default(),
            &d,
            0,
            &rc(linux, CutSelection::All, false),
        )
        .unwrap();
        let f = o.failure.expect("bulgu bekleniyordu");
        match &f.finding {
            Finding::RecoveryFailed(m) => assert!(m.contains("magic number mismatch"), "{m}"),
            other => panic!("RecoveryFailed bekleniyordu: {other:?}"),
        }
        assert_eq!(
            f.crash_point.map(|c| c.completed),
            Some(0),
            "açılış fazında"
        );
    }
}
