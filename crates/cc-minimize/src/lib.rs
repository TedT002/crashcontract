//! # cc-minimize — karşı örneği küçültme
//!
//! Bir bulgu, ortalama ~70 işlemlik bir iş yükünün yüzlerce kesme noktasından birinde çıkar. İnsan
//! için anlamlı olan, hatayı hâlâ üreten **en küçük** iş yüküdür: çoğu zaman 1–3 işlem. Sırayla:
//!
//! 1. **İşlem dizisi — ddmin** (Zeller & Hildebrandt, 2002): diziyi parçalara böl; bir parça ya da
//!    bir parçanın tümleyeni hâlâ başarısızsa onunla devam et; değilse parçaları inceltip tekrar dene.
//!    Sonuç **1-minimaldir**: tek bir işlemi daha çıkarmak hatayı yok eder.
//! 2. **Txn içi yazmalar:** her txn'in yazma listesi de ddmin ile küçültülür (ddmin yalnız bütün
//!    işlemleri çıkarabilir; txn içindeki gereksiz yazmalar kalırdı). Tek yazmalı txn düz işleme
//!    çevrilmeyi dener.
//! 3. **Anahtar uzayı:** bir anahtarı daha önce görünen bir anahtarla birleştir.
//! 4. **Değer boyutu:** değeri boşalt, olmazsa yarıya indir.
//! 5. Değişiklik olduysa ddmin bir kez daha (birleşen anahtarlar yeni işlemleri gereksiz kılabilir).
//! 6. Anahtarları görünüş sırasıyla `a`, `b`, … diye yeniden adlandır (okunabilirlik).
//!
//! Her adım yalnız **aynı bulgu sınıfı** korunursa kabul edilir ([`same_class`]): küçültme sırasında
//! başka bir hataya "kaymak" raporu yanıltır.
//!
//! **Çökme noktası:** runner kesmeleri artan sırayla ve her kesmede durumları "en az kalıcı işlem"
//! sırasıyla dener, ilk bulguda durur. Küçültülmüş dizide raporlanan kesme bu yüzden en erken bozuk
//! kesmedir; ayrıca küçültülecek bir şey kalmaz.

use std::path::Path;

use cc_adapter::StorageAdapter;
use cc_adapter::runner::{FailureCase, Finding, RunConfig, RunError, run_case};
use cc_model::{Key, Op, Value, Write};

/// İki bulgu aynı sınıfta mı? (Finding varyantı + ihlal türü; ayrıntılar değişebilir.)
pub fn same_class(a: &Finding, b: &Finding) -> bool {
    match (a, b) {
        (Finding::Violation(x), Finding::Violation(y)) => x.kind == y.kind,
        _ => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

/// ddmin: `test(input)` doğruyken, `test`'i hâlâ doğru tutan 1-minimal bir alt dizi döndürür.
///
/// Neden 1-minimal ve global minimum değil? Global minimum üstel arama ister; 1-minimallik
/// O(n²) testle garanti edilir ve pratikte hatanın özünü (genellikle 1–3 işlem) verir.
pub fn ddmin<T: Clone>(input: &[T], test: &mut dyn FnMut(&[T]) -> bool) -> Vec<T> {
    if test(&[]) {
        return Vec::new();
    }
    let mut cur: Vec<T> = input.to_vec();
    let mut n = 2usize;
    while cur.len() >= 2 {
        let chunks = split(cur.len(), n);
        let mut reduced = false;
        // Önce tek bir parça yeterli mi?
        for &(lo, hi) in &chunks {
            if test(&cur[lo..hi]) {
                cur = cur[lo..hi].to_vec();
                n = 2;
                reduced = true;
                break;
            }
        }
        // Sonra bir parçanın tümleyeni (o parçayı çıkarmak) yeterli mi?
        if !reduced && n > 2 {
            for &(lo, hi) in &chunks {
                let complement: Vec<T> = [&cur[..lo], &cur[hi..]].concat();
                if test(&complement) {
                    cur = complement;
                    n = (n - 1).max(2);
                    reduced = true;
                    break;
                }
            }
        }
        if !reduced {
            if n >= cur.len() {
                break;
            }
            n = (2 * n).min(cur.len());
        }
    }
    // n == 2 iken tümleyen = diğer parça, yukarıda zaten denendi; tek eleman kalırsa da 1-minimaldir
    // (boş dizi başta denendi).
    cur
}

/// `len` elemanı `n` ardışık parçaya böler (ilk parçalar en fazla bir eleman fazla).
fn split(len: usize, n: usize) -> Vec<(usize, usize)> {
    let n = n.min(len).max(1);
    let (q, r) = (len / n, len % n);
    let mut out = Vec::with_capacity(n);
    let mut lo = 0;
    for i in 0..n {
        let hi = lo + q + usize::from(i < r);
        out.push((lo, hi));
        lo = hi;
    }
    out
}

/// Küçültme istatistikleri (rapora girer; README'deki küçültme oranı buradan).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stats {
    /// Küçültme öncesi işlem sayısı.
    pub original_ops: usize,
    /// Küçültme sonrası işlem sayısı.
    pub minimized_ops: usize,
    /// Öncesi/sonrası farklı anahtar sayısı.
    pub original_keys: usize,
    /// Küçültme sonrası farklı anahtar sayısı.
    pub minimized_keys: usize,
    /// Öncesi toplam değer baytı.
    pub original_value_bytes: usize,
    /// Küçültme sonrası toplam değer baytı.
    pub minimized_value_bytes: usize,
    /// Koşulan aday sayısı (her biri tam bir runner koşusu).
    pub tests_run: usize,
}

/// Küçültme sonucu.
#[derive(Debug, Clone)]
pub struct Minimized {
    /// Küçültülmüş iş yükü.
    pub ops: Vec<Op>,
    /// Küçültülmüş iş yükünün bulgusu (aynı sınıf).
    pub failure: FailureCase,
    /// İstatistikler.
    pub stats: Stats,
}

/// Bir işlemin tüm anahtarlarını dönüştürür.
fn map_keys(op: &Op, f: &dyn Fn(&Key) -> Key) -> Op {
    let w = |w: &Write| match w {
        Write::Put { key, value } => Write::Put {
            key: f(key),
            value: value.clone(),
        },
        Write::Delete { key } => Write::Delete { key: f(key) },
    };
    match op {
        Op::Put { key, value } => Op::Put {
            key: f(key),
            value: value.clone(),
        },
        Op::Delete { key } => Op::Delete { key: f(key) },
        Op::Get { key } => Op::Get { key: f(key) },
        Op::Txn { writes } => Op::Txn {
            writes: writes.iter().map(w).collect(),
        },
        Op::Sync => Op::Sync,
    }
}

/// Anahtarlar, ilk görünüş sırasıyla (tekrarsız).
fn keys_in_order(ops: &[Op]) -> Vec<Key> {
    let mut out: Vec<Key> = Vec::new();
    let mut push = |k: &Key| {
        if !out.contains(k) {
            out.push(k.clone());
        }
    };
    for op in ops {
        match op {
            Op::Put { key, .. } | Op::Delete { key } | Op::Get { key } => push(key),
            Op::Txn { writes } => {
                for w in writes {
                    match w {
                        Write::Put { key, .. } | Write::Delete { key } => push(key),
                    }
                }
            }
            Op::Sync => {}
        }
    }
    out
}

/// Değer konumları: (işlem indeksi, txn içi yazma indeksi).
fn value_slots(ops: &[Op]) -> Vec<(usize, Option<usize>)> {
    let mut out = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        match op {
            Op::Put { .. } => out.push((i, None)),
            Op::Txn { writes } => {
                for (j, w) in writes.iter().enumerate() {
                    if matches!(w, Write::Put { .. }) {
                        out.push((i, Some(j)));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn value_at(ops: &[Op], slot: (usize, Option<usize>)) -> &Value {
    match (&ops[slot.0], slot.1) {
        (Op::Put { value, .. }, None) => value,
        (Op::Txn { writes }, Some(j)) => match &writes[j] {
            Write::Put { value, .. } => value,
            Write::Delete { .. } => unreachable!("value_slots yalnız Put konumları üretir"),
        },
        _ => unreachable!("value_slots yalnız Put konumları üretir"),
    }
}

fn with_value(ops: &[Op], slot: (usize, Option<usize>), new: Value) -> Vec<Op> {
    let mut out = ops.to_vec();
    match (&mut out[slot.0], slot.1) {
        (Op::Put { value, .. }, None) => *value = new,
        (Op::Txn { writes }, Some(j)) => {
            if let Write::Put { value, .. } = &mut writes[j] {
                *value = new;
            }
        }
        _ => unreachable!("value_slots yalnız Put konumları üretir"),
    }
    out
}

fn value_bytes(ops: &[Op]) -> usize {
    value_slots(ops)
        .into_iter()
        .map(|s| value_at(ops, s).len())
        .sum()
}

/// Bir bulguyu küçültür. `target`: korunacak bulgu (sınıfı). Seed sabit tutulur (çökme durumu
/// örneklemesi ona bağlıdır).
pub fn minimize<A: StorageAdapter>(
    ops: Vec<Op>,
    config: &A::Config,
    workdir: &Path,
    seed: u64,
    rc: &RunConfig,
    target: &Finding,
) -> Result<Minimized, RunError> {
    let original_ops = ops.len();
    let original_keys = keys_in_order(&ops).len();
    let original_value_bytes = value_bytes(&ops);
    let mut tests_run = 0usize;
    let mut last: Option<FailureCase> = None;
    let mut infra: Option<RunError> = None;

    // Aday hâlâ aynı sınıfta başarısız mı? Altyapı hatası "başarısız değil" sayılır (adayı kabul
    // etme) ama hatırlanır: hiçbir aday başarısız olmazsa raporlanır.
    let mut fails = |cand: &[Op]| -> bool {
        tests_run += 1;
        match run_case::<A>(cand.to_vec(), config, workdir, seed, rc) {
            Ok(o) => match o.failure {
                Some(f) if same_class(&f.finding, target) => {
                    last = Some(f);
                    true
                }
                _ => false,
            },
            Err(e) => {
                infra = Some(e);
                false
            }
        }
    };

    // 1. ddmin.
    let mut cur = ddmin(&ops, &mut fails);

    // 2. Txn içi yazmalar: her txn'in yazma listesini ayrıca ddmin'le.
    let mut changed = false;
    for i in 0..cur.len() {
        let Op::Txn { writes } = &cur[i] else {
            continue;
        };
        if writes.len() < 2 {
            continue;
        }
        let base = cur.clone();
        let fewer = ddmin(writes, &mut |ws| {
            let mut cand = base.clone();
            cand[i] = Op::Txn {
                writes: ws.to_vec(),
            };
            fails(&cand)
        });
        if fewer.len() < writes.len() {
            cur[i] = Op::Txn { writes: fewer };
            changed = true;
        }
    }
    // Tek yazmalı txn → düz işlem (okunabilirlik; atomiklik farkı yoksa aynı hatayı üretir).
    for i in 0..cur.len() {
        let plain = match &cur[i] {
            Op::Txn { writes } if writes.len() == 1 => match &writes[0] {
                Write::Put { key, value } => Op::Put {
                    key: key.clone(),
                    value: value.clone(),
                },
                Write::Delete { key } => Op::Delete { key: key.clone() },
            },
            _ => continue,
        };
        let mut cand = cur.clone();
        cand[i] = plain;
        if fails(&cand) {
            cur = cand;
            changed = true;
        }
    }

    // 3. Anahtar birleştirme: sondaki anahtarı daha öncekilerden biriyle değiştirmeyi dene.
    'merge: loop {
        let keys = keys_in_order(&cur);
        for i in (1..keys.len()).rev() {
            for j in 0..i {
                let (from, to) = (keys[i].clone(), keys[j].clone());
                let cand: Vec<Op> = cur
                    .iter()
                    .map(|op| map_keys(op, &|k| if *k == from { to.clone() } else { k.clone() }))
                    .collect();
                if fails(&cand) {
                    cur = cand;
                    changed = true;
                    continue 'merge;
                }
            }
        }
        break;
    }

    // 4. Değer boyutu: boş, olmazsa yarıya (başarısız olmayana dek).
    for slot in value_slots(&cur) {
        let mut v = value_at(&cur, slot).clone();
        if v.is_empty() {
            continue;
        }
        if fails(&with_value(&cur, slot, Vec::new())) {
            cur = with_value(&cur, slot, Vec::new());
            changed = true;
            continue;
        }
        // Boş değer hatayı yok ediyor: değer içeriği önemli; yarıya indirerek dene.
        while v.len() > 1 {
            let half = v[..v.len() / 2].to_vec();
            if !fails(&with_value(&cur, slot, half.clone())) {
                break;
            }
            cur = with_value(&cur, slot, half.clone());
            changed = true;
            v = half;
        }
    }

    // 5. Değişiklik olduysa ddmin bir kez daha.
    if changed {
        cur = ddmin(&cur, &mut fails);
    }

    // 6. Okunabilir anahtar adları.
    let keys = keys_in_order(&cur);
    let name = |i: usize| -> Key {
        if i < 26 {
            vec![b'a' + i as u8]
        } else {
            format!("k{i}").into_bytes()
        }
    };
    let renamed: Vec<Op> = cur
        .iter()
        .map(|op| {
            map_keys(op, &|k| {
                let i = keys
                    .iter()
                    .position(|x| x == k)
                    .expect("değişmez: anahtar listede");
                name(i)
            })
        })
        .collect();
    if renamed != cur && fails(&renamed) {
        cur = renamed;
    }

    // Son durumun bulgusu: `cur` ile yeniden koş (son kabul edilen aday `cur` olmayabilir).
    let final_ok = fails(&cur);
    let failure = match (final_ok, last) {
        (true, Some(f)) => f,
        _ => {
            return Err(infra.unwrap_or_else(|| {
                RunError::Io(std::io::Error::other(
                    "küçültülmüş dizi bulguyu yeniden üretmedi (deterministik olmayan motor?)",
                ))
            }));
        }
    };
    let stats = Stats {
        original_ops,
        minimized_ops: cur.len(),
        original_keys,
        minimized_keys: keys_in_order(&cur).len(),
        original_value_bytes,
        minimized_value_bytes: value_bytes(&cur),
        tests_run,
    };
    Ok(Minimized {
        ops: cur,
        failure,
        stats,
    })
}
