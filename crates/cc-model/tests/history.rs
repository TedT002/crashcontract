//! `History` sözleşme testleri (SPEC §6.2, H01–H11): yaşam döngüsü, değişmezler (I1/I2),
//! dayanıklılık tabanı, `crash_view` ve `state_at`.

mod common;
use cc_model::*;
use common::*;

// H01: Mutlu yol; Put -> Acked, Get -> Read, Sync -> Acked ve indeksler 0,1,2 döner.
#[test]
fn h01_happy_path_outcomes() {
    let mut h = History::new();
    assert_eq!(h.issue(put("a", "1")), Ok(0));
    h.ack(0).unwrap();
    assert_eq!(h.issue(get("a")), Ok(1));
    h.ack_read(1, some("1")).unwrap();
    assert_eq!(h.issue(Op::Sync), Ok(2));
    h.ack(2).unwrap();

    let outcomes: Vec<Outcome> = h.entries().iter().map(|e| e.outcome.clone()).collect();
    assert_eq!(
        outcomes,
        vec![
            Outcome::Acked,
            Outcome::Read { value: some("1") },
            Outcome::Acked
        ]
    );
    assert_eq!(h.len(), 3);
    assert!(!h.is_empty());
}

// H02: Bekleyen (Issued) işlem varken yeni işlem gönderilemez.
#[test]
fn h02_issue_while_pending_is_rejected() {
    let mut h = History::new();
    h.issue(put("a", "1")).unwrap();
    assert_eq!(
        h.issue(put("b", "2")),
        Err(HistoryError::PendingOp { index: 0 })
    );
}

// H03: Yanlış indeksli ack NotPending; doğru ack Ok; ikinci ack yine NotPending.
#[test]
fn h03_ack_wrong_index_and_double_ack() {
    let mut h = History::new();
    h.issue(put("a", "1")).unwrap();
    assert_eq!(h.ack(5), Err(HistoryError::NotPending { index: 5 }));
    assert_eq!(h.ack(0), Ok(()));
    assert_eq!(h.ack(0), Err(HistoryError::NotPending { index: 0 }));
}

// H04: I2 — Get için ack() ve Get olmayan için ack_read() OutcomeMismatch verir.
#[test]
fn h04_outcome_mismatch_on_ack_paths() {
    let mut h = History::new();
    h.issue(get("a")).unwrap();
    assert_eq!(h.ack(0), Err(HistoryError::OutcomeMismatch { index: 0 }));

    let mut h2 = History::new();
    h2.issue(put("a", "1")).unwrap();
    assert_eq!(
        h2.ack_read(0, None),
        Err(HistoryError::OutcomeMismatch { index: 0 })
    );
}

// H05: Failed geçmişi kapatır; sonra issue Closed döner.
#[test]
fn h05_failed_closes_history() {
    let mut h = History::new();
    h.issue(put("a", "1")).unwrap();
    h.fail(0, "io").unwrap();
    assert_eq!(h.issue(put("b", "2")), Err(HistoryError::Closed));
}

// H06: mark_crashed Issued'u InFlight yapar ve geçmişi kapatır; tamamen Acked geçmişte etkisizdir.
#[test]
fn h06_mark_crashed() {
    let mut h = History::new();
    h.issue(put("a", "1")).unwrap();
    h.mark_crashed();
    assert_eq!(h.entries()[0].outcome, Outcome::InFlight);
    assert_eq!(h.issue(put("b", "2")), Err(HistoryError::Closed));

    let mut acked = hist(vec![(put("a", "1"), ok()), (put("b", "2"), ok())]);
    let before = acked.clone();
    acked.mark_crashed();
    assert_eq!(acked, before);
}

// H07: I1 — belirsiz sonuç (InFlight/Failed/Issued) yalnız son kayıtta olabilir.
#[test]
fn h07_from_entries_uncertain_not_last() {
    let mk = |o: Outcome| {
        vec![
            Entry {
                op: put("a", "1"),
                outcome: o,
            },
            Entry {
                op: put("b", "2"),
                outcome: ok(),
            },
        ]
    };
    for o in [inflight(), failed(), Outcome::Issued] {
        assert_eq!(
            History::from_entries(mk(o)),
            Err(HistoryError::UncertainNotLast { index: 0 })
        );
    }
}

// H08: I2 — Put'a Read, Get'e Acked verilemez.
#[test]
fn h08_from_entries_outcome_mismatch() {
    let a = vec![Entry {
        op: put("a", "1"),
        outcome: read(Some("1")),
    }];
    let b = vec![Entry {
        op: get("a"),
        outcome: ok(),
    }];
    assert_eq!(
        History::from_entries(a),
        Err(HistoryError::OutcomeMismatch { index: 0 })
    );
    assert_eq!(
        History::from_entries(b),
        Err(HistoryError::OutcomeMismatch { index: 0 })
    );
}

// H09: durable_floor — OnAck n-1 (son belirsiz), OnSync son ACK'li Sync+1 (in-flight sync sayılmaz),
// NoGuarantee 0; tamamen ACK'li geçmişte OnAck = n.
#[test]
fn h09_durable_floor() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (Op::Sync, ok()),
        (put("b", "2"), ok()),
        (Op::Sync, inflight()),
    ]);
    assert_eq!(h.durable_floor(Durability::OnAck), 3);
    assert_eq!(h.durable_floor(Durability::OnSync), 2);
    assert_eq!(h.durable_floor(Durability::NoGuarantee), 0);

    let h2 = hist(vec![
        (put("a", "1"), ok()),
        (Op::Sync, ok()),
        (put("b", "2"), ok()),
    ]);
    assert_eq!(h2.durable_floor(Durability::OnAck), 3);
}

fn h10_base() -> History {
    hist(vec![
        (put("a", "1"), ok()),
        (Op::Sync, ok()),
        (put("b", "2"), ok()),
    ])
}

// H10: crash_view — in_flight ile son işlem InFlight olur; tam görünüm orijinale eşit; 0 -> boş.
#[test]
fn h10_crash_view() {
    let h = h10_base();
    assert_eq!(
        h.crash_view(2, true),
        hist(vec![
            (put("a", "1"), ok()),
            (Op::Sync, ok()),
            (put("b", "2"), inflight())
        ])
    );
    assert_eq!(h.crash_view(3, false), h);
    assert_eq!(h.crash_view(0, false), History::new());
}

// H10 (panic): completed > len() programlama hatasıdır.
#[test]
#[should_panic(expected = "completed (4) > len (3)")]
fn h10_crash_view_completed_out_of_range_panics() {
    let _ = h10_base().crash_view(4, false);
}

// H10 ek (SPEC §3.3 Panics): in_flight iken completed >= len() panic.
#[test]
#[should_panic(expected = "in_flight için completed (3) < len olmalı")]
fn h10_crash_view_inflight_at_len_panics() {
    let _ = h10_base().crash_view(3, true);
}

// H10 ek: completed == len iken in_flight=true önkoşulu ihlal eder -> panic.
// NOT: Panic I1 kontrolünden DEĞİL, `in_flight => completed < len` önkoşul assert'inden gelir;
// geçerli bir kaynak geçmişte (I1 gereği belirsiz kayıt yalnız sonda olur) `crash_view`'ın
// I1 yolu dışarıdan tetiklenemez, o yüzden orası savunma amaçlıdır ve burada sınanmaz.
#[test]
#[should_panic(expected = "in_flight için completed (2) < len olmalı")]
fn h10_crash_view_would_break_i1_panics() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), inflight())]);
    // completed=2 ve in_flight=true: [0,2) içinde belirsiz kayıt var.
    let _ = h.crash_view(2, true);
}

fn h11_base() -> History {
    hist(vec![
        (put("a", "1"), ok()),
        (txn(vec![wput("b", "2"), wdel("a")]), ok()),
        (put("c", "3"), ok()),
    ])
}

// H11: state_at — txn ortası (Cut{1,1}) yalnız ilk yazmayı içerir.
#[test]
fn h11_state_at() {
    let h = h11_base();
    assert_eq!(h.state_at(Cut::at_op(0)), state(&[]));
    assert_eq!(
        h.state_at(Cut {
            ops: 1,
            txn_writes: 1
        }),
        state(&[("a", "1"), ("b", "2")])
    );
    assert_eq!(h.state_at(Cut::at_op(2)), state(&[("b", "2")]));
    assert_eq!(h.state_at(Cut::at_op(3)), state(&[("b", "2"), ("c", "3")]));
}

// H11 (panic): txn_writes > 0 iken ilgili işlem Txn değil (op0 Put).
#[test]
#[should_panic(expected = "işlem Txn değil")]
fn h11_state_at_non_txn_with_txn_writes_panics() {
    let _ = h11_base().state_at(Cut {
        ops: 0,
        txn_writes: 1,
    });
}

// H11 ek: ops > len() panic.
#[test]
#[should_panic(expected = "cut.ops (4) > len (3)")]
fn h11_state_at_ops_out_of_range_panics() {
    let _ = h11_base().state_at(Cut::at_op(4));
}

// H11 ek: txn_writes >= writes.len() kanonik değildir -> panic.
#[test]
#[should_panic(expected = "txn_writes (2) >= writes.len() (2)")]
fn h11_state_at_txn_writes_too_large_panics() {
    let _ = h11_base().state_at(Cut {
        ops: 1,
        txn_writes: 2,
    });
}

// Regresyon: usize::MAX indeksi `index + 1` taşmasıyla panic ETMEMELİ, NotPending dönmeli
// (boş geçmişte de, bekleyen kayıt varken de). ack / ack_read / fail için.
#[test]
fn h03_huge_index_is_not_pending_not_overflow() {
    let mut empty = History::new();
    let e = HistoryError::NotPending { index: usize::MAX };
    assert_eq!(empty.ack(usize::MAX), Err(e.clone()));
    assert_eq!(empty.ack_read(usize::MAX, None), Err(e.clone()));
    assert_eq!(empty.fail(usize::MAX, "x"), Err(e.clone()));

    let mut h = History::new();
    h.issue(put("a", "1")).unwrap();
    assert_eq!(h.ack(usize::MAX), Err(e.clone()));
    assert_eq!(h.ack_read(usize::MAX, None), Err(e.clone()));
    assert_eq!(h.fail(usize::MAX, "x"), Err(e));
    // Hatalı çağrılar durumu bozmamalı.
    assert_eq!(h.ack(0), Ok(()));
}
