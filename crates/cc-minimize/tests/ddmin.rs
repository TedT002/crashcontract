//! ddmin birim testleri (SPEC_stage6 §2): bilinen minimum, 1-minimallik, boş girdi, determinizm.

use cc_minimize::ddmin;

/// "Hata" = girdi 3 ve 7'yi birlikte içeriyor.
fn needs_3_and_7(xs: &[u32]) -> bool {
    xs.contains(&3) && xs.contains(&7)
}

/// Bilinen minimuma iner ve sırayı korur.
#[test]
fn finds_known_minimum() {
    let input: Vec<u32> = (0..20).collect();
    let mut calls = 0;
    let out = ddmin(&input, &mut |xs| {
        calls += 1;
        needs_3_and_7(xs)
    });
    assert_eq!(out, vec![3, 7]);
    assert!(calls < 100, "çok fazla test: {calls}");
}

/// 1-minimallik: sonuçtan herhangi bir eleman çıkarılınca test başarısız olmaz.
#[test]
fn result_is_one_minimal() {
    // Daha karmaşık bir "hata": çift sayıların toplamı ≥ 30 ve 13 içeriyor.
    let test =
        |xs: &[u32]| xs.contains(&13) && xs.iter().filter(|x| *x % 2 == 0).sum::<u32>() >= 30;
    let input: Vec<u32> = (0..25).collect();
    let out = ddmin(&input, &mut |xs| test(xs));
    assert!(test(&out));
    for i in 0..out.len() {
        let mut fewer = out.clone();
        fewer.remove(i);
        assert!(
            !test(&fewer),
            "{out:?} 1-minimal değil: {i}. eleman çıkarılabilir"
        );
    }
}

/// Boş dizi bile başarısızsa sonuç boş; tek elemanlı hata tek elemana iner.
#[test]
fn empty_and_single() {
    let input: Vec<u32> = (0..10).collect();
    assert_eq!(ddmin(&input, &mut |_| true), Vec::<u32>::new());
    assert_eq!(ddmin(&input, &mut |xs| xs.contains(&9)), vec![9]);
    assert_eq!(ddmin(&[5u32], &mut |xs| xs.contains(&5)), vec![5]);
}

/// Aynı girdi → aynı sonuç ve aynı test dizisi.
#[test]
fn deterministic() {
    let input: Vec<u32> = (0..40).collect();
    let run = || {
        let mut seen = Vec::new();
        let out = ddmin(&input, &mut |xs| {
            seen.push(xs.to_vec());
            xs.contains(&11) && xs.contains(&29) && xs.contains(&30)
        });
        (out, seen)
    };
    assert_eq!(run(), run());
    assert_eq!(run().0, vec![11, 29, 30]);
}
