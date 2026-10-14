//! `check_reads` sözleşme testleri (D01–D06): çalışma sırasındaki Get'ler
//! kendinden önceki tüm yazmaların uygulanmış haline eşit olmalı (read-your-writes).

mod common;
use cc_model::*;
use common::*;

fn allowed() -> Verdict {
    Verdict::Allowed { witness: None }
}

// D01: Put sonrası okuma, Delete sonrası okuma doğru -> Allowed.
#[test]
fn d01_consistent_reads_allowed() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (get("a"), read(Some("1"))),
        (del("a"), ok()),
        (get("a"), read(None)),
    ]);
    assert_eq!(check_reads(&h), allowed());
}

// D02: Yazılmış değer okunamadı -> ReadMismatch (tüm alanlar).
#[test]
fn d02_read_missing_value_is_mismatch() {
    let h = hist(vec![(put("a", "1"), ok()), (get("a"), read(None))]);
    assert_eq!(
        check_reads(&h),
        Verdict::Violated(Violation {
            kind: ViolationKind::ReadMismatch,
            key: k("a"),
            expected: vec![some("1")],
            found: None,
            op_index: Some(1),
            best_cut: None,
            durable_floor: None,
        })
    );
}

// D03: Txn yazmaları (put + delete) okumalara yansır.
#[test]
fn d03_txn_effects_visible_to_reads() {
    let h = hist(vec![
        (put("b", "0"), ok()),
        (txn(vec![wput("a", "1"), wdel("b")]), ok()),
        (get("b"), read(None)),
        (get("a"), read(Some("1"))),
    ]);
    assert_eq!(check_reads(&h), allowed());
}

// D04: Boş geçmiş -> Allowed.
#[test]
fn d04_empty_history_allowed() {
    assert_eq!(check_reads(&History::new()), allowed());
}

// D05: Başarısız Get kontrol edilmez.
#[test]
fn d05_failed_get_not_checked() {
    let h = hist(vec![(put("a", "1"), ok()), (get("a"), failed())]);
    assert_eq!(check_reads(&h), allowed());
}

// D06: Eski değer okundu (üzerine yazma görülmedi) -> ReadMismatch, op_index = Get'in indeksi.
#[test]
fn d06_stale_read_is_mismatch() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (put("a", "2"), ok()),
        (get("a"), read(Some("1"))),
    ]);
    assert_eq!(
        check_reads(&h),
        Verdict::Violated(Violation {
            kind: ViolationKind::ReadMismatch,
            key: k("a"),
            expected: vec![some("2")],
            found: some("1"),
            op_index: Some(2),
            best_cut: None,
            durable_floor: None,
        })
    );
}
