//! `check_recovered` sözleşme testleri (SPEC §6.2, R01–R36). Her test, çökme sonrası kurtarılan
//! durumun motorun BİLDİRDİĞİ garantilere göre izinli olup olmadığına dair tek bir kuralı doğrular.

mod common;
use cc_model::Durability::{NoGuarantee, OnAck, OnSync};
use cc_model::*;
use common::*;

/// ACK = g(OnAck,true,true)
fn ack_g() -> Guarantees {
    g(OnAck, true, true)
}
/// SYNC = g(OnSync,true,true)
fn sync_g() -> Guarantees {
    g(OnSync, true, true)
}
/// NONE = g(NoGuarantee,true,true)
fn none_g() -> Guarantees {
    g(NoGuarantee, true, true)
}

fn c(n: usize) -> Cut {
    Cut::at_op(n)
}
fn allowed(cut: Cut) -> Verdict {
    Verdict::Allowed { witness: Some(cut) }
}
fn allowed_nowitness() -> Verdict {
    Verdict::Allowed { witness: None }
}
fn viol(
    kind: ViolationKind,
    key: &str,
    expected: Vec<Option<Value>>,
    found: Option<Value>,
    op_index: Option<usize>,
    best_cut: Option<Cut>,
    floor: Option<usize>,
) -> Verdict {
    Verdict::Violated(Violation {
        kind,
        key: k(key),
        expected,
        found,
        op_index,
        best_cut,
        durable_floor: floor,
    })
}

use ViolationKind::*;

// R01: Boş geçmiş + boş durum -> Allowed(C0).
#[test]
fn r01_empty_history_empty_state() {
    assert_eq!(
        check_recovered(&hist(vec![]), sync_g(), &state(&[])),
        allowed(c(0))
    );
}

// R02: Boş geçmişte hiç yazılmamış anahtar belirdi -> PhantomValue.
#[test]
fn r02_empty_history_phantom() {
    assert_eq!(
        check_recovered(&hist(vec![]), sync_g(), &state(&[("a", "1")])),
        viol(
            PhantomValue,
            "a",
            vec![None],
            some("1"),
            None,
            None,
            Some(0)
        )
    );
}

// R03: OnAck, tüm yazmalar kurtarıldı -> Allowed(C2).
#[test]
fn r03_onack_all_recovered() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), ok())]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1"), ("b", "2")])),
        allowed(c(2))
    );
}

// R04: OnAck'te ACK'li yazma kayboldu -> LostDurableWrite.
#[test]
fn r04_onack_lost_acked_write() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), ok())]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1")])),
        viol(
            LostDurableWrite,
            "b",
            vec![some("2")],
            None,
            Some(1),
            None,
            Some(2)
        )
    );
}

// R05: Yarıda kalan işlem uygulanmış olabilir.
#[test]
fn r05_inflight_applied_allowed() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), inflight())]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1"), ("b", "2")])),
        allowed(c(2))
    );
}

// R06: Yarıda kalan işlem uygulanmamış olabilir.
#[test]
fn r06_inflight_not_applied_allowed() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), inflight())]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1")])),
        allowed(c(1))
    );
}

// R07: Failed işlemin etkisi belirsiz: ikisi de izinli.
#[test]
fn r07_failed_op_both_outcomes_allowed() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), failed())]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1")])),
        allowed(c(1))
    );
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1"), ("b", "2")])),
        allowed(c(2))
    );
}

// R08: OnSync, Sync sonrası yazma (b) kaybolabilir; taban 2.
#[test]
fn r08_onsync_post_sync_write_may_be_lost() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (Op::Sync, ok()),
        (put("b", "2"), ok()),
    ]);
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("a", "1")])),
        allowed(c(2))
    );
}

// R09: Sync'lenmiş yazma kayboldu -> LostDurableWrite.
#[test]
fn r09_onsync_synced_write_lost() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (Op::Sync, ok()),
        (put("b", "2"), ok()),
    ]);
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[])),
        viol(
            LostDurableWrite,
            "a",
            vec![some("1")],
            None,
            Some(0),
            None,
            Some(2)
        )
    );
}

// R10: OnSync'te hiç Sync yoksa her şey kaybolabilir.
#[test]
fn r10_onsync_no_sync_all_may_be_lost() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), ok())]);
    assert_eq!(check_recovered(&h, sync_g(), &state(&[])), allowed(c(0)));
}

// R11: NoGuarantee, Sync'e rağmen boş durum izinli.
#[test]
fn r11_noguarantee_sync_does_not_pin() {
    let h = hist(vec![(put("a", "1"), ok()), (Op::Sync, ok())]);
    assert_eq!(check_recovered(&h, none_g(), &state(&[])), allowed(c(0)));
}

// R12: NoGuarantee bile hiç yazılmamış değeri (9) kabul etmez.
#[test]
fn r12_noguarantee_phantom_value() {
    let h = hist(vec![(put("a", "1"), ok())]);
    assert_eq!(
        check_recovered(&h, none_g(), &state(&[("a", "9")])),
        viol(
            PhantomValue,
            "a",
            vec![None, some("1")],
            some("9"),
            None,
            None,
            Some(0)
        )
    );
}

// R13: Prefix ihlali: b (sonraki yazma) var, a (önceki) yok.
#[test]
fn r13_prefix_violation_lost_earlier_write() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), ok())]);
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("b", "2")])),
        viol(
            PrefixViolation,
            "a",
            vec![some("1")],
            None,
            Some(0),
            Some(c(2)),
            Some(0)
        )
    );
}

// R14: prefix_order=false -> anahtar-bazlı kural aynı durumu kabul eder; witness yok.
#[test]
fn r14_keywise_mode_accepts_out_of_order_loss() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), ok())]);
    assert_eq!(
        check_recovered(&h, g(OnSync, true, false), &state(&[("b", "2")])),
        allowed_nowitness()
    );
}

// R15: Anahtar-bazlı modda da dayanıklı taban korunur.
#[test]
fn r15_keywise_mode_still_enforces_floor() {
    let h = hist(vec![(put("a", "1"), ok())]);
    assert_eq!(
        check_recovered(&h, g(OnAck, true, false), &state(&[])),
        viol(
            LostDurableWrite,
            "a",
            vec![some("1")],
            None,
            Some(0),
            None,
            Some(1)
        )
    );
}

// R16: Atomik txn ya hep ya hiç: {} -> C0, tam -> C1.
#[test]
fn r16_atomic_txn_all_or_nothing() {
    let h = hist(vec![(txn(vec![wput("a", "1"), wput("b", "2")]), ok())]);
    assert_eq!(check_recovered(&h, sync_g(), &state(&[])), allowed(c(0)));
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("a", "1"), ("b", "2")])),
        allowed(c(1))
    );
}

// R17: Atomik txn kısmen görünüyor -> PartialTxn (çapraz-anahtar sınıflandırma).
#[test]
fn r17_atomic_txn_partial() {
    let h = hist(vec![(txn(vec![wput("a", "1"), wput("b", "2")]), ok())]);
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("a", "1")])),
        viol(
            PartialTxn,
            "b",
            vec![some("2")],
            None,
            Some(0),
            Some(c(1)),
            Some(0)
        )
    );
}

// R18: Atomik olmayan txn: txn içi kesme izinli, tanık Cut{0,1}.
#[test]
fn r18_non_atomic_txn_mid_cut_allowed() {
    let h = hist(vec![(txn(vec![wput("a", "1"), wput("b", "2")]), ok())]);
    assert_eq!(
        check_recovered(&h, g(OnSync, false, true), &state(&[("a", "1")])),
        allowed(Cut {
            ops: 0,
            txn_writes: 1
        })
    );
}

// R19: Taban 1 (OnAck, son işlem belirsiz); in-flight atomik txn kısmen uygulanmış -> PartialTxn.
#[test]
fn r19_inflight_atomic_txn_partial() {
    let h = hist(vec![
        (put("c", "0"), ok()),
        (txn(vec![wput("a", "1"), wput("b", "2")]), inflight()),
    ]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("c", "0"), ("a", "1")])),
        viol(
            PartialTxn,
            "b",
            vec![some("2")],
            None,
            Some(1),
            Some(c(2)),
            Some(1)
        )
    );
}

// R20: ACK'li atomik txn'in b yazması kayıp: anahtar-bazlı sınıf önceliği (LostDurableWrite).
#[test]
fn r20_keywise_class_has_priority_over_partial_txn() {
    let h = hist(vec![(txn(vec![wput("a", "1"), wput("b", "2")]), ok())]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1")])),
        viol(
            LostDurableWrite,
            "b",
            vec![some("2")],
            None,
            Some(0),
            None,
            Some(1)
        )
    );
}

// R21: Sync sonrası üst üste yazmalar: taban değeri 1; 1/2/3 izinli, {} değil.
#[test]
fn r21_overwrites_after_sync() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (Op::Sync, ok()),
        (put("a", "2"), ok()),
        (put("a", "3"), ok()),
    ]);
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("a", "2")])),
        allowed(c(3))
    );
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("a", "1")])),
        allowed(c(2))
    );
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[])),
        viol(
            LostDurableWrite,
            "a",
            vec![some("1"), some("2"), some("3")],
            None,
            Some(0),
            None,
            Some(2)
        )
    );
}

// R22: Dayanıklı taban 2 değerli; eski değer (1) geri geldi -> LostDurableWrite.
#[test]
fn r22_stale_value_below_floor() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (put("a", "2"), ok()),
        (Op::Sync, ok()),
    ]);
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("a", "1")])),
        viol(
            LostDurableWrite,
            "a",
            vec![some("2")],
            some("1"),
            Some(1),
            None,
            Some(3)
        )
    );
}

// R23: Silinen anahtar geri geldi -> ResurrectedDelete (tombstone kaybı).
#[test]
fn r23_resurrected_delete() {
    let h = hist(vec![(put("a", "1"), ok()), (del("a"), ok())]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1")])),
        viol(
            ResurrectedDelete,
            "a",
            vec![None],
            some("1"),
            Some(1),
            None,
            Some(2)
        )
    );
}

// R24: Silme Sync sonrası: eski değer de, silinmiş hali de izinli.
#[test]
fn r24_delete_after_sync_either_state() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (Op::Sync, ok()),
        (del("a"), ok()),
    ]);
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("a", "1")])),
        allowed(c(2))
    );
    assert_eq!(check_recovered(&h, sync_g(), &state(&[])), allowed(c(3)));
}

// R25: Put-Delete-Put; taban 3. Yalnız {a:2} izinli; eski değer ve silinmiş hal LostDurableWrite.
#[test]
fn r25_put_delete_put_floor() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (del("a"), ok()),
        (put("a", "2"), ok()),
    ]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "2")])),
        allowed(c(3))
    );
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1")])),
        viol(
            LostDurableWrite,
            "a",
            vec![some("2")],
            some("1"),
            Some(2),
            None,
            Some(3)
        )
    );
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[])),
        viol(
            LostDurableWrite,
            "a",
            vec![some("2")],
            None,
            Some(2),
            None,
            Some(3)
        )
    );
}

// R26: Taban 2 (silme dayanıklı); belirsiz Put(2) uygulanmış olabilir; 1 geri gelirse ResurrectedDelete.
#[test]
fn r26_delete_then_inflight_put() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (del("a"), ok()),
        (put("a", "2"), inflight()),
    ]);
    assert_eq!(check_recovered(&h, ack_g(), &state(&[])), allowed(c(2)));
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "2")])),
        allowed(c(3))
    );
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1")])),
        viol(
            ResurrectedDelete,
            "a",
            vec![None, some("2")],
            some("1"),
            Some(1),
            None,
            Some(2)
        )
    );
}

// R27: b için başka anahtarın değeri (1) belirdi -> PhantomValue.
#[test]
fn r27_phantom_wrong_value_for_key() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), ok())]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1"), ("b", "1")])),
        viol(
            PhantomValue,
            "b",
            vec![some("2")],
            some("1"),
            None,
            None,
            Some(2)
        )
    );
}

// R28: Hiç yazılmamış anahtar (z) -> PhantomValue.
#[test]
fn r28_phantom_unknown_key() {
    let h = hist(vec![(put("a", "1"), ok())]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1"), ("z", "5")])),
        viol(
            PhantomValue,
            "z",
            vec![None],
            some("5"),
            None,
            None,
            Some(1)
        )
    );
}

// R29: Anahtar-bazlı mod + atomik txn: sağlam kontrol kısmi txn'i yakalar; boş durum izinli.
#[test]
fn r29_keywise_atomic_txn_sound_check() {
    let h = hist(vec![(txn(vec![wput("a", "1"), wput("b", "2")]), ok())]);
    let gg = g(OnSync, true, false);
    assert_eq!(
        check_recovered(&h, gg, &state(&[("a", "1")])),
        viol(
            PartialTxn,
            "b",
            vec![some("2")],
            None,
            Some(0),
            None,
            Some(0)
        )
    );
    assert_eq!(check_recovered(&h, gg, &state(&[])), allowed_nowitness());
}

// R30: Aynı anahtara txn içi çift yazma: atomikte ara değer (1) izinsiz, atomik değilken izinli.
#[test]
fn r30_double_write_in_txn() {
    let h = hist(vec![(txn(vec![wput("a", "1"), wput("a", "2")]), ok())]);
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("a", "1")])),
        viol(
            PartialTxn,
            "a",
            vec![None, some("2")],
            some("1"),
            Some(0),
            None,
            Some(0)
        )
    );
    assert_eq!(
        check_recovered(&h, g(OnSync, false, true), &state(&[("a", "1")])),
        allowed(Cut {
            ops: 0,
            txn_writes: 1
        })
    );
}

// R31: Get de kesme üretir; tanık aynı durumun EN SON kesmesidir (C2).
#[test]
fn r31_witness_is_latest_matching_cut() {
    let h = hist(vec![(put("a", "1"), ok()), (get("a"), read(Some("1")))]);
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("a", "1")])),
        allowed(c(2))
    );
}

// R32: Determinizm: aynı girdi iki kez -> eşit Verdict.
#[test]
fn r32_determinism() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), ok())]);
    let r = state(&[("b", "2")]);
    assert_eq!(
        check_recovered(&h, sync_g(), &r),
        check_recovered(&h, sync_g(), &r)
    );
}

// R33: Yarıda kalan Sync tabanı yükseltmez; boş durum izinli.
#[test]
fn r33_inflight_sync_does_not_raise_floor() {
    let h = hist(vec![(put("a", "1"), ok()), (Op::Sync, inflight())]);
    assert_eq!(check_recovered(&h, sync_g(), &state(&[])), allowed(c(0)));
}

// R34: Hiç garanti yok + prefix yok -> tek anahtar kalsa da izinli.
#[test]
fn r34_no_guarantees_at_all() {
    let h = hist(vec![(put("a", "1"), ok()), (put("b", "2"), ok())]);
    assert_eq!(
        check_recovered(&h, g(NoGuarantee, false, false), &state(&[("b", "2")])),
        allowed_nowitness()
    );
}

// R35: Issued (mark_crashed çağrılmamış) belirsiz muamelesi görür.
#[test]
fn r35_issued_is_uncertain() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (put("b", "2"), Outcome::Issued),
    ]);
    assert_eq!(
        check_recovered(&h, ack_g(), &state(&[("a", "1")])),
        allowed(c(1))
    );
}

// R36: Sıra dışı kayıp: b=3 var ama a=2 yok (a=4 var). En az uyuşmazlık, en son kesme C5.
#[test]
fn r36_prefix_violation_with_overwrites() {
    let h = hist(vec![
        (put("a", "1"), ok()),
        (Op::Sync, ok()),
        (put("a", "2"), ok()),
        (put("b", "3"), ok()),
        (put("a", "4"), ok()),
    ]);
    assert_eq!(
        check_recovered(&h, sync_g(), &state(&[("a", "4")])),
        viol(
            PrefixViolation,
            "b",
            vec![some("3")],
            None,
            Some(3),
            Some(c(5)),
            Some(2)
        )
    );
}
