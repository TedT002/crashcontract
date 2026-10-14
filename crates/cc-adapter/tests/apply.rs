//! cc-adapter kontrat testleri.

use std::collections::BTreeMap;
use std::path::Path;

use cc_adapter::{AdapterError, StorageAdapter, apply_op};
use cc_model::{
    Durability, Guarantees, History, HistoryError, Op, Outcome, State, Value, Verdict, Write,
    check_reads,
};
use cc_trace::{CrashPoint, OpSpan, TraceEvent, TraceLog};

/// Test içi bellek adaptörü: her yazmada log'a bir SyncDir olayı ekler; `fail_on` sıradaki
/// adaptör çağrısında G/Ç hatası üretir.
#[derive(Debug)]
struct MapAdapter {
    map: BTreeMap<Vec<u8>, Vec<u8>>,
    log: Option<TraceLog>,
    calls: usize,
    fail_on: Option<usize>,
}

#[derive(Debug, Clone)]
struct MapConfig {
    fail_on: Option<usize>,
}

impl MapAdapter {
    fn call(&mut self) -> Result<(), AdapterError> {
        let i = self.calls;
        self.calls += 1;
        if self.fail_on == Some(i) {
            return Err(std::io::Error::other("enjekte hata").into());
        }
        Ok(())
    }
    fn touch(&self) {
        if let Some(log) = &self.log {
            log.push(TraceEvent::SyncDir);
        }
    }
}

impl StorageAdapter for MapAdapter {
    type Config = MapConfig;
    fn guarantees(_: &MapConfig) -> Guarantees {
        Guarantees {
            durability: Durability::OnAck,
            atomic_txn: true,
            prefix_order: true,
        }
    }
    fn open(_: &Path, c: &MapConfig, log: Option<TraceLog>) -> Result<Self, AdapterError> {
        Ok(MapAdapter {
            map: BTreeMap::new(),
            log,
            calls: 0,
            fail_on: c.fail_on,
        })
    }
    fn put(&mut self, k: &[u8], v: &[u8]) -> Result<(), AdapterError> {
        self.call()?;
        self.touch();
        self.map.insert(k.to_vec(), v.to_vec());
        Ok(())
    }
    fn get(&mut self, k: &[u8]) -> Result<Option<Value>, AdapterError> {
        self.call()?;
        Ok(self.map.get(k).cloned())
    }
    fn delete(&mut self, k: &[u8]) -> Result<(), AdapterError> {
        self.call()?;
        self.touch();
        self.map.remove(k);
        Ok(())
    }
    fn txn(&mut self, ws: &[Write]) -> Result<(), AdapterError> {
        self.call()?;
        for w in ws {
            self.touch();
            match w {
                Write::Put { key, value } => {
                    self.map.insert(key.clone(), value.clone());
                }
                Write::Delete { key } => {
                    self.map.remove(key);
                }
            }
        }
        Ok(())
    }
    fn sync(&mut self) -> Result<(), AdapterError> {
        self.call()
    }
    fn scan(&mut self) -> Result<State, AdapterError> {
        Ok(self.map.clone())
    }
    fn close(self) -> Result<(), AdapterError> {
        Ok(())
    }
}

fn b(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

fn a01_ops() -> Vec<Op> {
    vec![
        Op::Put {
            key: b("a"),
            value: b("1"),
        },
        Op::Get { key: b("a") },
        Op::Txn {
            writes: vec![
                Write::Put {
                    key: b("b"),
                    value: b("2"),
                },
                Write::Delete { key: b("a") },
            ],
        },
        Op::Sync,
        Op::Get { key: b("a") },
    ]
}

fn open(fail_on: Option<usize>, log: Option<TraceLog>) -> MapAdapter {
    MapAdapter::open(Path::new("."), &MapConfig { fail_on }, log).unwrap()
}

/// A01: sonuçlar doğru türde kaydedilir; okumalar modelle tutarlı.
#[test]
fn a01_outcomes_and_reads() {
    let mut ad = open(None, None);
    let mut h = History::new();
    for (i, op) in a01_ops().into_iter().enumerate() {
        assert_eq!(apply_op(&mut ad, &mut h, None, op), Ok(i));
    }
    let outcomes: Vec<_> = h.entries().iter().map(|e| e.outcome.clone()).collect();
    assert_eq!(
        outcomes,
        vec![
            Outcome::Acked,
            Outcome::Read {
                value: Some(b("1"))
            },
            Outcome::Acked,
            Outcome::Acked,
            Outcome::Read { value: None },
        ]
    );
    assert_eq!(check_reads(&h), Verdict::Allowed { witness: None });
    assert_eq!(ad.scan().unwrap(), State::from([(b("b"), b("2"))]));
}

/// A02: adaptör hatası Failed olur ve geçmişi kapatır; sonraki işlem adaptöre hiç ulaşmaz.
#[test]
fn a02_failure_closes_history() {
    let log = TraceLog::new();
    let mut ad = open(Some(1), Some(log.clone()));
    let mut h = History::new();
    let put = |v: &str| Op::Put {
        key: b("k"),
        value: b(v),
    };
    apply_op(&mut ad, &mut h, Some(&log), put("1")).unwrap();
    apply_op(&mut ad, &mut h, Some(&log), put("2")).unwrap();
    match &h.entries()[1].outcome {
        Outcome::Failed { reason } => assert!(reason.contains("enjekte hata"), "{reason}"),
        o => panic!("Failed bekleniyordu: {o:?}"),
    }
    assert_eq!(
        apply_op(&mut ad, &mut h, Some(&log), put("3")),
        Err(HistoryError::Closed)
    );
    assert_eq!(ad.calls, 2, "üçüncü işlem adaptöre ulaşmamalı");
    assert_eq!(
        log.snapshot().spans.len(),
        2,
        "kapalı geçmişte span açılmaz"
    );
}

/// A03: span'ler her işlemin olay aralığını doğru çizer.
#[test]
fn a03_spans_and_crash_point() {
    let log = TraceLog::new();
    let mut ad = open(None, Some(log.clone()));
    let mut h = History::new();
    for op in a01_ops() {
        apply_op(&mut ad, &mut h, Some(&log), op).unwrap();
    }
    let s = |start, end| OpSpan {
        start,
        end: Some(end),
    };
    let trace = log.snapshot();
    assert_eq!(
        trace.spans,
        vec![s(0, 1), s(1, 1), s(1, 3), s(3, 3), s(3, 3)]
    );
    assert_eq!(
        trace.crash_point(2),
        CrashPoint {
            completed: 2,
            in_flight: true
        }
    );
}
