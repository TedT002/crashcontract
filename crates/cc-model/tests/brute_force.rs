//! Kaba kuvvet karşılaştırma testi: prefix modunun artımlı sayaçlı
//! algoritması, her izinli kesmenin `state_at` ile tek tek hesaplanıp R ile karşılaştırılmasıyla
//! (açıkça "yavaş ama aşikâr doğru" referans) aynı Allowed/Violated kararını ve aynı tanığı vermeli.
//! Rastgelelik: elle yazılmış SplitMix64 + sabit seed -> tamamen deterministik, flaky değil.

mod common;
use cc_model::*;
use common::*;

/// SplitMix64: küçük, iyi bilinen, dış bağımlılıksız PRNG.
struct SplitMix64(u64);
impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// [0, n) aralığında sayı.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const KEYS: [&str; 3] = ["a", "b", "c"];
const VALS: [&str; 3] = ["1", "2", "3"];
const SEED: u64 = 0xC0FF_EE15_BADC_0DE5;
const HISTORIES: usize = 1500;

fn rand_write(rng: &mut SplitMix64) -> Write {
    let key = KEYS[rng.below(3)];
    if rng.below(4) == 0 {
        wdel(key)
    } else {
        wput(key, VALS[rng.below(3)])
    }
}

fn rand_op(rng: &mut SplitMix64) -> Op {
    match rng.below(10) {
        0..=3 => put(KEYS[rng.below(3)], VALS[rng.below(3)]),
        4 => del(KEYS[rng.below(3)]),
        5 | 6 => get(KEYS[rng.below(3)]),
        7 | 8 => {
            let len = rng.below(4); // boş txn dahil
            txn((0..len).map(|_| rand_write(rng)).collect())
        }
        _ => Op::Sync,
    }
}

/// ≤8 işlemli geçmiş; son işlem ~1/3 olasılıkla belirsiz (Issued/InFlight/Failed).
fn rand_history(rng: &mut SplitMix64) -> History {
    let n = rng.below(9);
    let mut entries = Vec::new();
    for i in 0..n {
        let op = rand_op(rng);
        let outcome = if i + 1 == n && rng.below(3) == 0 {
            match rng.below(3) {
                0 => Outcome::Issued,
                1 => inflight(),
                _ => failed(),
            }
        } else if matches!(op, Op::Get { .. }) {
            // check_recovered okuma sonuçlarına bakmaz; rastgele değer de olsa sorun değil.
            read(if rng.below(2) == 0 {
                None
            } else {
                Some(VALS[rng.below(3)])
            })
        } else {
            ok()
        };
        entries.push((op, outcome));
    }
    hist(entries)
}

/// Dayanıklılık tabanı tanımının yeniden yazımı. DİKKAT: bu, `History::durable_floor` tanımının aynı mantıkla
/// ikinci bir yazılışıdır; ortak bir yanlış anlamayı yakalamaz, yalnızca iki ayrı yazımın
/// (kütüphane ↔ test) uyuştuğunu doğrular. Gerçek bağımsızlık sağlamaz.
fn ref_floor(h: &History, d: Durability) -> usize {
    let es = h.entries();
    match d {
        Durability::NoGuarantee => 0,
        Durability::OnAck => es
            .iter()
            .take_while(|e| matches!(e.outcome, Outcome::Acked | Outcome::Read { .. }))
            .count(),
        Durability::OnSync => es
            .iter()
            .rposition(|e| e.op == Op::Sync && e.outcome == Outcome::Acked)
            .map_or(0, |i| i + 1),
    }
}

/// Sıralı izinli kesme listesi.
fn ref_cuts(h: &History, f: usize, atomic: bool) -> Vec<Cut> {
    let mut cuts = vec![Cut::at_op(f)];
    for i in f..h.len() {
        if !atomic && let Op::Txn { writes } = &h.entries()[i].op {
            for j in 1..writes.len() {
                cuts.push(Cut {
                    ops: i,
                    txn_writes: j,
                });
            }
        }
        cuts.push(Cut::at_op(i + 1));
    }
    cuts
}

/// R: üç kaynaktan biri, sonra ~1/3 olasılıkla bozulma.
/// * ~1/2: izinli bir kesmenin durumu;
/// * ~1/2: geçmişin HER kesmesinin durumu — F'den küçük kesmeler (dayanıklılık tabanı altı) ve
///   atomik modda izinsiz olan txn-ortası kesmeler dahil. Böylece "taban altı" ve "yarım txn"
///   durumları tesadüfe kalmadan, kasıtlı olarak üretilir.
fn rand_recovered(rng: &mut SplitMix64, h: &History, cuts: &[Cut]) -> State {
    let mut r = if rng.below(2) == 0 {
        h.state_at(cuts[rng.below(cuts.len())])
    } else {
        let every = ref_cuts(h, 0, false);
        h.state_at(every[rng.below(every.len())])
    };
    if rng.below(3) == 0 {
        match rng.below(4) {
            0 => {
                r.remove(&k(KEYS[rng.below(3)]));
            }
            1 => {
                r.insert(k(KEYS[rng.below(3)]), v(VALS[rng.below(3)]));
            }
            2 => {
                r.insert(k(KEYS[rng.below(3)]), v("9")); // hiç yazılmamış değer
            }
            _ => {
                r.insert(k("z"), v("1")); // hiç yazılmamış anahtar
            }
        }
    }
    r
}

const DURABILITIES: [Durability; 3] = [
    Durability::OnAck,
    Durability::OnSync,
    Durability::NoGuarantee,
];

// Prefix modu: karar + tanık, kaba kuvvet referansıyla birebir aynı olmalı.
#[test]
fn brute_force_prefix_mode_matches_reference() {
    let mut rng = SplitMix64(SEED);
    let (mut n_allowed, mut n_violated, mut n_cases) = (0usize, 0usize, 0usize);
    for iter in 0..HISTORIES {
        let h = rand_history(&mut rng);
        for d in DURABILITIES {
            for atomic in [true, false] {
                let gg = g(d, atomic, true);
                let f = ref_floor(&h, d);
                let cuts = ref_cuts(&h, f, atomic);
                for _ in 0..2 {
                    let r = rand_recovered(&mut rng, &h, &cuts);
                    let expected_witness =
                        cuts.iter().rev().find(|c| h.state_at(**c) == r).copied();
                    let verdict = check_recovered(&h, gg, &r);
                    let ctx = format!(
                        "seed={SEED:#x} iter={iter} g={gg:?}\nhistory={:?}\nR={r:?}\nverdict={verdict:?}",
                        h.entries()
                    );
                    n_cases += 1;
                    match (expected_witness, &verdict) {
                        (Some(w), Verdict::Allowed { witness }) => {
                            assert_eq!(*witness, Some(w), "tanık kesme farklı\n{ctx}");
                            n_allowed += 1;
                        }
                        (None, Verdict::Violated(vi)) => {
                            assert_eq!(vi.durable_floor, Some(f), "durable_floor farklı\n{ctx}");
                            n_violated += 1;
                        }
                        _ => panic!(
                            "karar referansla uyuşmuyor (beklenen tanık: {expected_witness:?})\n{ctx}"
                        ),
                    }
                }
            }
        }
    }
    // Test kendi kendini doğrulasın: iki sınıf da gerçekten sınanmış olmalı.
    assert!(n_cases >= 500, "en az 500 vaka gerekli, {n_cases}");
    assert!(
        n_allowed > 100 && n_violated > 100,
        "dengesiz: allowed={n_allowed} violated={n_violated}"
    );
}

// Sağlamlık: prefix-izinli bir durum anahtar-bazlı modda ASLA Violated olmamalı
// (anahtar-bazlı kural prefix kuralından gevşektir; Allowed ise witness None).
#[test]
fn brute_force_keywise_mode_is_sound_wrt_prefix_allowed_states() {
    let mut rng = SplitMix64(SEED ^ 0x5555_5555_5555_5555);
    let mut n_cases = 0usize;
    for iter in 0..HISTORIES {
        let h = rand_history(&mut rng);
        for d in DURABILITIES {
            for atomic in [true, false] {
                let gg = g(d, atomic, false);
                let f = ref_floor(&h, d);
                let cuts = ref_cuts(&h, f, atomic);
                for c in &cuts {
                    let r = h.state_at(*c);
                    let verdict = check_recovered(&h, gg, &r);
                    n_cases += 1;
                    assert_eq!(
                        verdict,
                        Verdict::Allowed { witness: None },
                        "izinli kesme {c:?} anahtar-bazlı modda reddedildi\nseed={SEED:#x} iter={iter} g={gg:?}\nhistory={:?}\nR={r:?}",
                        h.entries()
                    );
                }
            }
        }
    }
    assert!(n_cases >= 500);
}

// ---------------------------------------------------------------------------------------------
// Anahtar-bazlı mod sağlamlığı: prefix-dışı ama "anahtar başına" izinli durumlar
// ---------------------------------------------------------------------------------------------

/// Kütüphaneden bağımsız adım listesi: (anahtar, yeni değer; None = silme). `steps_of` / `state_at`
/// KULLANILMAZ; yalnız Op'un public alanları okunur.
type RefStep = (Vec<u8>, Option<Vec<u8>>);

fn ref_steps(op: &Op) -> Vec<RefStep> {
    match op {
        Op::Put { key, value } => vec![(key.clone(), Some(value.clone()))],
        Op::Delete { key } => vec![(key.clone(), None)],
        Op::Txn { writes } => writes
            .iter()
            .map(|w| match w {
                Write::Put { key, value } => (key.clone(), Some(value.clone())),
                Write::Delete { key } => (key.clone(), None),
            })
            .collect(),
        Op::Get { .. } | Op::Sync => Vec::new(),
    }
}

fn ref_apply(st: &mut State, (key, val): &RefStep) {
    match val {
        Some(x) => {
            st.insert(key.clone(), x.clone());
        }
        None => {
            st.remove(key);
        }
    }
}

// atomic_txn=true: [F,n) işlemlerinin HER alt kümesi A (her işlem tam uygulanır, sırayla) anahtar-bazlı
// modda izinlidir: her anahtarın değeri ya taban değeri ya da [F,n)'de o anahtara yazılan bir değerdir
// (txn'in son yazması), ve hiçbir txn yarım değildir. Bu, prefix-dışı (ör. ara işlem atlanmış) durumları da kapsar.
#[test]
fn brute_force_keywise_atomic_all_op_subsets_are_allowed() {
    let mut rng = SplitMix64(SEED ^ 0x1111_2222_3333_4444);
    let mut n_cases = 0usize;
    for iter in 0..HISTORIES {
        let h = rand_history(&mut rng);
        for d in DURABILITIES {
            let f = ref_floor(&h, d);
            let mut base = State::new();
            for e in &h.entries()[..f] {
                for st in ref_steps(&e.op) {
                    ref_apply(&mut base, &st);
                }
            }
            let m = h.len() - f; // n ≤ 8 ⇒ ≤ 2^8 alt küme
            for mask in 0u32..(1 << m) {
                let mut r = base.clone();
                for (off, e) in h.entries()[f..].iter().enumerate() {
                    if mask >> off & 1 == 1 {
                        for st in ref_steps(&e.op) {
                            ref_apply(&mut r, &st);
                        }
                    }
                }
                let verdict = check_recovered(&h, g(d, true, false), &r);
                n_cases += 1;
                assert_eq!(
                    verdict,
                    Verdict::Allowed { witness: None },
                    "alt küme mask={mask:#b} reddedildi\nseed={SEED:#x} iter={iter} d={d:?} F={f}\nhistory={:?}\nR={r:?}",
                    h.entries()
                );
            }
        }
    }
    assert!(n_cases >= 500);
}

// atomic_txn=false: [F,n) aralığındaki ADIMların rastgele alt kümeleri (txn içi adımlar bağımsız)
// anahtar-bazlı modda izinlidir.
#[test]
fn brute_force_keywise_nonatomic_random_step_subsets_are_allowed() {
    let mut rng = SplitMix64(SEED ^ 0x7777_8888_9999_AAAA);
    let mut n_cases = 0usize;
    for iter in 0..HISTORIES {
        let h = rand_history(&mut rng);
        for d in DURABILITIES {
            let f = ref_floor(&h, d);
            let mut base = State::new();
            for e in &h.entries()[..f] {
                for st in ref_steps(&e.op) {
                    ref_apply(&mut base, &st);
                }
            }
            let steps: Vec<RefStep> = h.entries()[f..]
                .iter()
                .flat_map(|e| ref_steps(&e.op))
                .collect();
            for _ in 0..16 {
                let mut r = base.clone();
                for st in &steps {
                    if rng.below(2) == 0 {
                        ref_apply(&mut r, st);
                    }
                }
                let verdict = check_recovered(&h, g(d, false, false), &r);
                n_cases += 1;
                assert_eq!(
                    verdict,
                    Verdict::Allowed { witness: None },
                    "adım alt kümesi reddedildi\nseed={SEED:#x} iter={iter} d={d:?} F={f}\nhistory={:?}\nR={r:?}",
                    h.entries()
                );
            }
        }
    }
    assert!(n_cases >= 500);
}

// ---------------------------------------------------------------------------------------------
// Violated sonuçlarının iç tutarlılık değişmezleri (rastgele girdi, tüm garanti kombinasyonları)
// ---------------------------------------------------------------------------------------------

#[test]
fn brute_force_violation_invariants() {
    let mut rng = SplitMix64(SEED ^ 0xDEAD_BEEF_0BAD_F00D);
    let mut kinds_seen = std::collections::BTreeSet::new();
    let mut n_violated = 0usize;
    for iter in 0..HISTORIES {
        let h = rand_history(&mut rng);
        let n = h.len();
        for d in DURABILITIES {
            for atomic in [true, false] {
                for prefix in [true, false] {
                    let gg = g(d, atomic, prefix);
                    let f = ref_floor(&h, d);
                    let cuts = ref_cuts(&h, f, atomic);
                    let r = rand_recovered(&mut rng, &h, &cuts);
                    let Verdict::Violated(vi) = check_recovered(&h, gg, &r) else {
                        continue;
                    };
                    let ctx = format!(
                        "seed={SEED:#x} iter={iter} g={gg:?}\nhistory={:?}\nR={r:?}\nviolation={vi:?}",
                        h.entries()
                    );
                    n_violated += 1;
                    kinds_seen.insert(vi.kind);

                    // 1) bulunan değer beklenenler arasında olamaz
                    assert!(!vi.expected.contains(&vi.found), "found ∈ expected\n{ctx}");
                    // 2) expected sıralı ve tekrarsız
                    assert!(
                        vi.expected.windows(2).all(|w| w[0] < w[1]),
                        "expected sıralı/tekrarsız değil\n{ctx}"
                    );
                    // 3) found, gerçekten R'deki değer
                    assert_eq!(vi.found.as_ref(), r.get(&vi.key), "found ≠ R[key]\n{ctx}");
                    // 4) op_index: geçmişte ve o işlem gerçekten bu anahtara yazıyor
                    match (vi.kind, vi.op_index) {
                        (ViolationKind::PhantomValue, idx) => {
                            assert_eq!(idx, None, "PhantomValue'da op_index olmamalı\n{ctx}")
                        }
                        (_, Some(i)) => {
                            assert!(i < n, "op_index ≥ n\n{ctx}");
                            assert!(
                                ref_steps(&h.entries()[i].op)
                                    .iter()
                                    .any(|(kk, _)| *kk == vi.key),
                                "op_index işlemi anahtara yazmıyor\n{ctx}"
                            );
                        }
                        (_, None) => panic!("op_index eksik\n{ctx}"),
                    }
                    // 5) best_cut yoksa: R[key], izinli kesmelerdeki hiçbir val_key değerine eşit
                    //    olamaz. PartialTxn HARİÇ: o sınıfta `key` "uygulanmamış görünen" anahtardır
                    //    ve R[key] meşru bir eski değer (ör. taban) olabilir; ihlal anahtarın
                    //    kendisinde değil txn bütünlüğündedir.
                    if vi.best_cut.is_none() && vi.kind != ViolationKind::PartialTxn {
                        for c in &cuts {
                            assert_ne!(
                                h.state_at(*c).get(&vi.key),
                                r.get(&vi.key),
                                "best_cut=None ama R[key] kesme {c:?} değeriyle eşit\n{ctx}"
                            );
                        }
                    }
                    // 6) PrefixViolation: best_cut var ve orada key farklı
                    if vi.kind == ViolationKind::PrefixViolation {
                        let bc = vi
                            .best_cut
                            .expect("PrefixViolation'da best_cut Some olmalı");
                        assert_ne!(
                            h.state_at(bc).get(&vi.key),
                            r.get(&vi.key),
                            "best_cut'ta key zaten eşit\n{ctx}"
                        );
                    }
                }
            }
        }
    }
    assert!(n_violated > 500, "yetersiz ihlal: {n_violated}");
    assert!(
        kinds_seen.len() >= 4,
        "ihlal çeşitliliği düşük: {kinds_seen:?}"
    );
}
