//! Hüküm: kurtarılan durum izinli kümede mi?
//!
//! ## Tasarım özeti
//!
//! * **Prefix modu** (`prefix_order`): izinli küme, `F` ile `n` arasındaki kesmelerin durumlarıdır.
//!   Üstel açık listeleme yerine tek geçişlik artımlı bir **uyuşmazlık sayacı** kullanılır.
//! * **Anahtar-bazlı mod**: her anahtar bağımsız olarak kendi aday değerlerinden birini taşımalıdır.
//! * Hüküm (Allowed/Violated) yalnız üyelik testinden çıkar; ihlal **sınıflandırması** sadece
//!   teşhistir ve hükmü asla değiştirmez.

use crate::op::{apply_step, steps_of};
use crate::{Cut, Guarantees, History, Key, Op, State, Value, Verdict, Violation, ViolationKind};
use std::collections::{BTreeMap, BTreeSet};

/// Çökme sonrası kurtarılan durum, motorun bildirdiği garantilere göre izinli mi?
/// `history` son kaydı Issued/InFlight/Failed olabilir (üçü de "belirsiz" muamelesi görür).
/// Saf fonksiyon; aynı girdi → aynı Verdict.
pub fn check_recovered(history: &History, guarantees: Guarantees, recovered: &State) -> Verdict {
    let floor = history.durable_floor(guarantees.durability);
    let atomic = guarantees.atomic_txn;

    if guarantees.prefix_order {
        let scan = prefix_scan(history, floor, atomic, recovered);
        if let Some(witness) = scan.last_match {
            return Verdict::Allowed {
                witness: Some(witness),
            };
        }
        // Buraya gelindiyse hiçbir kesme eşleşmedi: ihlal kesin. Aşağısı yalnız teşhis.
        // Önce anahtar-bazlı (tek anahtarı açıklayan) sınıflar, sonra çapraz-anahtar.
        if let Some(v) = key_based_violation(history, floor, atomic, recovered) {
            return Verdict::Violated(v);
        }
        return Verdict::Violated(classify_cross_key(
            history,
            floor,
            atomic,
            recovered,
            scan.best_cut,
        ));
    }

    if let Some(v) = key_based_violation(history, floor, atomic, recovered) {
        return Verdict::Violated(v);
    }
    if atomic && let Some(v) = atomic_txn_violation(history, floor, recovered) {
        return Verdict::Violated(v);
    }
    Verdict::Allowed { witness: None }
}

/// Çalışma sırasındaki (çökme öncesi) Get sonuçlarını doğrular. Tek iş parçacıklı sıralı yükte
/// her Get, kendinden önceki tüm yazmaların uygulanmış haline eşit olmalıdır (read-your-writes).
/// Bu kural garantilerden bağımsızdır (dayanıklılık yalnız çökmeyi ilgilendirir), o yüzden
/// Guarantees almaz. İlk uyuşmazlıkta döner; yoksa `Allowed { witness: None }`.
pub fn check_reads(history: &History) -> Verdict {
    let mut state = State::new();
    for (i, e) in history.entries().iter().enumerate() {
        // Failed/Issued/InFlight Get'in döndürdüğü bir değer yoktur; kontrol edilmez.
        if let (Op::Get { key }, crate::Outcome::Read { value }) = (&e.op, &e.outcome)
            && state.get(key) != value.as_ref()
        {
            return Verdict::Violated(Violation {
                kind: ViolationKind::ReadMismatch,
                key: key.clone(),
                expected: vec![state.get(key).cloned()],
                found: value.clone(),
                op_index: Some(i),
                best_cut: None,
                durable_floor: None,
            });
        }
        for (k, v) in steps_of(&e.op) {
            apply_step(&mut state, k, v);
        }
    }
    Verdict::Allowed { witness: None }
}

// ---------------------------------------------------------------------------------------------
// Prefix modu: artımlı uyuşmazlık sayacı
// ---------------------------------------------------------------------------------------------

struct PrefixScan {
    /// Durumu `R`'ye eşit olan EN SON izinli kesme.
    last_match: Option<Cut>,
    /// En az uyuşmazlıklı kesme (eşitlikte en sonuncusu); teşhis için.
    best_cut: Cut,
}

/// Kesme yürütücüsünün olayları. Adımlar ve kesmeler tek bir akışta, geçmişteki sırayla gelir.
enum CutEvent<'a> {
    /// Bir yazma adımı uygulanıyor (anahtar, yeni değer; None = silme).
    Step(&'a Key, Option<&'a Value>),
    /// Adımlar bu kesmeye kadar uygulandı; kesme izinli kümenin bir üyesidir.
    Cut(Cut),
}

/// İzinli kesmelerin TEK üreticisi (SPEC §4.2): `Cut{F,0}` ile başlar; `[F, n)` işlemlerini sırayla
/// gezer; atomik olmayan modda txn içindeki her ara adımdan sonra da bir kesme üretir; her
/// işlemden sonra `at_op(i+1)` üretir.
///
/// NEDEN tek yürütücü: prefix taraması (tanık/uyuşmazlık) ile ihlal sınıflandırması (`cut_list`)
/// aynı kesme dizisini görmek ZORUNDA (sınıflandırma `p*`'ı listede arar). İki ayrı döngü sessizce
/// ayrışabilirdi; tek döngüde bu hata sınıfı yapısal olarak yok. Bellek O(1): kesmeleri
/// biriktirmez, çağıran ne tutacağına kendisi karar verir.
fn walk_cuts<'a>(
    history: &'a History,
    floor: usize,
    atomic: bool,
    mut on_event: impl FnMut(CutEvent<'a>),
) {
    on_event(CutEvent::Cut(Cut::at_op(floor)));
    for (i, e) in history.entries().iter().enumerate().skip(floor) {
        let steps = steps_of(&e.op);
        let is_txn = matches!(e.op, Op::Txn { .. });
        let len = steps.len();
        for (j, (k, v)) in steps.into_iter().enumerate() {
            on_event(CutEvent::Step(k, v));
            if !atomic && is_txn && j + 1 < len {
                on_event(CutEvent::Cut(Cut {
                    ops: i,
                    txn_writes: j + 1,
                }));
            }
        }
        on_event(CutEvent::Cut(Cut::at_op(i + 1)));
    }
}

/// Tüm izinli kesmeleri tek geçişte tarar.
///
/// Değişmez: her kesmede `mism` = o kesmenin durumu `S` ile `R`'nin farklı olduğu anahtar sayısı.
/// Başlangıçta doğrudan hesaplanır; sonraki her adım yalnız BİR anahtarı değiştirir, bu yüzden
/// sayaç o anahtarın eski/yeni katkısı düzeltilerek güncellenir. `mism == 0 ⇔ S == R`, dolayısıyla
/// bu, izinli kümenin **tam** üyelik testidir (yanlış alarm da, kaçırma da yok).
///
/// Maliyet: zaman ≈ O((|S_F| + |R| + toplam adım) · log) (harita işlemleri), bellek O(|S| + |R|);
/// kesme sayısından bağımsız ek bellek yok. Her kesme için durumu baştan kurup karşılaştırmak
/// ise kesme sayısı × (geçmiş uzunluğu + durum boyutu) maliyetlidir.
///
/// Tanık olarak EN SON eşleşen kesme seçilir: (1) determinizm — birden çok eşleşmede hep aynı
/// cevap, (2) en çok bilgi — Get/Sync gibi durum değiştirmeyen işlemlerden sonraki konum,
/// "o işlemler de görüldü" bilgisini taşır.
///
/// Atomik txn'de kesmeler yalnız işlem sınırlarındadır, çünkü motor ya hep ya hiç vaat eder;
/// txn ortası durum izinli küme dışındadır. Atomik değilse txn içi her adım ayrı kesmedir.
fn prefix_scan(history: &History, floor: usize, atomic: bool, r: &State) -> PrefixScan {
    let mut s = history.state_at(Cut::at_op(floor));

    // Başlangıç uyuşmazlığı: S ve R'nin anahtar birleşimi üzerinde farklı olanlar.
    let mut mism: usize = 0;
    for (k, sv) in &s {
        if r.get(k) != Some(sv) {
            mism += 1;
        }
    }
    for k in r.keys() {
        if !s.contains_key(k) {
            mism += 1;
        }
    }

    let mut last_match = None;
    let mut best: Option<(usize, Cut)> = None;
    walk_cuts(history, floor, atomic, |ev| match ev {
        CutEvent::Step(k, new_v) => {
            let rv = r.get(k);
            let was_diff = s.get(k) != rv;
            apply_step(&mut s, k, new_v);
            let now_diff = s.get(k) != rv;
            // Ayrı dallar: değişmez bozulursa (ör. sayaç 0 iken azaltma) debug'da taşma paniği.
            if now_diff && !was_diff {
                mism += 1;
            } else if was_diff && !now_diff {
                mism -= 1;
            }
        }
        CutEvent::Cut(c) => {
            if mism == 0 {
                last_match = Some(c);
            }
            // `<=`: eşitlikte en son kesme kazanır.
            if best.is_none_or(|(bm, _)| mism <= bm) {
                best = Some((mism, c));
            }
        }
    });

    PrefixScan {
        last_match,
        best_cut: best
            .expect("değişmez: en az bir kesme (Cut{F,0}) ziyaret edildi")
            .1,
    }
}

// ---------------------------------------------------------------------------------------------
// Anahtar-bazlı kural
// ---------------------------------------------------------------------------------------------

/// Her anahtar için izinli değer kümesi `C_k`.
///
/// `C_k = {taban değeri} ∪ [F, n) aralığında k'ye yazılan değerler` (atomik txn'de txn içinde yalnız
/// SON yazma; ara değerler hiçbir zaman dışarıdan görünmemelidir). `[F, n)` öncesindeki eski
/// değerler fsync sınırının altındadır, izinli değildir.
fn key_candidates(
    history: &History,
    floor: usize,
    atomic: bool,
    base: &State,
    r: &State,
) -> BTreeMap<Key, BTreeSet<Option<Value>>> {
    let mut cands: BTreeMap<Key, BTreeSet<Option<Value>>> = BTreeMap::new();
    for k in base.keys().chain(r.keys()) {
        cands.entry(k.clone()).or_default();
    }
    for e in &history.entries()[floor..] {
        let steps = steps_of(&e.op);
        if atomic && matches!(e.op, Op::Txn { .. }) {
            // Aynı anahtara birden çok yazmada yalnız sonuncusu görünür olabilir.
            let mut last: BTreeMap<&Key, Option<&Value>> = BTreeMap::new();
            for (k, v) in steps {
                last.insert(k, v);
            }
            for (k, v) in last {
                cands.entry(k.clone()).or_default().insert(v.cloned());
            }
        } else {
            for (k, v) in steps {
                cands.entry(k.clone()).or_default().insert(v.cloned());
            }
        }
    }
    for (k, set) in &mut cands {
        set.insert(base.get(k).cloned());
    }
    cands
}

/// Anahtar-bazlı kontrol: bayt sırasındaki ilk başarısız anahtarı sınıflandırıp döndürür.
///
/// Sağlam (sound) ama tek başına tam değil: çapraz-anahtar tutarlılığı (prefix) ya da txn
/// bütünlüğü burada görülmez; onlar prefix taraması ve [`atomic_txn_violation`] ile ele alınır.
fn key_based_violation(
    history: &History,
    floor: usize,
    atomic: bool,
    r: &State,
) -> Option<Violation> {
    let base = history.state_at(Cut::at_op(floor));
    let cands = key_candidates(history, floor, atomic, &base, r);
    for (k, c) in &cands {
        let found = r.get(k);
        if !c.contains(&found.cloned()) {
            return Some(classify_key(history, floor, atomic, &base, k, c, found));
        }
    }
    None
}

/// §4.6(a): tek anahtar ihlalinin sınıfı. Yalnız teşhis; hüküm zaten verildi.
fn classify_key(
    history: &History,
    floor: usize,
    atomic: bool,
    base: &State,
    k: &Key,
    c: &BTreeSet<Option<Value>>,
    found: Option<&Value>,
) -> Violation {
    let mut wall: BTreeSet<&Value> = BTreeSet::new();
    let mut wpre: BTreeSet<&Value> = BTreeSet::new();
    let mut last_pre: Option<usize> = None;
    for (i, e) in history.entries().iter().enumerate() {
        for (kk, v) in steps_of(&e.op) {
            if kk != k {
                continue;
            }
            if let Some(v) = v {
                wall.insert(v);
                if i < floor {
                    wpre.insert(v);
                }
            }
            if i < floor {
                last_pre = Some(i);
            }
        }
    }

    let (kind, op_index) = match found {
        Some(v) if !wall.contains(v) => (ViolationKind::PhantomValue, None),
        None => (
            ViolationKind::LostDurableWrite,
            // None ∉ C_k ⇒ taban Some ⇒ k, [0,F)'de yazılmıştır.
            Some(last_pre.expect("değişmez: None ∉ C_k ⇒ taban Some ⇒ last_pre var")),
        ),
        Some(v) if wpre.contains(v) => {
            let kind = if base.get(k).is_none() {
                ViolationKind::ResurrectedDelete
            } else {
                ViolationKind::LostDurableWrite
            };
            (
                kind,
                Some(last_pre.expect("değişmez: v ∈ Wpre ⇒ last_pre var")),
            )
        }
        Some(v) => {
            // v yalnız [F,n)'de, atomik bir Txn'in ara (son olmayan) yazması olarak geçiyor.
            debug_assert!(atomic, "atomik olmayan modda bu dal erişilemez");
            let idx = (floor..history.len()).find(|&i| {
                let ws: Vec<Option<&Value>> = steps_of(&history.entries()[i].op)
                    .into_iter()
                    .filter(|(kk, _)| *kk == k)
                    .map(|(_, v)| v)
                    .collect();
                matches!(history.entries()[i].op, Op::Txn { .. })
                    && ws.len() >= 2
                    && ws[..ws.len() - 1].contains(&Some(v))
            });
            (
                ViolationKind::PartialTxn,
                Some(idx.expect("değişmez: v bir txn ara yazması")),
            )
        }
    };

    Violation {
        kind,
        key: k.clone(),
        expected: c.iter().cloned().collect(),
        found: found.cloned(),
        op_index,
        best_cut: None,
        durable_floor: Some(floor),
    }
}

/// Atomik txn sağlam kontrolü (yalnız `atomic_txn && !prefix_order`).
///
/// Bir Txn'in (T) yazdığı anahtarlardan bazıları KESİN uygulanmış, bazıları KESİN uygulanmamış
/// görünüyorsa txn kısmidir. Anahtar `k` yalnız şu koşullarda bilgi verir: T'den sonra başka bir
/// işlem `k`'ye yazmamıştır; ayrıca "uygulanmış" demek için `R[k]` = T'nin `k`'ye son yazdığı değer
/// olmalı VE bu değer, `k`'nin F..=T arasındaki kesmelerde (taban değeri ve `[F,T)` işlemlerinin
/// yazdığı değerler) aldığı değerlerden hiçbiri olmamalıdır (yoksa T uygulanmadan da görünebilirdi).
/// `R[k]` ≠ T'nin değeri ise "uygulanmamış" sayılır. Bu yüzden kontrol **sağlamdır** (yanlış alarm
/// yok) ama **tam değildir**: bilgi vermeyen anahtarlar atlandığından bazı kısmi txn'ler kaçabilir.
fn atomic_txn_violation(history: &History, floor: usize, r: &State) -> Option<Violation> {
    let n = history.len();

    // Anahtar zaman çizelgesi: (işlem indeksi, işlemden sonraki değer), tüm [0,n) için.
    let mut timeline: BTreeMap<&Key, Vec<(usize, Option<&Value>)>> = BTreeMap::new();
    for (i, e) in history.entries().iter().enumerate() {
        for (k, v) in steps_of(&e.op) {
            let line = timeline.entry(k).or_default();
            match line.last_mut() {
                Some(last) if last.0 == i => last.1 = v, // aynı işlemde son yazma kazanır
                _ => line.push((i, v)),
            }
        }
    }

    for t in floor..n {
        let Op::Txn { .. } = &history.entries()[t].op else {
            continue;
        };
        // Txn'in yazdığı farklı anahtarlar (bayt sırası) ve txn içindeki son değerleri.
        let mut post: BTreeMap<&Key, Option<&Value>> = BTreeMap::new();
        for (k, v) in steps_of(&history.entries()[t].op) {
            post.insert(k, v);
        }
        let mut applied = false;
        let mut not_applied: Option<(&Key, Option<&Value>)> = None;
        for (k, post_k) in post {
            let line = &timeline[k];
            if line.last().is_some_and(|&(i, _)| i > t) {
                continue; // sonradan başka işlem yazmış (indeks sıralı ⇒ son kayda bakmak yeter): bilgi vermez
            }
            // pre_k = {val_k(at_op(c)) : F ≤ c ≤ T}
            let mut pre: BTreeSet<Option<&Value>> = BTreeSet::new();
            pre.insert(
                line.iter()
                    .rev()
                    .find(|&&(i, _)| i < floor)
                    .and_then(|&(_, v)| v),
            );
            for &(i, v) in line {
                if (floor..t).contains(&i) {
                    pre.insert(v);
                }
            }
            let rk = r.get(k);
            if rk != post_k {
                if not_applied.is_none() {
                    not_applied = Some((k, post_k));
                }
            } else if !pre.contains(&post_k) {
                applied = true;
            }
        }
        if let (true, Some((k, post_k))) = (applied, not_applied) {
            return Some(Violation {
                kind: ViolationKind::PartialTxn,
                key: k.clone(),
                expected: vec![post_k.cloned()],
                found: r.get(k).cloned(),
                op_index: Some(t),
                best_cut: None,
                durable_floor: Some(floor),
            });
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Çapraz-anahtar sınıflandırma (prefix modu)
// ---------------------------------------------------------------------------------------------

/// İzinli kesme listesi ve her kesmeye kadar uygulanan adım sayısı `s(c)` (tüm geçmişte baştan
/// sayılır). Kesmeleri [`walk_cuts`] üretir; burada yalnız biriktirilir (yalnız ihlal yolunda).
fn cut_list(history: &History, floor: usize, atomic: bool) -> Vec<(Cut, usize)> {
    // s(Cut{F,0}) = [0,F) işlemlerinin toplam adım sayısı.
    let mut applied: usize = history.entries()[..floor]
        .iter()
        .map(|e| steps_of(&e.op).len())
        .sum();
    let mut list = Vec::new();
    walk_cuts(history, floor, atomic, |ev| match ev {
        CutEvent::Step(..) => applied += 1,
        CutEvent::Cut(c) => list.push((c, applied)),
    });
    list
}

/// §4.6(b): tüm tek-anahtar kontrolleri geçti ama hiçbir prefix `R`'yi açıklamıyor.
fn classify_cross_key(
    history: &History,
    floor: usize,
    atomic: bool,
    r: &State,
    p_star: Cut,
) -> Violation {
    let at_p = history.state_at(p_star);
    // M: p*'ın durumu ile R'nin farklı olduğu anahtarlar (BTreeSet → bayt sırası).
    let m: Vec<&Key> = at_p
        .keys()
        .chain(r.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|k| at_p.get(*k) != r.get(*k))
        .collect();
    let k = (*m.first().expect("değişmez: p* eşleşmedi ⇒ M boş değil")).clone();

    let violation = |kind, op_index| Violation {
        kind,
        key: k.clone(),
        expected: vec![at_p.get(&k).cloned()],
        found: r.get(&k).cloned(),
        op_index: Some(op_index),
        best_cut: Some(p_star),
        durable_floor: Some(floor),
    };

    // Atomik txn: fark, tek bir txn'in yazdığı anahtarların tamamını kapsıyorsa suçlu o txn'dir
    // (kesme tam ona denk gelen sınırdadır; ya hep ya hiç bozulmuş).
    if atomic {
        let before = p_star.ops.checked_sub(1).filter(|&t| t >= floor);
        let after = Some(p_star.ops).filter(|&t| t < history.len());
        for t in before.into_iter().chain(after) {
            let Op::Txn { .. } = &history.entries()[t].op else {
                continue;
            };
            let written: BTreeSet<&Key> = steps_of(&history.entries()[t].op)
                .into_iter()
                .map(|(kk, _)| kk)
                .collect();
            if m.iter().all(|mk| written.contains(*mk)) {
                return violation(ViolationKind::PartialTxn, t);
            }
        }
    }

    // PrefixViolation: k'nin R'deki değerini taşıyan, p*'a en yakın izinli kesme q'yu bul;
    // p* ile q arasında k'yi değiştiren işlem "sıra dışı kayıp"ın nedenidir.
    let cuts = cut_list(history, floor, atomic);
    let flat: Vec<(usize, &Key, Option<&Value>)> = history
        .entries()
        .iter()
        .enumerate()
        .flat_map(|(i, e)| steps_of(&e.op).into_iter().map(move |(kk, v)| (i, kk, v)))
        .collect();
    let mut val_after: Vec<Option<&Value>> = Vec::with_capacity(flat.len() + 1);
    let mut cur = None;
    val_after.push(cur);
    for &(_, kk, v) in &flat {
        if *kk == k {
            cur = v;
        }
        val_after.push(cur);
    }

    let p_idx = cuts
        .iter()
        .position(|&(c, _)| c == p_star)
        .expect("değişmez: p* izinli kesme listesindedir");
    let found = r.get(&k);
    let mut q_idx = None;
    let mut best_dist = usize::MAX;
    for (idx, &(_, s)) in cuts.iter().enumerate() {
        if val_after[s] == found {
            let dist = idx.abs_diff(p_idx);
            // `<=`: eşitlikte sonraki (büyük idx) kesme kazanır.
            if dist <= best_dist {
                best_dist = dist;
                q_idx = Some(idx);
            }
        }
    }
    let q_idx = q_idx.expect("değişmez: k anahtar-bazlı kontrolü geçti ⇒ q vardır");
    let (s_p, s_q) = (cuts[p_idx].1, cuts[q_idx].1);
    let op_index = if q_idx < p_idx {
        flat[s_q..s_p].iter().rev().find(|(_, kk, _)| **kk == k)
    } else {
        flat[s_p..s_q].iter().find(|(_, kk, _)| **kk == k)
    }
    .map(|&(i, _, _)| i)
    .expect("değişmez: val_k farklı ⇒ aralıkta k'ye yazan adım var");

    violation(ViolationKind::PrefixViolation, op_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entry, Outcome, Write};

    fn entry(op: Op) -> Entry {
        Entry {
            op,
            outcome: Outcome::Acked,
        }
    }

    fn put(k: &str, v: &str) -> Op {
        Op::Put {
            key: k.into(),
            value: v.into(),
        }
    }

    fn wput(k: &str, v: &str) -> Write {
        Write::Put {
            key: k.into(),
            value: v.into(),
        }
    }

    /// Prefix taraması ile sınıflandırmanın kullandığı `cut_list`, aynı yürütücüden gelir; yine de
    /// sözleşmeyi sabitleriz: (1) kesme dizisi SPEC §4.2 sırasında, (2) her `(c, s)` için ilk `s`
    /// adımın uygulanmış hali `state_at(c)` ile aynı (s sayacı kesmeyle tutarlı), (3) prefix
    /// taramasının tanığı/en iyi kesmesi bu listenin içinden çıkar.
    #[test]
    fn cut_list_matches_spec_order_and_step_counts() {
        let h = History::from_entries(vec![
            entry(put("a", "0")),
            entry(Op::Txn {
                writes: vec![wput("a", "1"), wput("b", "2"), wput("c", "3")],
            }),
            entry(Op::Txn { writes: vec![] }),
            entry(put("b", "9")),
        ])
        .unwrap();

        for atomic in [true, false] {
            for floor in 0..=h.len() {
                let list = cut_list(&h, floor, atomic);

                // (1) beklenen dizi, elle
                let mut want = vec![Cut::at_op(floor)];
                for i in floor..h.len() {
                    if !atomic && i == 1 {
                        for j in 1..3 {
                            want.push(Cut {
                                ops: 1,
                                txn_writes: j,
                            });
                        }
                    }
                    want.push(Cut::at_op(i + 1));
                }
                let got: Vec<Cut> = list.iter().map(|&(c, _)| c).collect();
                assert_eq!(got, want, "atomic={atomic} floor={floor}");

                // (2) s(c) ile state_at(c) tutarlı
                let flat: Vec<(&Key, Option<&Value>)> =
                    h.entries().iter().flat_map(|e| steps_of(&e.op)).collect();
                for &(c, s) in &list {
                    let mut st = State::new();
                    for &(k, v) in &flat[..s] {
                        apply_step(&mut st, k, v);
                    }
                    assert_eq!(st, h.state_at(c), "cut={c:?} s={s}");
                }

                // (3) tarama sonucu listenin içinden: her izinli kesmenin durumu tanık olur
                for &(c, _) in &list {
                    let r = h.state_at(c);
                    let scan = prefix_scan(&h, floor, atomic, &r);
                    assert!(list.iter().any(|&(x, _)| Some(x) == scan.last_match));
                    assert!(list.iter().any(|&(x, _)| x == scan.best_cut));
                }
            }
        }
    }
}
