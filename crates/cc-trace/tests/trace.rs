//! cc-trace kontrat testleri.

mod common;

use std::io::ErrorKind;

use cc_trace::{CrashPoint, DirVfs, FileId, OpSpan, OpenMode, Trace, TraceEvent, TraceLog, Vfs};
use common::fresh_dir;

fn recorded(name: &str) -> (DirVfs, TraceLog) {
    let log = TraceLog::new();
    let vfs = DirVfs::new(fresh_dir(name), Some(log.clone())).expect("DirVfs");
    (vfs, log)
}

fn cp(completed: usize, in_flight: bool) -> CrashPoint {
    CrashPoint {
        completed,
        in_flight,
    }
}

/// T01: her değiştirici çağrı birebir bir olay üretir, sırası korunur.
#[test]
fn t01_events_recorded_in_order() {
    let (mut vfs, log) = recorded("t01");
    let f = vfs.open("a", OpenMode::CreateNew).unwrap();
    vfs.write_at(f, 0, b"xy").unwrap();
    vfs.set_len(f, 1).unwrap();
    vfs.fsync(f).unwrap();
    vfs.fdatasync(f).unwrap();
    vfs.sync_dir().unwrap();
    vfs.rename("a", "b").unwrap();
    vfs.unlink("b").unwrap();
    let f0 = FileId(0);
    assert_eq!(
        log.snapshot().events,
        vec![
            TraceEvent::Open {
                name: "a".into(),
                file: f0,
                created: true
            },
            TraceEvent::Write {
                file: f0,
                offset: 0,
                data: b"xy".to_vec()
            },
            TraceEvent::SetLen { file: f0, len: 1 },
            TraceEvent::Fsync { file: f0 },
            TraceEvent::Fdatasync { file: f0 },
            TraceEvent::SyncDir,
            TraceEvent::Rename {
                from: "a".into(),
                to: "b".into()
            },
            TraceEvent::Unlink { name: "b".into() },
        ]
    );
}

/// T02: okuma, list, close ve boş yazma kaydedilmez (dayanıklı durumu değiştirmezler).
#[test]
fn t02_non_mutating_calls_not_recorded() {
    let (mut vfs, log) = recorded("t02");
    let a = vfs.open("b", OpenMode::CreateNew).unwrap();
    vfs.open("a", OpenMode::CreateNew).unwrap();
    vfs.write_at(a, 0, b"hello").unwrap();
    let n = log.event_count();
    assert_eq!(vfs.read_all(a).unwrap(), b"hello");
    assert_eq!(vfs.list().unwrap(), vec!["a".to_string(), "b".to_string()]);
    vfs.write_at(a, 3, b"").unwrap();
    vfs.close(a).unwrap();
    assert_eq!(log.event_count(), n);
}

/// T03: FileId inode kimliğidir — rename sonrası yeniden açılış aynı kimliği alır.
#[test]
fn t03_file_id_follows_inode_across_rename() {
    let (mut vfs, log) = recorded("t03");
    let a = vfs.open("a", OpenMode::CreateNew).unwrap();
    let b = vfs.open("b", OpenMode::CreateNew).unwrap();
    assert_eq!((a, b), (FileId(0), FileId(1)));
    vfs.close(a).unwrap();
    vfs.rename("a", "c").unwrap();
    let c = vfs.open("c", OpenMode::Existing).unwrap();
    assert_eq!(c, FileId(0));
    assert_eq!(
        log.snapshot().events.last(),
        Some(&TraceEvent::Open {
            name: "c".into(),
            file: FileId(0),
            created: false
        })
    );
}

/// T04: başarısız çağrılar hata döndürür ve olay EKLEMEZ.
#[test]
fn t04_failed_calls_not_recorded() {
    let (mut vfs, log) = recorded("t04");
    vfs.open("a", OpenMode::CreateNew).unwrap();
    let n = log.event_count();
    assert_eq!(
        vfs.open("a", OpenMode::CreateNew).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(
        vfs.rename("yok", "x").unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(
        vfs.open("yok", OpenMode::Existing).unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(log.event_count(), n);
}

/// T05: yol kaçışı ve geçersiz tanıtıcılar InvalidInput; olay yok.
#[test]
fn t05_invalid_names_and_handles() {
    let (mut vfs, log) = recorded("t05");
    for bad in ["", "a/b", ".", "..", "a\0"] {
        assert_eq!(
            vfs.open(bad, OpenMode::CreateNew).unwrap_err().kind(),
            ErrorKind::InvalidInput,
            "ad {bad:?}"
        );
    }
    let a = vfs.open("a", OpenMode::CreateNew).unwrap();
    let n = log.event_count();
    vfs.close(a).unwrap();
    assert_eq!(
        vfs.write_at(a, 0, b"x").unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    vfs.open("a", OpenMode::Existing).unwrap();
    assert_eq!(
        vfs.open("a", OpenMode::Existing).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    // Yalnız yeniden açılış (geçerli) bir olay ekledi.
    assert_eq!(log.event_count(), n + 1);
}

/// T06: kayıtlı DirVfs yalnız boş dizinde ve tek kaydediciye bağlanır.
#[test]
fn t06_recording_requires_empty_dir_and_single_attach() {
    let dir = fresh_dir("t06");
    std::fs::write(dir.join("x"), b"1").unwrap();
    let err = DirVfs::new(&dir, Some(TraceLog::new())).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::DirectoryNotEmpty);
    // Kayıtsız kip dolu dizinde açılabilir.
    DirVfs::new(&dir, None).unwrap();

    let log = TraceLog::new();
    DirVfs::new(fresh_dir("t06b"), Some(log.clone())).unwrap();
    let err = DirVfs::new(fresh_dir("t06c"), Some(log)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
}

/// T07: çağrıların gerçek dosya sistemine etkisi.
#[test]
fn t07_real_effects_on_disk() {
    let dir = fresh_dir("t07");
    let mut vfs = DirVfs::new(&dir, None).unwrap();
    let a = vfs.open("a", OpenMode::CreateNew).unwrap();
    vfs.write_at(a, 0, b"abcdef").unwrap();
    vfs.write_at(a, 2, b"ZZ").unwrap();
    vfs.set_len(a, 5).unwrap();
    assert_eq!(std::fs::read(dir.join("a")).unwrap(), b"abZZe");
    vfs.open("b", OpenMode::CreateNew).unwrap();
    vfs.rename("a", "c").unwrap();
    vfs.unlink("b").unwrap();
    assert_eq!(vfs.list().unwrap(), vec!["c".to_string()]);
    assert_eq!(std::fs::read(dir.join("c")).unwrap(), b"abZZe");
    assert_eq!(vfs.root(), dir.as_path());
}

/// T08: kesme noktası → crash_view argümanları (olayı hiç gerçekleşmemiş işlem gönderilmemiş sayılır).
#[test]
fn t08_crash_point_mapping() {
    let t = Trace {
        events: vec![TraceEvent::SyncDir; 6],
        spans: vec![
            OpSpan {
                start: 2,
                end: Some(4),
            },
            OpSpan {
                start: 4,
                end: Some(4),
            },
            OpSpan {
                start: 4,
                end: Some(6),
            },
        ],
    };
    let got: Vec<_> = [0, 2, 3, 4, 5, 6]
        .iter()
        .map(|&c| t.crash_point(c))
        .collect();
    assert_eq!(
        got,
        vec![
            cp(0, false),
            cp(0, false),
            cp(0, true),
            cp(2, false),
            cp(2, true),
            cp(3, false)
        ]
    );
    let open = Trace {
        events: vec![TraceEvent::SyncDir; 2],
        spans: vec![OpSpan {
            start: 1,
            end: None,
        }],
    };
    assert_eq!(open.crash_point(1), cp(0, false));
    assert_eq!(open.crash_point(2), cp(0, true));
}

/// T09a: begin_op indeksi span sayısına eşit olmalı.
#[test]
#[should_panic(expected = "begin_op")]
fn t09_begin_op_wrong_index_panics() {
    TraceLog::new().begin_op(1);
}

/// T09b: açık span yokken end_op.
#[test]
#[should_panic(expected = "end_op")]
fn t09_end_op_without_open_span_panics() {
    TraceLog::new().end_op(0);
}

/// T09c: kesme olay sayısını aşamaz.
#[test]
#[should_panic(expected = "crash_point")]
fn t09_crash_point_out_of_range_panics() {
    let t = Trace::default();
    let _ = t.crash_point(1);
}
