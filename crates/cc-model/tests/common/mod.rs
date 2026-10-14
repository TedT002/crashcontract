// Her test dosyası (history.rs, recovered.rs, reads.rs, brute_force.rs) bu modülü ayrı ayrı
// derler ve hiçbiri her yardımcıyı kullanmaz; dead_code uyarısı `-D warnings` ile CI'ı kırar.
#![allow(dead_code)]

//! Testler için küçük DSL: geçmiş/işlem/durum kurmayı tek satıra indirir, böylece
//! test tablosu okunabilir kalır.

use cc_model::{Durability, Entry, Guarantees, History, Key, Op, Outcome, State, Value, Write};

/// Metinden anahtar.
pub fn k(s: &str) -> Key {
    s.as_bytes().to_vec()
}

/// Metinden değer.
pub fn v(s: &str) -> Value {
    s.as_bytes().to_vec()
}

/// `Op::Put`.
pub fn put(key: &str, val: &str) -> Op {
    Op::Put {
        key: k(key),
        value: v(val),
    }
}

/// `Op::Delete`.
pub fn del(key: &str) -> Op {
    Op::Delete { key: k(key) }
}

/// `Op::Get`.
pub fn get(key: &str) -> Op {
    Op::Get { key: k(key) }
}

/// `Op::Txn`.
pub fn txn(writes: Vec<Write>) -> Op {
    Op::Txn { writes }
}

/// Txn içi Put.
pub fn wput(key: &str, val: &str) -> Write {
    Write::Put {
        key: k(key),
        value: v(val),
    }
}

/// Txn içi Delete.
pub fn wdel(key: &str) -> Write {
    Write::Delete { key: k(key) }
}

/// `Outcome::Acked`.
pub fn ok() -> Outcome {
    Outcome::Acked
}

/// `Outcome::Read`.
pub fn read(val: Option<&str>) -> Outcome {
    Outcome::Read { value: val.map(v) }
}

/// `Outcome::Failed` (sabit neden "io").
pub fn failed() -> Outcome {
    Outcome::Failed {
        reason: "io".into(),
    }
}

/// `Outcome::InFlight`.
pub fn inflight() -> Outcome {
    Outcome::InFlight
}

/// Kayıt listesinden geçerli bir geçmiş kurar (geçersizse test düşer).
pub fn hist(entries: Vec<(Op, Outcome)>) -> History {
    let entries = entries
        .into_iter()
        .map(|(op, outcome)| Entry { op, outcome })
        .collect();
    History::from_entries(entries).expect("test geçmişi geçerli olmalı")
}

/// (anahtar, değer) çiftlerinden durum.
pub fn state(pairs: &[(&str, &str)]) -> State {
    pairs.iter().map(|(a, b)| (k(a), v(b))).collect()
}

/// Garanti kısayolu.
pub fn g(d: Durability, atomic_txn: bool, prefix_order: bool) -> Guarantees {
    Guarantees {
        durability: d,
        atomic_txn,
        prefix_order,
    }
}

/// `Some(v(s))`.
pub fn some(s: &str) -> Option<Value> {
    Some(v(s))
}
