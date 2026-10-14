//! toy-correct motor testleri.

mod common;

use std::io::Write as _;
use std::path::Path;

use cc_adapter::{AdapterError, StorageAdapter, apply_op};
use cc_model::{Durability, Guarantees, History, Op, State, Verdict, Write, check_reads};
use cc_trace::{CrashPoint, FileId, OpSpan, TraceEvent as E, TraceLog};
use common::{b, fresh_dir};
use toy_correct::format::{
    CHECKPOINT_FILE as CP, CHECKPOINT_TMP_FILE as TMP, decode_checkpoint, encode_checkpoint,
    encode_wal_record, wal_file_name,
};
use toy_correct::{SyncPolicy, ToyConfig, ToyDb};

fn cfg(policy: SyncPolicy, every: u32) -> ToyConfig {
    ToyConfig {
        sync_policy: policy,
        checkpoint_every: every,
        mutation: None,
    }
}
fn put(k: &str, v: &str) -> Op {
    Op::Put {
        key: b(k),
        value: b(v),
    }
}
fn del(k: &str) -> Op {
    Op::Delete { key: b(k) }
}
fn get(k: &str) -> Op {
    Op::Get { key: b(k) }
}
fn wput(k: &str, v: &str) -> Write {
    Write::Put {
        key: b(k),
        value: b(v),
    }
}
fn wdel(k: &str) -> Write {
    Write::Delete { key: b(k) }
}
fn state(pairs: &[(&str, &str)]) -> State {
    pairs.iter().map(|(k, v)| (b(k), b(v))).collect()
}

/// Kayıtlı bir koşu: boş dizinde açılış + işlemler.
struct Run {
    db: ToyDb,
    h: History,
    log: TraceLog,
}

impl Run {
    fn new(dir: &Path, c: &ToyConfig) -> Run {
        let log = TraceLog::new();
        let db = ToyDb::open(dir, c, Some(log.clone())).expect("açılış");
        Run {
            db,
            h: History::new(),
            log,
        }
    }
    fn apply(&mut self, ops: impl IntoIterator<Item = Op>) {
        for op in ops {
            apply_op(&mut self.db, &mut self.h, Some(&self.log), op).expect("issue");
        }
    }
    /// `i`. işlemin span'indeki olaylar.
    fn span_events(&self, i: usize) -> Vec<E> {
        let t = self.log.snapshot();
        let s = t.spans[i];
        t.events[s.start..s.end.unwrap()].to_vec()
    }
}

/// Kayıtsız (log'suz) yeniden açılış.
fn reopen(dir: &Path, c: &ToyConfig) -> Result<ToyDb, AdapterError> {
    ToyDb::open(dir, c, None)
}

fn list(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

fn append(path: &Path, bytes: &[u8]) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

/// Y05: temel işlevsellik ve bildirilen garantiler, iki politika için.
#[test]
fn y05_basic_ops_and_guarantees() {
    for (policy, dur) in [
        (SyncPolicy::EveryCommit, Durability::OnAck),
        (SyncPolicy::OnSyncOnly, Durability::OnSync),
    ] {
        let c = cfg(policy, 0);
        let mut r = Run::new(&fresh_dir(&format!("y05-{policy:?}")), &c);
        r.apply([
            put("a", "1"),
            put("b", "2"),
            del("a"),
            Op::Txn {
                writes: vec![wput("c", "3"), wdel("b")],
            },
            get("c"),
            get("a"),
            Op::Sync,
        ]);
        assert_eq!(r.db.scan().unwrap(), state(&[("c", "3")]));
        assert_eq!(check_reads(&r.h), Verdict::Allowed { witness: None });
        assert_eq!(
            ToyDb::guarantees(&c),
            Guarantees {
                durability: dur,
                atomic_txn: true,
                prefix_order: true
            }
        );
    }
}

/// Bootstrap olayları (R2 + C1–C3): 7 olay.
fn bootstrap_events() -> Vec<E> {
    vec![
        E::Open {
            name: wal_file_name(1),
            file: FileId(0),
            created: true,
        },
        E::SyncDir,
        E::Open {
            name: TMP.into(),
            file: FileId(1),
            created: true,
        },
        E::Write {
            file: FileId(1),
            offset: 0,
            data: encode_checkpoint(1, &State::new()),
        },
        E::Fsync { file: FileId(1) },
        E::Rename {
            from: TMP.into(),
            to: CP.into(),
        },
        E::SyncDir,
    ]
}

/// Y06: EveryCommit'te tam olay listesi — fsync düzeni §4.3 tablosuyla birebir.
#[test]
fn y06_every_commit_full_event_list() {
    let mut r = Run::new(&fresh_dir("y06"), &cfg(SyncPolicy::EveryCommit, 0));
    r.apply([put("a", "1")]);
    let mut expected = bootstrap_events();
    expected.extend([
        E::Write {
            file: FileId(0),
            offset: 0,
            data: encode_wal_record(1, &[wput("a", "1")]),
        },
        E::Fdatasync { file: FileId(0) },
    ]);
    let t = r.log.snapshot();
    assert_eq!(t.events, expected);
    assert_eq!(
        t.spans,
        vec![OpSpan {
            start: 7,
            end: Some(9)
        }]
    );
}

/// Y07: OnSyncOnly'de fdatasync yalnız Sync'te; Get ve boş Txn G/Ç yapmaz.
#[test]
fn y07_on_sync_only_defers_fdatasync() {
    let mut r = Run::new(&fresh_dir("y07"), &cfg(SyncPolicy::OnSyncOnly, 0));
    r.apply([
        put("a", "1"),
        get("a"),
        Op::Txn { writes: vec![] },
        Op::Sync,
    ]);
    assert_eq!(
        r.span_events(0),
        vec![E::Write {
            file: FileId(0),
            offset: 0,
            data: encode_wal_record(1, &[wput("a", "1")])
        }]
    );
    assert_eq!(r.span_events(1), vec![]);
    assert_eq!(r.span_events(2), vec![]);
    assert_eq!(r.span_events(3), vec![E::Fdatasync { file: FileId(0) }]);
}

/// Y08: Txn = tek kayıt = tek Write (atomikliğin kaynağı).
#[test]
fn y08_txn_is_single_record() {
    let mut r = Run::new(&fresh_dir("y08"), &cfg(SyncPolicy::OnSyncOnly, 0));
    let ws = vec![wput("a", "1"), wput("b", "2"), wdel("a")];
    r.apply([Op::Txn { writes: ws.clone() }]);
    assert_eq!(
        r.span_events(0),
        vec![E::Write {
            file: FileId(0),
            offset: 0,
            data: encode_wal_record(1, &ws)
        }]
    );
}

/// Y09: işlem içinde tetiklenen checkpoint'in olay dizisi; sonraki kayıt yeni WAL'a LSN 1 ile gider.
#[test]
fn y09_checkpoint_inside_op() {
    let mut r = Run::new(&fresh_dir("y09"), &cfg(SyncPolicy::EveryCommit, 2));
    r.apply([put("a", "1"), put("b", "2"), put("c", "3")]);
    let rec_a = encode_wal_record(1, &[wput("a", "1")]);
    assert_eq!(
        r.span_events(1),
        vec![
            E::Write {
                file: FileId(0),
                offset: rec_a.len() as u64,
                data: encode_wal_record(2, &[wput("b", "2")]),
            },
            E::Fdatasync { file: FileId(0) },
            E::Open {
                name: wal_file_name(2),
                file: FileId(2),
                created: true
            },
            E::SyncDir,
            E::Open {
                name: TMP.into(),
                file: FileId(3),
                created: true
            },
            E::Write {
                file: FileId(3),
                offset: 0,
                data: encode_checkpoint(2, &state(&[("a", "1"), ("b", "2")])),
            },
            E::Fsync { file: FileId(3) },
            E::Rename {
                from: TMP.into(),
                to: CP.into()
            },
            E::SyncDir,
            E::Unlink {
                name: wal_file_name(1)
            },
        ]
    );
    assert_eq!(
        r.span_events(2),
        vec![
            E::Write {
                file: FileId(2),
                offset: 0,
                data: encode_wal_record(1, &[wput("c", "3")])
            },
            E::Fdatasync { file: FileId(2) },
        ]
    );
}

/// Y10: temiz kapanış + yeniden açılış: WAL replay ve kurtarma checkpoint'i (yeni nesil).
#[test]
fn y10_reopen_replays_and_checkpoints() {
    let dir = fresh_dir("y10");
    let c = cfg(SyncPolicy::EveryCommit, 0);
    let mut r = Run::new(&dir, &c);
    r.apply([put("a", "1"), put("b", "2"), del("b")]);
    r.db.close().unwrap();
    let mut db = reopen(&dir, &c).unwrap();
    assert_eq!(db.scan().unwrap(), state(&[("a", "1")]));
    assert_eq!(list(&dir), vec![CP.to_string(), wal_file_name(2)]);
    let (g, _) = decode_checkpoint(&std::fs::read(dir.join(CP)).unwrap()).unwrap();
    assert_eq!(g, 2);
}

/// Y11: birden çok otomatik checkpoint sonrası durum korunur, dizinde tek WAL kalır.
#[test]
fn y11_many_checkpoints() {
    let dir = fresh_dir("y11");
    let c = cfg(SyncPolicy::EveryCommit, 3);
    let mut r = Run::new(&dir, &c);
    r.apply((0..10).map(|i| put(&format!("k{i}"), &format!("v{i}"))));
    r.db.close().unwrap();
    let mut db = reopen(&dir, &c).unwrap();
    let expected: State = (0..10)
        .map(|i| (b(&format!("k{i}")), b(&format!("v{i}"))))
        .collect();
    assert_eq!(db.scan().unwrap(), expected);
    assert_eq!(
        list(&dir).iter().filter(|n| n.starts_with("wal-")).count(),
        1
    );
}

/// Y12: silme (tombstone) hem WAL replay'inde hem checkpoint'te korunur.
#[test]
fn y12_tombstones_survive_replay_and_checkpoint() {
    let dir = fresh_dir("y12");
    let mut r = Run::new(&dir, &cfg(SyncPolicy::EveryCommit, 0));
    r.apply([put("a", "1"), del("a")]);
    r.db.close().unwrap();
    let c1 = cfg(SyncPolicy::EveryCommit, 1);
    let mut db = reopen(&dir, &c1).unwrap();
    assert_eq!(db.scan().unwrap(), State::new());
    db.put(b"b", b"1").unwrap();
    db.delete(b"b").unwrap();
    db.close().unwrap();
    let mut db = reopen(&dir, &c1).unwrap();
    assert_eq!(db.scan().unwrap(), State::new());
}

/// P(a,1), P(b,2), [P(c,3)] ile kapatılmış bir dizin.
fn closed_dir(name: &str, with_c: bool) -> std::path::PathBuf {
    let dir = fresh_dir(name);
    let mut r = Run::new(&dir, &cfg(SyncPolicy::EveryCommit, 0));
    r.apply([put("a", "1"), put("b", "2")]);
    if with_c {
        r.apply([put("c", "3")]);
    }
    r.db.close().unwrap();
    dir
}

/// Y13: yarım (yırtık) son kayıt yok sayılır; eski WAL silinir, yeni nesil açılır.
#[test]
fn y13_torn_tail_ignored() {
    let dir = closed_dir("y13", false);
    let rec = encode_wal_record(3, &[wput("c", "3")]);
    append(&dir.join(wal_file_name(1)), &rec[..rec.len() / 2]);
    let mut db = reopen(&dir, &ToyConfig::default()).unwrap();
    assert_eq!(db.scan().unwrap(), state(&[("a", "1"), ("b", "2")]));
    assert_eq!(list(&dir), vec![CP.to_string(), wal_file_name(2)]);
}

/// Y14 yardımcı: rec(2)'nin CRC'sini boz (rec(3) geçerli kalır) ve yeniden aç.
fn corrupt_middle_and_reopen(name: &str) -> (std::path::PathBuf, ToyDb) {
    let dir = closed_dir(name, true);
    let wal = dir.join(wal_file_name(1));
    let mut bytes = std::fs::read(&wal).unwrap();
    let off = encode_wal_record(1, &[wput("a", "1")]).len();
    bytes[off + 12] ^= 0xFF;
    std::fs::write(&wal, bytes).unwrap();
    let db = reopen(&dir, &ToyConfig::default()).unwrap();
    (dir, db)
}

/// Y14: ilk bozuk kayıtta dur — ardındaki geçerli c uygulanmaz (prefix tutarlılığı).
#[test]
fn y14_stop_at_first_bad_record() {
    let (_dir, mut db) = corrupt_middle_and_reopen("y14");
    assert_eq!(db.scan().unwrap(), state(&[("a", "1")]));
}

/// Y15: eski-geçerli-kayıt problemi. Aynı WAL'a ekleme yapılsaydı, b ile aynı boyda olan d kaydı
/// rec(2)'nin yerine oturur ve ardındaki eski-geçerli rec(3) (c) diriltilirdi. Nesil başına yeni
/// WAL bunu yapısal olarak engeller.
#[test]
fn y15_stale_valid_record_never_resurrected() {
    let (dir, mut db) = corrupt_middle_and_reopen("y15");
    db.put(b"d", b"4").unwrap();
    db.close().unwrap();
    // d yeni nesil WAL'a (wal-2), LSN 1 ile yazıldı; eski WAL kurtarmada silindi.
    assert_eq!(list(&dir), vec![CP.to_string(), wal_file_name(2)]);
    assert_eq!(
        std::fs::read(dir.join(wal_file_name(2))).unwrap(),
        encode_wal_record(1, &[wput("d", "4")])
    );
    let mut db = reopen(&dir, &ToyConfig::default()).unwrap();
    assert_eq!(db.scan().unwrap(), state(&[("a", "1"), ("d", "4")]));
}

/// Y16: LSN atlaması (CRC geçerli olsa da) replay'i durdurur.
#[test]
fn y16_lsn_gap_stops_replay() {
    let dir = closed_dir("y16", false);
    append(
        &dir.join(wal_file_name(1)),
        &encode_wal_record(5, &[wput("z", "9")]),
    );
    let mut db = reopen(&dir, &ToyConfig::default()).unwrap();
    assert_eq!(db.scan().unwrap(), state(&[("a", "1"), ("b", "2")]));
}

/// Y17: bozuk CHECKPOINT, eksik WAL ve tanınmayan dosya kurtarmayı reddettirir (gizlenmez).
#[test]
fn y17_corruption_rejected() {
    let corrupt = |dir: &Path| {
        assert!(matches!(
            reopen(dir, &ToyConfig::default()),
            Err(AdapterError::Corruption(_))
        ))
    };

    let dir = closed_dir("y17a", false);
    let mut cp = std::fs::read(dir.join(CP)).unwrap();
    cp[10] ^= 1;
    std::fs::write(dir.join(CP), cp).unwrap();
    corrupt(&dir);

    let dir = closed_dir("y17b", false);
    std::fs::remove_file(dir.join(wal_file_name(1))).unwrap();
    corrupt(&dir);

    let dir = closed_dir("y17c", false);
    std::fs::write(dir.join("junk"), b"x").unwrap();
    corrupt(&dir);
}

/// Y18: açılış GC'si yarım checkpoint TMP'sini ve yabancı nesil WAL'ı siler; durum korunur.
#[test]
fn y18_open_garbage_collects() {
    let dir = closed_dir("y18", false);
    std::fs::write(dir.join(TMP), b"cop").unwrap();
    std::fs::write(dir.join(wal_file_name(9)), b"").unwrap();
    let mut db = reopen(&dir, &ToyConfig::default()).unwrap();
    assert_eq!(db.scan().unwrap(), state(&[("a", "1"), ("b", "2")]));
    assert_eq!(list(&dir), vec![CP.to_string(), wal_file_name(2)]);
}

/// Y19: bootstrap sırasında çökme (CHECKPOINT yok) → her şey silinir, sıfırdan bootstrap.
#[test]
fn y19_bootstrap_crash_restarts() {
    let dir = fresh_dir("y19");
    std::fs::write(dir.join(wal_file_name(1)), b"").unwrap();
    std::fs::write(dir.join(TMP), &encode_checkpoint(1, &State::new())[..10]).unwrap();
    let mut db = reopen(&dir, &ToyConfig::default()).unwrap();
    assert_eq!(db.scan().unwrap(), State::new());
    assert_eq!(list(&dir), vec![CP.to_string(), wal_file_name(1)]);
    let (g, _) = decode_checkpoint(&std::fs::read(dir.join(CP)).unwrap()).unwrap();
    assert_eq!(g, 1);
}

/// Y20: sınırı aşan kayıt G/Ç'den önce reddedilir; iz ve durum değişmez.
#[test]
fn y20_oversized_record_rejected_before_io() {
    let mut r = Run::new(&fresh_dir("y20"), &ToyConfig::default());
    let before = r.log.event_count();
    let big = vec![0u8; (1 << 24) + 1];
    match r.db.put(b"k", &big) {
        Err(AdapterError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput),
        other => panic!("InvalidInput bekleniyordu: {other:?}"),
    }
    assert_eq!(r.log.event_count(), before);
    assert_eq!(r.db.scan().unwrap(), State::new());
}

/// Y21: apply_op ile span'ler geçmişle hizalı; Get G/Ç yapmaz; iz sonunda hepsi tamamlanmış.
#[test]
fn y21_spans_align_with_history() {
    let mut r = Run::new(&fresh_dir("y21"), &ToyConfig::default());
    r.apply([put("a", "1"), get("a"), del("a"), get("a"), Op::Sync]);
    let t = r.log.snapshot();
    assert_eq!(t.spans.len(), 5);
    for i in [1, 3] {
        assert_eq!(
            Some(t.spans[i].start),
            t.spans[i].end,
            "Get span'i boş olmalı"
        );
    }
    assert_eq!(
        t.crash_point(t.events.len()),
        CrashPoint {
            completed: 5,
            in_flight: false
        }
    );
}
