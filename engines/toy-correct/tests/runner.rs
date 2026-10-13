//! Runner testleri (SPEC_stage4 §3): doğru motor temiz geçer; bilerek yanlış adaptörler bulgu üretir.

mod common;

use std::path::Path;

use cc_adapter::runner::{CutSelection, Finding, RunConfig, run_case, select_cuts};
use cc_adapter::{AdapterError, StorageAdapter};
use cc_model::{Durability, Guarantees, State, Value, ViolationKind, Write};
use cc_trace::TraceLog;
use cc_workload::{WorkloadConfig, generate};
use common::fresh_dir;
use toy_correct::{SyncPolicy, ToyConfig, ToyDb};

/// Hangi yalanın söyleneceği.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lie {
    /// OnSyncOnly ile çalışır ama OnAck bildirir: ACK'li yazmalar çökmede kaybolabilir.
    ClaimsOnAck,
    /// Çökme sonrası (kayıtsız) açılışta her zaman hata.
    NeverRecovers,
    /// Çalışma sırasında okumalarda yanlış değer döner.
    BadReads,
}

/// ToyDb sarmalayıcısı: bilerek yanlış davranır (runner'ın bulgu üretebildiğini kanıtlar).
#[derive(Debug)]
struct Liar {
    inner: ToyDb,
    lie: Lie,
}

impl StorageAdapter for Liar {
    type Config = Lie;
    fn guarantees(lie: &Lie) -> Guarantees {
        let durability = if *lie == Lie::ClaimsOnAck {
            Durability::OnAck
        } else {
            Durability::OnSync
        };
        Guarantees {
            durability,
            atomic_txn: true,
            prefix_order: true,
        }
    }
    fn open(dir: &Path, lie: &Lie, log: Option<TraceLog>) -> Result<Self, AdapterError> {
        if *lie == Lie::NeverRecovers && log.is_none() {
            return Err(AdapterError::Corruption("bilerek açılmıyor".into()));
        }
        let cfg = ToyConfig {
            sync_policy: SyncPolicy::OnSyncOnly,
            ..ToyConfig::default()
        };
        Ok(Liar {
            inner: ToyDb::open(dir, &cfg, log)?,
            lie: *lie,
        })
    }
    fn put(&mut self, k: &[u8], v: &[u8]) -> Result<(), AdapterError> {
        self.inner.put(k, v)
    }
    fn get(&mut self, k: &[u8]) -> Result<Option<Value>, AdapterError> {
        let v = self.inner.get(k)?;
        Ok(if self.lie == Lie::BadReads {
            Some(b"yalan".to_vec())
        } else {
            v
        })
    }
    fn delete(&mut self, k: &[u8]) -> Result<(), AdapterError> {
        self.inner.delete(k)
    }
    fn txn(&mut self, ws: &[Write]) -> Result<(), AdapterError> {
        self.inner.txn(ws)
    }
    fn sync(&mut self) -> Result<(), AdapterError> {
        self.inner.sync()
    }
    fn scan(&mut self) -> Result<State, AdapterError> {
        self.inner.scan()
    }
    fn close(self) -> Result<(), AdapterError> {
        self.inner.close()
    }
}

/// Doğru motor: tüm kesmelerde bulgu yok; sayaçlar dolu.
#[test]
fn clean_engine_has_no_findings() {
    let dir = fresh_dir("runner-clean");
    for seed in 0..20u64 {
        let cfg = ToyConfig {
            sync_policy: if seed.is_multiple_of(2) {
                SyncPolicy::EveryCommit
            } else {
                SyncPolicy::OnSyncOnly
            },
            checkpoint_every: 7,
            mutation: None,
        };
        let ops = generate(seed, &WorkloadConfig::default());
        let n = ops.len();
        let o = run_case::<ToyDb>(ops, &cfg, &dir, seed, &RunConfig::default()).unwrap();
        assert_eq!(o.failure, None, "seed {seed}");
        assert_eq!(o.ops, n);
        assert_eq!(o.cuts, o.events + 1);
        assert!(o.states >= o.cuts);
    }
}

/// OnAck bildirip fsync'lemeyen adaptör: dayanıklı ACK'li yazma kaybı yakalanır.
#[test]
fn lying_durability_is_caught() {
    let dir = fresh_dir("runner-liar");
    let ops = generate(3, &WorkloadConfig::default());
    let o = run_case::<Liar>(ops, &Lie::ClaimsOnAck, &dir, 3, &RunConfig::default()).unwrap();
    let f = o.failure.expect("bulgu bekleniyordu");
    assert!(f.cut.is_some() && f.choice.is_some() && f.recovered.is_some());
    match f.finding {
        Finding::Violation(v) => assert_eq!(v.kind, ViolationKind::LostDurableWrite, "{v}"),
        other => panic!("ihlal bekleniyordu: {other:?}"),
    }
}

/// Çökmeden sonra açılamayan motor: RecoveryFailed.
#[test]
fn recovery_failure_is_a_finding() {
    let dir = fresh_dir("runner-norecover");
    let ops = generate(1, &WorkloadConfig::default());
    let o = run_case::<Liar>(ops, &Lie::NeverRecovers, &dir, 1, &RunConfig::default()).unwrap();
    let f = o.failure.expect("bulgu bekleniyordu");
    assert_eq!(f.cut, Some(0), "ilk kesmede yakalanmalı");
    assert!(
        matches!(f.finding, Finding::RecoveryFailed(_)),
        "{:?}",
        f.finding
    );
}

/// Çalışma sırasında yanlış okuma: çökme durumlarına geçmeden ReadMismatch.
#[test]
fn bad_reads_are_caught_without_crash() {
    let dir = fresh_dir("runner-badreads");
    let ops = generate(5, &WorkloadConfig::default());
    let o = run_case::<Liar>(ops, &Lie::BadReads, &dir, 5, &RunConfig::default()).unwrap();
    let f = o.failure.expect("bulgu bekleniyordu");
    assert_eq!((f.cut, o.states), (None, 0));
    match f.finding {
        Finding::Violation(v) => assert_eq!(v.kind, ViolationKind::ReadMismatch),
        other => panic!("ReadMismatch bekleniyordu: {other:?}"),
    }
}

/// Kesme örneklemesi: deterministik, tekrarsız, artan, sınırlı; küçük izde hepsi.
#[test]
fn cut_sampling() {
    let a = select_cuts(100, 9, CutSelection::Sample(5));
    assert_eq!(a, select_cuts(100, 9, CutSelection::Sample(5)));
    assert_eq!(a.len(), 5);
    assert!(a.windows(2).all(|w| w[0] < w[1]) && a.iter().all(|&c| c <= 100));
    assert_ne!(a, select_cuts(100, 10, CutSelection::Sample(5)));
    assert_eq!(
        select_cuts(3, 0, CutSelection::Sample(10)),
        vec![0, 1, 2, 3]
    );
    assert_eq!(select_cuts(3, 0, CutSelection::All), vec![0, 1, 2, 3]);
}
