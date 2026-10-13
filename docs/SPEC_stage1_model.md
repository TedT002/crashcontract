# Aşama 1 — İskelet + `cc-model` — Teknik Sözleşme

> Bağlam: `/CLAUDE.md` (tasarım kararları 1–5) ve `docs/PLAN_crashcontract.md`.
> Bu belge **bağlayıcıdır**. Public imzalar (tip adları, alan adları, varyant adları, fonksiyon
> imzaları) birebir uygulanır; değişiklik gerekiyorsa implementer durur ve SPEC revizyonu ister.
> İç (private / `pub(crate)`) yardımcılar serbesttir.

---

## 0. Bu aşamanın kapsamı ve kararları

1. **Workspace kökü proje köküdür** (`/home/tedt/Desktop/rüya`). CLAUDE.md'deki `crashcontract/`
   klasörü deponun kendisidir; ekstra bir alt klasör açılmaz.
2. **Yalnızca `crates/cc-model` oluşturulur.** Diğer crate'ler (cc-trace, cc-workload, cc-adapter,
   cc-minimize, cc-shim, cc-cli, engines/*) **kendi aşamalarında** eklenir.
   Gerekçe: (a) boş iskelet crate'ler, henüz tasarlanmamış API'ler için yer tutucu olur ve yanlış
   tahminlere davetiye çıkarır; (b) her boş crate CI'da derleme/lint yükü ve "neden var?" sorusudur;
   (c) `members` açık liste olduğundan yeni crate eklemek tek satırdır, workspace bozulmaz.
   Glob (`crates/*`) KULLANILMAZ: Cargo, glob ile eşleşen ama `Cargo.toml`'u olmayan bir dizinde
   hata verir; açık liste, yarım bırakılmış dizinlerin CI'ı kırmasını engeller.
3. **`cc-model` sıfır dış bağımlılıklıdır.** Yalnız `std`. `serde` Aşama 6'da (JSON rapor) eklenecek;
   o zaman bu tiplere `#[derive(Serialize, Deserialize)]` eklenir (eklemeli değişiklik, kontratı
   bozmaz; `History` için deserialize `History::from_entries` doğrulamasından geçecek şekilde yapılır).
   Bu aşamada `serde` EKLENMEZ.
4. **Toolchain sabit:** `rust-toolchain.toml` → `1.98.1` (1.98.0'daki vtable miscompile düzeltmesi).
   `clippy -D warnings` floating stable ile yeni lint'ler yüzünden CI'ı keyfi kırar; sabitlemek bunu önler.
5. **Edition 2024, resolver "3"** (MSRV-farkındalıklı çözümleyici; sanal manifestte açıkça yazılmalı).
6. **Determinizm:** `cc-model` içinde `HashMap`/`HashSet` YASAK; yalnız `BTreeMap`/`BTreeSet`/`Vec`.
   Hükmün içeriği (hangi anahtar/işlem raporlanır) girdiye göre tamamen belirlidir.

---

## 1. Dizin / Bileşen Yapısı (birebir)

```
/home/tedt/Desktop/rüya/
├── CLAUDE.md                      (mevcut, DOKUNMA)
├── Cargo.toml                     (sanal workspace manifesti)
├── Cargo.lock                     (ilk derlemede oluşur; depoya dahil edilir — CI --locked kullanır)
├── rust-toolchain.toml
├── .gitignore
├── .github/
│   └── workflows/
│       └── ci.yml
├── docs/
│   ├── PLAN_crashcontract.md      (mevcut)
│   └── SPEC_stage1_model.md       (bu dosya)
└── crates/
    └── cc-model/
        ├── Cargo.toml
        ├── src/
        │   ├── lib.rs             crate doc (öğretici genel bakış) + `pub use` re-export'lar
        │   ├── op.rs              Key, Value, State, Write, Op
        │   ├── history.rs         Outcome, Entry, History, HistoryError, Cut
        │   ├── guarantees.rs      Durability, Guarantees
        │   ├── verdict.rs         Verdict, Violation, ViolationKind (+ Display)
        │   └── check.rs           check_recovered, check_reads (+ iç zaman çizelgesi yapıları)
        └── tests/
            ├── common/
            │   └── mod.rs         test DSL yardımcıları (bkz. §6.1)
            ├── history.rs         H01–H11
            ├── recovered.rs       R01–R36 (+ kaba kuvvet karşılaştırma testi, bkz. Doğrulama Planı)
            └── reads.rs           D01–D06
```

Modüller `lib.rs`'te **private** (`mod op;` vb.) tanımlanır; tüm public öğeler crate kökünden
re-export edilir. Tüketiciler yalnızca `cc_model::X` yolunu kullanır; modül yolları kontrat değildir.

```rust
// lib.rs (re-export listesi — birebir)
pub use check::{check_reads, check_recovered};
pub use guarantees::{Durability, Guarantees};
pub use history::{Cut, Entry, History, HistoryError, Outcome};
pub use op::{Key, Op, State, Value, Write};
pub use verdict::{Verdict, Violation, ViolationKind};
```

### 1.1 Kök `Cargo.toml` (birebir içerik; yorumlar Türkçe kalabilir)

```toml
[workspace]
resolver = "3"
# Açık liste: her crate kendi aşamasında buraya eklenir (glob yok — bkz. SPEC §0.2).
members = [
    "crates/cc-model",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.98"
publish = false

[workspace.lints.rust]
# Yalnızca cc-shim (Aşama 7, LD_PRELOAD) crate seviyesinde gerekçeli #![allow(unsafe_code)] ile açar.
# "forbid" değil "deny": forbid, crate seviyesinde allow ile geçersiz kılınamaz.
unsafe_code = "deny"
missing_docs = "warn"

[workspace.lints.clippy]
dbg_macro = "warn"
todo = "warn"
unimplemented = "warn"
```

### 1.2 `crates/cc-model/Cargo.toml`

```toml
[package]
name = "cc-model"
description = "CrashContract referans modeli: izinli çökme-sonrası durumlar kümesi ve hüküm"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

# Bilerek boş: model saf mantıktır, yalnız std kullanır.
# serde Aşama 6'da (JSON rapor) gerekçesiyle eklenecek.
[dependencies]

[lints]
workspace = true
```

### 1.3 `rust-toolchain.toml`

```toml
[toolchain]
channel = "1.98.1"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

### 1.4 `.gitignore`

```
/target
```

### 1.5 `.github/workflows/ci.yml`

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      # rust-toolchain.toml'daki sürümü kur. rustup 1.28.0 otomatik kurulumu kaldırmıştı;
      # bu satır (rustup duyurusundaki resmi tek satır) eski ve yeni rustup'ta çalışır.
      - name: Toolchain
        run: rustup show active-toolchain || rustup toolchain install
      - name: fmt
        run: cargo fmt --all -- --check
      - name: clippy
        run: cargo clippy --workspace --all-targets --locked -- -D warnings
      - name: test
        run: cargo test --workspace --locked
```
Not: Dış action olarak yalnız `actions/checkout` kullanılır. Cache action'ı bilinçli olarak yok
(dış bağımlılık yok; derleme süresi önemsiz). Daha yeni bir `actions/checkout` major sürümü
kullanılmak istenirse önce sürümün varlığı doğrulanmalı.

---

## 2. Kavramlar (implementer'ın kafasında net olması gerekenler)

- **İşlem (op):** iş yükünün motora gönderdiği tek çağrı: `Put`, `Delete`, `Get`, `Txn` (begin…commit
  arası yazmalar, commit ACK noktasıdır), `Sync`.
- **Adım (step):** durumu değiştiren tek yazma. `Put`/`Delete` = 1 adım; `Txn` = içindeki yazma sayısı
  kadar adım (sırayla); `Get`/`Sync` = 0 adım.
- **ACK ≠ dayanıklılık:** Motorun "tamam" demesi, verinin diskte olduğu anlamına gelmez. Hangi ACK'in
  dayanıklılık vaat ettiğini motorun **bildirdiği** `Durability` belirler.
- **Dayanıklılık tabanı (durable floor) `F`:** ilk `F` işlemin etkisi çökmeden sonra kesinlikle
  görülmelidir (fsync sınırı).
- **Üst sınır `n`:** geçmişteki işlem sayısı (in-flight dahil). Hiç gönderilmemiş işlemin etkisi asla
  görülemez.
- **Belirsiz işlem:** sonucu `Issued`, `InFlight` veya `Failed` olan işlem. Uygulanmış da olabilir,
  uygulanmamış da (karar 3). **Değişmez: yalnızca geçmişin SON işlemi belirsiz olabilir.** (Tek iş
  parçacıklı sıralı yük; ayrıca runner bir işlem `Failed` döndüğünde iş yükünü durdurur — böylece
  belirsizlik tek noktada kalır ve kontrol polinom kalır.)
- **Kesme noktası (cut):** "geçmişin ilk p adımı uygulandı" konumu. Atomik txn varsa kesmeler yalnız
  işlem sınırlarında; yoksa txn içinde de olabilir.
- **İzinli küme (prefix modu):** `{ state_at(c) : c izinli kesme, Cut{F,0} ≤ c ≤ Cut{n,0} }`.
- **İzinli küme (anahtar-bazlı mod, prefix garantisi yok):** her anahtar bağımsız olarak kendi
  izinli değerlerinden birini taşır (+ atomik txn için çapraz-anahtar sağlam kontrol, §4.4).

---

## 3. Public API (birebir imzalar)

Tüm public öğeler — enum varyantları, struct alanları ve **varyant alanları** dahil — `///` doküman
yorumu taşır (`missing_docs = "warn"` + `-D warnings`). Aşağıdaki bloklarda yer kazanmak için bazı
alan yorumları kısaltılmıştır; implementer eksik olanları ekler. Derive listeleri birebir uygulanır.

### 3.1 `op.rs`

```rust
use std::collections::BTreeMap;

/// Anahtar: ham bayt dizisi. Sıralama bayt-sözlük (lexicographic) sırasıdır;
/// tüm "ilk anahtar" kuralları bu sıraya göredir.
pub type Key = Vec<u8>;

/// Değer: ham bayt dizisi.
pub type Value = Vec<u8>;

/// Bir anahtar-değer deposunun mantıksal içeriği. Silinmiş anahtar haritada YOKTUR
/// (tombstone burada temsil edilmez; tombstone motorun iç ayrıntısıdır).
pub type State = BTreeMap<Key, Value>;

/// Bir işlem (txn) içindeki tek yazma.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// Anahtara değer yaz (varsa üzerine yaz).
    Put { key: Key, value: Value },
    /// Anahtarı sil (yoksa etkisiz).
    Delete { key: Key },
}

/// İş yükünün motora gönderdiği tek işlem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Tek yazma; ACK'i motorun Durability bildirimine göre dayanıklılık vaat eder/etmez.
    Put { key: Key, value: Value },
    /// Tek silme.
    Delete { key: Key },
    /// Okuma; durumu değiştirmez. Sonucu `Outcome::Read` ile kaydedilir.
    Get { key: Key },
    /// begin…commit arası yazmalar; ACK = commit'in dönmesi. Txn içinde Get yoktur (v1 kapsamı).
    /// Boş `writes` geçerlidir (etkisiz).
    Txn { writes: Vec<Write> },
    /// Açık dayanıklılık bariyeri (motorun sync/flush çağrısı).
    Sync,
}
```

### 3.2 `guarantees.rs`

```rust
/// Motorun BİLDİRDİĞİ dayanıklılık modu. Model motoru vaat ettiğiyle yargılar:
/// vaat etmediği bir şeyi kaybetmesi ihlal değildir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Durability {
    /// Her başarılı yazma/txn/sync ACK'i dönmeden önce dayanıklıdır (ör. commit başına fsync).
    OnAck,
    /// Yazmalar yalnızca ACK'li bir `Op::Sync` sonrasında dayanıklıdır (ör. grup commit,
    /// sayfa önbelleğinde bekleyen WAL). Sync'ten önceki her şey Sync ACK'iyle sabitlenir.
    OnSync,
    /// Hiçbir dayanıklılık vaadi yok. Kurtarılan durum yine de hiç yazılmamış veri içeremez
    /// ve (prefix_order ise) bir prefix olmalıdır.
    NoGuarantee,
}

/// Motorun bildirdiği garantilerin tamamı. Tüm kombinasyonlar geçerlidir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Guarantees {
    /// Dayanıklılık modu.
    pub durability: Durability,
    /// true: bir Txn'in yazmaları çökme sonrası ya hep ya hiç görünür.
    /// false: Txn, sıralı bağımsız yazmalar gibi değerlendirilir (txn içinde kesme serbest).
    pub atomic_txn: bool,
    /// true: kurtarılan durum geçmişin bir prefix'ine eşit olmalı (sonraki bir yazma duruyorsa
    /// öncekiler kaybolmamış olmalı). false: yalnızca anahtar-bazlı kural (§4.4).
    pub prefix_order: bool,
}
```

### 3.3 `history.rs`

```rust
use crate::{Durability, Op, State, Value};

/// Bir işlemin sonucu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Gönderildi, henüz dönmedi (kayıt sürerken geçici durum). Kontrolde InFlight gibi davranır.
    Issued,
    /// Get dışındaki bir işlem başarıyla döndü.
    Acked,
    /// Get başarıyla döndü; `value` okunan değer (None = anahtar yok).
    Read { value: Option<Value> },
    /// İşlem hata döndürdü. Etkisi belirsizdir (uygulanmış olabilir). Geçmişi kapatır.
    Failed { reason: String },
    /// Çökme anında yarıda kaldı. Etkisi belirsizdir. Geçmişi kapatır.
    InFlight,
}

impl Outcome {
    /// Issued | Failed | InFlight → true.
    pub fn is_uncertain(&self) -> bool;
}

/// Geçmişteki tek kayıt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// İşlem.
    pub op: Op,
    /// Sonucu.
    pub outcome: Outcome,
}

/// Geçmiş üzerinde bir konum: ilk `ops` işlem tamamen + `ops` indeksli işlemin (bir Txn olmalı)
/// ilk `txn_writes` yazması uygulanmış. Kanonik biçim: `txn_writes == 0` veya
/// `0 < txn_writes < writes.len()` (tam uygulanmış txn `Cut{ops+1, 0}` ile gösterilir).
/// Sıra (derive Ord) = geçmişteki konum sırası.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cut {
    /// Tamamen uygulanmış işlem sayısı.
    pub ops: usize,
    /// `ops` indeksli Txn'in uygulanmış yazma sayısı (çoğunlukla 0).
    pub txn_writes: usize,
}

impl Cut {
    /// `Cut { ops, txn_writes: 0 }`.
    pub const fn at_op(ops: usize) -> Cut;
}

/// Geçmiş oluşturma/doğrulama hataları.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    /// Önceki işlem (`index`) hâlâ Issued iken yeni işlem gönderilmeye çalışıldı.
    PendingOp { index: usize },
    /// Son işlem Failed/InFlight; geçmiş kapalı, yeni işlem eklenemez.
    Closed,
    /// `index` bekleyen (Issued durumundaki son) işlem değil.
    NotPending { index: usize },
    /// Get için `ack` ya da Get olmayan için `ack_read` (veya from_entries'te Read/Acked uyumsuzluğu).
    OutcomeMismatch { index: usize },
    /// from_entries: belirsiz sonuçlu bir kayıt son kayıt değil.
    UncertainNotLast { index: usize },
}
// impl std::fmt::Display for HistoryError (Türkçe tek satır) + impl std::error::Error.

/// İşlem geçmişi. Değişmezler (her public yol korur):
///  (I1) Yalnızca son kaydın sonucu belirsiz (Issued/Failed/InFlight) olabilir.
///  (I2) `Outcome::Read` yalnız `Op::Get` için; `Outcome::Acked` yalnız Get olmayan için.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct History {
    entries: Vec<Entry>, // private: değişmezler yalnız API ile korunur
}

impl History {
    /// Boş geçmiş.
    pub fn new() -> History;

    /// Hazır kayıtlardan geçmiş kurar; I1/I2'yi doğrular. (Testler ve Aşama 6 rapor okuma için.)
    /// Hata önceliği: kayıtlar baştan sona taranır; ilk bozulan kayıt için önce I2
    /// (`OutcomeMismatch`), sonra I1 (`UncertainNotLast`) kontrol edilir.
    /// Son kaydın `Issued` olması geçerlidir.
    pub fn from_entries(entries: Vec<Entry>) -> Result<History, HistoryError>;

    /// Kayıtlar (salt-okunur).
    pub fn entries(&self) -> &[Entry];
    /// Kayıt sayısı (= n).
    pub fn len(&self) -> usize;
    /// Boş mu?
    pub fn is_empty(&self) -> bool;

    /// Yeni işlemi `Issued` olarak ekler, indeksini döndürür.
    /// Hata: son kayıt Issued → `PendingOp{index: son}`; son kayıt Failed/InFlight → `Closed`.
    pub fn issue(&mut self, op: Op) -> Result<usize, HistoryError>;

    /// Bekleyen Get olmayan işlemi `Acked` yapar.
    /// Hata: `index` son kayıt değil ya da son kayıt Issued değil → `NotPending{index}`;
    /// op Get → `OutcomeMismatch{index}`. (Önce NotPending, sonra OutcomeMismatch kontrol edilir.)
    pub fn ack(&mut self, index: usize) -> Result<(), HistoryError>;

    /// Bekleyen Get'i `Read{value}` yapar. Hatalar `ack` ile aynı sırada; op Get değil → OutcomeMismatch.
    pub fn ack_read(&mut self, index: usize, value: Option<Value>) -> Result<(), HistoryError>;

    /// Bekleyen işlemi `Failed{reason}` yapar (her op türü için). Hata: `NotPending{index}`.
    pub fn fail(&mut self, index: usize, reason: impl Into<String>) -> Result<(), HistoryError>;

    /// Çökme: son kayıt Issued ise InFlight yapar; değilse hiçbir şey yapmaz.
    pub fn mark_crashed(&mut self);

    /// Aşama 4 runner'ı için: tam koşu geçmişinden, çökme noktasına göre bir görünüm üretir.
    /// Sonuç: kayıtlar [0, completed) aynen; `in_flight` ise `completed` indeksli işlem
    /// `Outcome::InFlight` ile eklenir; geri kalan atılır.
    /// # Panics
    /// `completed > len()`; ya da `in_flight && completed >= len()`;
    /// ya da sonuç I1'i bozarsa ([0, completed) içinde son eleman dışında belirsiz kayıt kalırsa,
    /// veya `in_flight` iken [0, completed) içinde belirsiz kayıt varsa).
    pub fn crash_view(&self, completed: usize, in_flight: bool) -> History;

    /// `cut` konumuna kadar tüm adımları (sonuçtan bağımsız) boş durumdan uygulayıp durumu döndürür.
    /// # Panics
    /// `cut.ops > len()`; `cut.txn_writes > 0` iken `cut.ops` indeksli işlem yok/Txn değil ya da
    /// `cut.txn_writes >= writes.len()`.
    pub fn state_at(&self, cut: Cut) -> State;

    /// Dayanıklılık tabanı F (bkz. §4.1).
    pub fn durable_floor(&self, durability: Durability) -> usize;
}
```

### 3.4 `verdict.rs`

```rust
use crate::{Cut, Key, Value};

/// İhlal sınıfı. Minimizer "aynı türde ihlal hâlâ var mı?" sorusunu bununla sorar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ViolationKind {
    /// Dayanıklılık tabanının altındaki (vaat edilmiş) bir yazmanın etkisi kayıp ya da eski değer döndü.
    LostDurableWrite,
    /// Prefix tutarlılığı bozuk: tek başına her anahtar izinli ama hiçbir prefix hepsini açıklamıyor
    /// (sonraki bir yazma duruyor, önceki kayıp — sıra dışı kayıp).
    PrefixViolation,
    /// O anahtara hiç yazılmamış bir değer belirdi (çöp/bozuk kayıt/yanlış anahtar).
    PhantomValue,
    /// Atomik bildirilen bir Txn kısmen uygulanmış.
    PartialTxn,
    /// Dayanıklı biçimde silinmiş bir anahtar eski değeriyle geri geldi (tombstone kaybı).
    ResurrectedDelete,
    /// Çökmeden önce, çalışma sırasında Get modelden farklı değer döndürdü.
    ReadMismatch,
}

/// Bir ihlalin raporu (minimizer ve JSON rapor için yeterli bilgi).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Sınıf.
    pub kind: ViolationKind,
    /// İhlalin görüldüğü anahtar (birden fazlaysa bayt sırasına göre ilki).
    pub key: Key,
    /// Bu anahtar için beklenen aday değerler; sıralı (Option<Value> Ord: None önce) ve tekrarsız.
    /// Anlamı türe göre §4.6'da tanımlı.
    pub expected: Vec<Option<Value>>,
    /// Kurtarılan durumda (veya ReadMismatch'te okumada) bulunan değer.
    pub found: Option<Value>,
    /// İlgili işlemin geçmişteki indeksi (türe göre §4.6). PhantomValue'da None.
    pub op_index: Option<usize>,
    /// Çapraz-anahtar ihlallerinde (prefix modu) en iyi uyan kesme noktası; diğerlerinde None.
    pub best_cut: Option<Cut>,
    /// Hesaplanan dayanıklılık tabanı F; ReadMismatch'te None.
    pub durable_floor: Option<usize>,
}

/// Modelin hükmü. Hata değil, değerdir.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum Verdict {
    /// Kurtarılan durum izinli kümede.
    /// `witness`: prefix modunda durumu açıklayan EN SON izinli kesme; anahtar-bazlı modda ve
    /// `check_reads`'te None.
    Allowed { witness: Option<Cut> },
    /// İzinli küme dışında; birincil ihlal.
    Violated(Violation),
}

impl Verdict {
    /// Allowed mı?
    pub fn is_allowed(&self) -> bool;
    /// Violated ise ihlale referans.
    pub fn violation(&self) -> Option<&Violation>;
}
// impl Display for ViolationKind (Türkçe kısa ad), impl Display for Violation (tek satır,
// anahtar/değerler `escape_ascii()` ile). Display biçimi insan içindir, kontrat DEĞİLDİR.
```

### 3.5 `check.rs`

```rust
use crate::{Guarantees, History, State, Verdict};

/// Çökme sonrası kurtarılan durum, motorun bildirdiği garantilere göre izinli mi?
/// `history` son kaydı Issued/InFlight/Failed olabilir (üçü de "belirsiz" muamelesi görür).
/// Saf fonksiyon; aynı girdi → aynı Verdict.
pub fn check_recovered(history: &History, guarantees: Guarantees, recovered: &State) -> Verdict;

/// Çalışma sırasındaki (çökme öncesi) Get sonuçlarını doğrular. Tek iş parçacıklı sıralı yükte
/// her Get, kendinden önceki tüm yazmaların uygulanmış haline eşit olmalıdır (read-your-writes).
/// Bu kural garantilerden bağımsızdır (dayanıklılık yalnız çökmeyi ilgilendirir), o yüzden
/// Guarantees almaz. İlk uyuşmazlıkta döner; yoksa `Allowed { witness: None }`.
pub fn check_reads(history: &History) -> Verdict;
```

---

## 4. Semantik ve Algoritma (bağlayıcı)

Notasyon: `n = history.len()`, `F = durable_floor`, `R = recovered`, `val_k(c) = state_at(c).get(k)`,
`W` = toplam adım sayısı, `K` = farklı anahtar sayısı.

### 4.1 Dayanıklılık tabanı `F` (`History::durable_floor`)
- `OnAck`: sonucu `Acked` veya `Read` olan baştaki ardışık kayıtların sayısı (I1 gereği: son kayıt
  belirsizse `n-1`, değilse `n`).
- `OnSync`: sonucu `Acked` olan son `Op::Sync`'in indeksi `+ 1`; yoksa `0`. (Issued/InFlight/Failed
  Sync tabanı YÜKSELTMEZ — sync dönmediyse bariyer kurulmamıştır.)
- `NoGuarantee`: `0`.

### 4.2 İzinli kesmeler
Sıralı liste `cuts(F)`: `Cut::at_op(F)`'den `Cut::at_op(n)`'ye kadar her işlem sınırı; ayrıca
`atomic_txn == false` ise `F ≤ i < n` olan her `Op::Txn{writes}` için `Cut{i, j}`, `1 ≤ j < writes.len()`
(konum sırasına göre araya). `Get`/`Sync` da işlem sınırı üretir (durum aynı olsa da ayrı kesmedir).

### 4.3 Prefix modu (`prefix_order == true`) — tam üyelik testi
```
S = state_at(Cut::at_op(F))
mism = |{ k : S.get(k) != R.get(k) }|          // S ve R'nin sıralı birleşimi, O(|S|+|R|)
last_match = None
best = None                                     // (mism, cut): en az uyuşmazlık; eşitlikte EN SON kesme
kontrol_et(Cut::at_op(F))
for i in F..n:
    for (j, step (k, new_v)) in steps(op i):    // Put → (k, Some v), Delete → (k, None)
        old = S.get(k); uygula; new = S.get(k)
        mism += (new != R.get(k)) as i64 - (old != R.get(k)) as i64
        if !atomic_txn && op i Txn && j+1 < len: kontrol_et(Cut{i, j+1})
    kontrol_et(Cut::at_op(i+1))
  "kontrol_et(c)": if mism == 0 { last_match = Some(c) };
                   if best None || mism <= best.mism { best = (mism, c) }
if let Some(c) = last_match → Allowed { witness: Some(c) }
else → sınıflandır: önce §4.4 anahtar-bazlı (ilk başarısız anahtar döner), hiçbiri başarısız
       değilse §4.6(b) çapraz-anahtar sınıflandırma (p* = best.cut).
```
**Doğruluk:** Değişmez — her kesmede `mism` = `S` ile `R`'nin farklı olduğu anahtar sayısı. Başlangıçta
doğrudan hesaplanır; her adım yalnız bir anahtarı değiştirir ve sayaç o anahtarın katkısını
eski/yeni olarak düzeltir. `mism == 0 ⇔ S == R`. İzinli kümenin tanımı (§2) gereği bu bir **tam**
üyelik testidir: yanlış alarm (izinli durumu reddetme) ve kaçırma yoktur. Sınıflandırma yalnız
ihlal varken çalışır; hükmü değiştirmez, yalnız teşhis eder.
**Karmaşıklık:** O((n + W + |S_F| + |R|) · log K) zaman, O(|S|) ek bellek. Açık listeleme (üstel)
yerine sembolik aralık + artımlı sayaç.

### 4.4 Anahtar-bazlı kural (prefix_order == false'ta HÜKÜM, prefix modunda TEŞHİS)
Her anahtar `k` için:
- `base_k = val_k(Cut::at_op(F))`
- `C_k` (izinli değerler) = `{base_k}` ∪ { `[F, n)` aralığındaki işlemlerin `k`'ye yazdığı değerler,
  `atomic_txn` ise bir Txn içinde yalnız `k`'ye yapılan **son** yazma, değilse her yazma }
  (Delete → `None`).
- Kurtarılan değer `R.get(k)` ∈ `C_k` olmalı.

Kontrol edilecek anahtarlar: `keys(R) ∪ keys(base) ∪ {[F,n)'de yazılan anahtarlar}`, **bayt sırasıyla**.
Hiçbir yerde geçmeyen anahtar için `C_k = {None}`.
`[F,n)` dışındaki (öncesindeki) eski değerler izinli değildir: bunlar fsync sınırının altındadır.
Karmaşıklık: O((W + |R|) · log K) zaman, O(W) bellek (değer kopyaları dahil).

**Neden bu gevşek kural doğru?** Sıralama garantisi yoksa motor, farklı anahtarlara yapılan dayanıksız
yazmaları birbirinden bağımsız kaybedebilir (ör. farklı sayfalar, farklı dosyalar). Ama tek bir anahtar
için kurtarılan değer yine (a) o anahtara gerçekten yazılmış bir değer olmalı, (b) tabanda dayanıklı
olandan daha eski olmamalı. Bu, "anahtar başına prefix"tir.

**Atomik txn sağlam (sound) kontrolü — yalnız `atomic_txn && !prefix_order`, tüm anahtarlar
`C_k` kontrolünü geçtikten sonra:** `F ≤ T < n` olan her `Op::Txn` (artan sırada) için, Txn'in
yazdığı her farklı anahtar `k` (bayt sırasıyla), `post_k` = Txn içinde `k`'ye son yazılan değer:
- `(T, n)` aralığında `k`'ye yazan başka işlem varsa → atla (bilgi vermez).
- `pre_k` = `{ val_k(Cut::at_op(c)) : F ≤ c ≤ T }`.
- `R.get(k) != post_k` → **NotApplied**; `R.get(k) == post_k` ve `post_k ∉ pre_k` → **Applied**;
  aksi → belirsiz (yok say).
- Aynı Txn'de hem Applied hem NotApplied anahtar varsa → `PartialTxn` (§4.6(c)). Bu kontrol
  **sağlamdır** (yalnız kesin çelişkide ihlal der; yanlış alarm yok) ama **tam değildir** (bazı kısmi
  txn'ler kaçabilir) — bu, öğretici yorumda açıkça yazılır.
- Hiç ihlal yoksa `Allowed { witness: None }`.

### 4.5 `check_reads`
```
S = boş
for (i, e) in entries:
    if op Get{k} && outcome Read{value}: if S.get(k) != value → ReadMismatch (§4.6(d))
    op i'nin adımlarını S'ye uygula (sonuçtan bağımsız; I1 gereği belirsiz yalnız sonuncu)
Allowed { witness: None }
```
Failed/Issued/InFlight Get kontrol edilmez.

### 4.6 İhlal sınıflandırması ve alan doldurma (birebir)

**(a) Anahtar-bazlı (ilk başarısız anahtar `k`, `found = R.get(k)`, `R.get(k) ∉ C_k`):**
`Wall_k` = `[0,n)`'de `k`'ye yazılmış tüm Put değerleri (txn ara değerleri dahil);
`Wpre_k` = `[0,F)`'de `k`'ye yazılmış tüm Put değerleri; `last_pre_k` = `[0,F)`'de `k`'ye yazan
(Put/Delete/Txn) son işlemin indeksi.
Sırayla ilk uyan kural:
1. `found = Some(v)`, `v ∉ Wall_k` → **PhantomValue**, `op_index = None`.
2. `found = None` → **LostDurableWrite**, `op_index = Some(last_pre_k)`. (`None ∉ C_k` ⇒ `base_k` Some ⇒
   `last_pre_k` vardır.)
3. `found = Some(v)`, `v ∈ Wpre_k`: `base_k == None` → **ResurrectedDelete**, aksi →
   **LostDurableWrite**; her ikisinde `op_index = Some(last_pre_k)`.
4. Aksi (v yalnız `[F,n)`'de atomik bir Txn'in ara yazması olarak geçiyor; atomik olmayan modda
   bu dal erişilemez — `debug_assert!(atomic_txn)`) → **PartialTxn**, `op_index` = `[F,n)`'de `k`'ye
   `v`'yi ara (son olmayan) yazma olarak yazan ilk Txn'in indeksi.
Alanlar: `expected = C_k` (sıralı, tekrarsız), `best_cut = None`, `durable_floor = Some(F)`.

**(b) Çapraz-anahtar (yalnız prefix modu; tüm anahtarlar (a)'yı geçti):** `p* = best.cut` (§4.3).
`M` = `state_at(p*)` ile `R`'nin farklı olduğu anahtarlar (bayt sırası, boş olamaz), `k = M[0]`.
- `atomic_txn` ise, sırayla `T = p*.ops - 1` (varsa ve `T ≥ F`) ve `T = p*.ops` (`< n` ise) adayları
  için: `op T` bir Txn ve `M`'nin her anahtarı `T` tarafından yazılıyorsa → **PartialTxn**,
  `op_index = Some(T)`.
- Aksi → **PrefixViolation**, `op_index`:
  `q` = izinli kesme listesinde `val_k(q) == R.get(k)` olan ve liste konumu `p*`'a en yakın kesme
  (eşitlikte sonraki). `s(c)` = c'ye kadar uygulanan adım sayısı.
  `q < p*` → `[s(q), s(p*))` adım aralığında `k`'ye yazan **son** adımın işlem indeksi;
  `q > p*` → `[s(p*), s(q))` aralığında `k`'ye yazan **ilk** adımın işlem indeksi.
  (`q` vardır, çünkü `k` (a)'yı geçti; aralıkta `k`'ye yazan adım vardır, çünkü `val_k` farklı.)
Alanlar: `expected = vec![val_k(p*)]`, `found = R.get(k)`, `best_cut = Some(p*)`,
`durable_floor = Some(F)`.

**(c) Atomik txn sağlam kontrolü (§4.4):** `k` = ilk NotApplied anahtar, `expected = vec![post_k]`,
`found = R.get(k)`, `op_index = Some(T)`, `best_cut = None`, `durable_floor = Some(F)`.

**(d) ReadMismatch:** `key = k`, `expected = vec![S.get(k)]`, `found = value`, `op_index = Some(i)`,
`best_cut = None`, `durable_floor = None`.

### 4.7 Öğretici yorumlar (zorunlu içerik — kısa ama "neden" anlatan)
- `lib.rs` crate doc: izinli küme fikri (karar 3), neden tek beklenen durum değil; prefix vs anahtar-bazlı
  mod; modelin motoru **bildirdiği** garantilerle yargıladığı.
- `guarantees.rs`: ACK ≠ dayanıklılık; `OnSync`'te ACK'li ama sync'lenmemiş verinin neden sayfa
  önbelleğinde kalıp kaybolabileceği (fsync sınırı); `prefix_order`'ın sıralı WAL + checksum'lı kayıtta
  "ilk bozuk kayıtta dur" replay'inden doğduğu.
- `history.rs`: neden yalnız son işlem belirsiz olabilir (tek iş parçacığı + Failed'da durma);
  `durable_floor`'un fsync sınırının modeldeki karşılığı olduğu; in-flight sync'in tabanı neden
  yükseltmediği; `crash_view`'in Aşama 4'te iz kesme noktasıyla nasıl eşleşeceği.
- `check.rs`: uyuşmazlık sayacı değişmezi ve tam üyelik gerekçesi; neden "en son eşleşen kesme" tanık
  seçildiği (determinizm + en çok bilgi); atomik txn'de kesmelerin neden yalnız işlem sınırında olduğu;
  anahtar-bazlı kontrolün sağlam-ama-tam-değil olduğu; sınıflandırmanın hükmü değiştirmediği.
- `verdict.rs`: her `ViolationKind`'in tipik gerçek hata sınıfı: fsync atlama → LostDurableWrite;
  WAL replay'de son kaydı atma → LostDurableWrite/PrefixViolation; tombstone yoksayma →
  ResurrectedDelete; kısmi kaydı geçerli sayma → PhantomValue/PartialTxn; rename öncesi fsync yapmama →
  LostDurableWrite/PrefixViolation. (Bu eşlemeler Aşama 5'te ÖLÇÜLECEK; yorumda "beklenen" diye yazılır.)

### 4.8 Edition 2024 / lint notları
- `gen` ayrılmış anahtar sözcük; değişken adı olarak kullanma.
- `#[must_use]` Verdict üzerinde; testlerde sonucu her zaman kullan (assert).
- `unwrap()` kütüphane kodunda yalnız değişmezle kanıtlanmış yerde; tercihen
  `expect("değişmez: ...")` ile ve yanında neden kanıtlandığını anlatan yorumla.

---

## 5. Kenar Durumlar ve Değişmezler (özet)
- Boş geçmiş: `F = 0`, `cuts = [Cut::at_op(0)]`; `R = {}` → Allowed(Cut{0,0}); dolu R → PhantomValue.
- Son işlem Issued (mark_crashed çağrılmamış) → InFlight ile aynı muamele.
- Failed işlem: etkisi belirsiz (uygulanmış/uygulanmamış ikisi de izinli); OnAck tabanına girmez.
- Get/Sync durum değiştirmez; ancak kesme üretir (prefix modunda tanık, aynı durumun en son kesmesidir).
- Boş Txn: 0 adım; geçerli.
- Aynı anahtara txn içinde çift yazma: atomikse ara değer izinli DEĞİL; değilse izinli.
- `state_at`, `crash_view` yanlış argümanda panic eder (programlama hatası); `check_*` hiçbir geçerli
  `History` için panic etmez.

---

## 6. Kabul Kriterleri

### 6.1 Test DSL (`tests/common/mod.rs`) — birebir imzalar
Dosyanın başında `#![allow(dead_code)]` ve gerekçe yorumu ("her test dosyası her yardımcıyı
kullanmaz; dead_code uyarısı -D warnings ile CI'ı kırar"). Her `tests/*.rs` dosyası `//!` ile başlayan
kısa bir crate doc yorumu içerir.
```rust
pub fn k(s: &str) -> Key;                          // s.as_bytes().to_vec()
pub fn v(s: &str) -> Value;
pub fn put(key: &str, val: &str) -> Op;
pub fn del(key: &str) -> Op;
pub fn get(key: &str) -> Op;
pub fn txn(writes: Vec<Write>) -> Op;
pub fn wput(key: &str, val: &str) -> Write;
pub fn wdel(key: &str) -> Write;
pub fn ok() -> Outcome;                            // Outcome::Acked
pub fn read(val: Option<&str>) -> Outcome;         // Outcome::Read
pub fn failed() -> Outcome;                        // Outcome::Failed{reason:"io".into()}
pub fn inflight() -> Outcome;                      // Outcome::InFlight
pub fn hist(entries: Vec<(Op, Outcome)>) -> History;   // from_entries(...).expect(...)
pub fn state(pairs: &[(&str, &str)]) -> State;
pub fn g(d: Durability, atomic_txn: bool, prefix_order: bool) -> Guarantees;
pub fn some(s: &str) -> Option<Value>;             // Some(v(s))
```

### 6.2 Test tablosu
Kısaltmalar: `P(a,1)`=put("a","1"), `D(a)`=del, `G(a)`=get, `T[...]`=txn, `S`=Sync;
sonuç: `✓`=ok(), `?`=inflight(), `✗`=failed(), `I`=Outcome::Issued, `r=x`=read(Some x), `r=∅`=read(None).
Garanti kısaltmaları: **ACK**=g(OnAck,true,true), **SYNC**=g(OnSync,true,true),
**NONE**=g(NoGuarantee,true,true). Allowed(c) = `Verdict::Allowed{witness: Some(c)}`,
Allowed(—) = `witness: None`. `Cn` = `Cut::at_op(n)`.
İhlal satırları `kind, key, expected, found, op_index, best_cut, durable_floor` alanlarının
**hepsini** assert eder (`Violation` struct eşitliği ile).

**`tests/history.rs`**
| ID | Senaryo | Beklenen |
|----|---------|----------|
| H01 | new → issue(P(a,1))=0, ack(0); issue(G(a))=1, ack_read(1,some 1); issue(S)=2, ack(2) | entries outcomes [Acked, Read{Some 1}, Acked] |
| H02 | issue(P) (ack yok) → issue(P) | Err(PendingOp{index:0}) |
| H03 | issue(P)=0 → ack(5); sonra ack(0) ok; tekrar ack(0) | Err(NotPending{5}); Ok; Err(NotPending{0}) |
| H04 | issue(G(a))=0 → ack(0); yeni geçmişte issue(P)=0 → ack_read(0,None) | Err(OutcomeMismatch{0}) ikisinde |
| H05 | issue(P)=0, fail(0,"io") → issue(P) | Err(Closed) |
| H06 | issue(P)=0 → mark_crashed → entries[0].outcome; issue(P) | InFlight; Err(Closed). Tamamı Acked geçmişte mark_crashed → değişiklik yok |
| H07 | from_entries [P(a,1)?, P(b,2)✓]; [P✗, P✓]; [P I, P✓] | Err(UncertainNotLast{0}) üçünde |
| H08 | from_entries [P(a,1) r=1]; [G(a) ✓] | Err(OutcomeMismatch{0}) ikisinde |
| H09 | [P(a,1)✓, S✓, P(b,2)✓, S?] durable_floor; [P✓,S✓,P✓] OnAck | OnAck=3, OnSync=2, NoGuarantee=0; ikinci: OnAck=3 |
| H10 | [P(a,1)✓,S✓,P(b,2)✓]: crash_view(2,true); crash_view(3,false); crash_view(0,false); crash_view(4,false) | [P✓,S✓,P(b,2)?]; orijinale eşit; boş; **panic** (`#[should_panic]`, ayrı test fonksiyonu) |
| H11 | [P(a,1)✓, T[wput(b,2), wdel(a)]✓, P(c,3)✓]: state_at C0, Cut{1,1}, C2, C3; state_at(Cut{0,1}) | {}, {a:1,b:2}, {b:2}, {b:2,c:3}; son çağrı **panic** (ayrı test fonksiyonu) |

**`tests/recovered.rs`** (`check_recovered(&h, g, &r)`)
| ID | Garanti | Geçmiş | R | Beklenen |
|----|---------|--------|---|----------|
| R01 | SYNC | [] | {} | Allowed(C0) |
| R02 | SYNC | [] | {a:1} | PhantomValue, a, [None], some 1, None, None, Some(0) |
| R03 | ACK | [P(a,1)✓, P(b,2)✓] | {a:1,b:2} | Allowed(C2) |
| R04 | ACK | [P(a,1)✓, P(b,2)✓] | {a:1} | LostDurableWrite, b, [some 2], None, Some(1), None, Some(2) |
| R05 | ACK | [P(a,1)✓, P(b,2)?] | {a:1,b:2} | Allowed(C2) |
| R06 | ACK | [P(a,1)✓, P(b,2)?] | {a:1} | Allowed(C1) |
| R07 | ACK | [P(a,1)✓, P(b,2)✗] | {a:1} ve {a:1,b:2} | Allowed(C1) ve Allowed(C2) |
| R08 | SYNC | [P(a,1)✓, S✓, P(b,2)✓] | {a:1} | Allowed(C2) |
| R09 | SYNC | [P(a,1)✓, S✓, P(b,2)✓] | {} | LostDurableWrite, a, [some 1], None, Some(0), None, Some(2) |
| R10 | SYNC | [P(a,1)✓, P(b,2)✓] | {} | Allowed(C0) |
| R11 | NONE | [P(a,1)✓, S✓] | {} | Allowed(C0) |
| R12 | NONE | [P(a,1)✓] | {a:9} | PhantomValue, a, [None, some 1], some 9, None, None, Some(0) |
| R13 | SYNC | [P(a,1)✓, P(b,2)✓] | {b:2} | PrefixViolation, a, [some 1], None, Some(0), Some(C2), Some(0) |
| R14 | g(OnSync,true,false) | [P(a,1)✓, P(b,2)✓] | {b:2} | Allowed(—) |
| R15 | g(OnAck,true,false) | [P(a,1)✓] | {} | LostDurableWrite, a, [some 1], None, Some(0), None, Some(1) |
| R16 | SYNC | [T[wput(a,1),wput(b,2)]✓] | {} ve {a:1,b:2} | Allowed(C0) ve Allowed(C1) |
| R17 | SYNC | [T[wput(a,1),wput(b,2)]✓] | {a:1} | PartialTxn, b, [some 2], None, Some(0), Some(C1), Some(0) |
| R18 | g(OnSync,false,true) | [T[wput(a,1),wput(b,2)]✓] | {a:1} | Allowed(Cut{ops:0,txn_writes:1}) |
| R19 | ACK | [P(c,0)✓, T[wput(a,1),wput(b,2)]?] | {c:0,a:1} | PartialTxn, b, [some 2], None, Some(1), Some(C2), Some(1) |
| R20 | ACK | [T[wput(a,1),wput(b,2)]✓] | {a:1} | LostDurableWrite, b, [some 2], None, Some(0), None, Some(1) (anahtar-bazlı sınıf önceliği) |
| R21 | SYNC | [P(a,1)✓, S✓, P(a,2)✓, P(a,3)✓] | {a:2}; {a:1}; {} | Allowed(C3); Allowed(C2); LostDurableWrite, a, [some 1, some 2, some 3], None, Some(0), None, Some(2) |
| R22 | SYNC | [P(a,1)✓, P(a,2)✓, S✓] | {a:1} | LostDurableWrite, a, [some 2], some 1, Some(1), None, Some(3) |
| R23 | ACK | [P(a,1)✓, D(a)✓] | {a:1} | ResurrectedDelete, a, [None], some 1, Some(1), None, Some(2) |
| R24 | SYNC | [P(a,1)✓, S✓, D(a)✓] | {a:1}; {} | Allowed(C2); Allowed(C3) |
| R25 | ACK | [P(a,1)✓, D(a)✓, P(a,2)✓] | {a:2}; {a:1}; {} | Allowed(C3); LostDurableWrite, a, [some 2], some 1, Some(2), None, Some(3); LostDurableWrite, a, [some 2], None, Some(2), None, Some(3) |
| R26 | ACK | [P(a,1)✓, D(a)✓, P(a,2)?] | {}; {a:2}; {a:1} | Allowed(C2); Allowed(C3); ResurrectedDelete, a, [None, some 2], some 1, Some(1), None, Some(2) |
| R27 | ACK | [P(a,1)✓, P(b,2)✓] | {a:1,b:1} | PhantomValue, b, [some 2], some 1, None, None, Some(2) |
| R28 | ACK | [P(a,1)✓] | {a:1,z:5} | PhantomValue, z, [None], some 5, None, None, Some(1) |
| R29 | g(OnSync,true,false) | [T[wput(a,1),wput(b,2)]✓] | {a:1}; {} | PartialTxn, b, [some 2], None, Some(0), None, Some(0); Allowed(—) |
| R30 | SYNC ve g(OnSync,false,true) | [T[wput(a,1),wput(a,2)]✓] | {a:1} | SYNC: PartialTxn, a, [None, some 2], some 1, Some(0), None, Some(0); atomik değil: Allowed(Cut{0,1}) |
| R31 | SYNC | [P(a,1)✓, G(a) r=1] | {a:1} | Allowed(C2) (en son eşleşen kesme) |
| R32 | SYNC | R13 girdisi iki kez | {b:2} | iki Verdict birbirine eşit (determinizm) |
| R33 | SYNC | [P(a,1)✓, S?] | {} | Allowed(C0) (in-flight sync tabanı yükseltmez) |
| R34 | g(NoGuarantee,false,false) | [P(a,1)✓, P(b,2)✓] | {b:2} | Allowed(—) |
| R35 | ACK | [P(a,1)✓, P(b,2) I] | {a:1} | Allowed(C1) (Issued = belirsiz) |
| R36 | SYNC | [P(a,1)✓, S✓, P(a,2)✓, P(b,3)✓, P(a,4)✓] | {a:4} | PrefixViolation, b, [some 3], None, Some(3), Some(C5), Some(2) |

R13 ve R36'nın türetimi (implementer doğrulaması için): R36'da kesmeler C2..C5, uyuşmazlık sayıları
1,1,2,1 → en az ve en son p*=C5; M={b}; op4 Txn değil → PrefixViolation; `val_b`: C2=∅, C3=∅, C4=3, C5=3;
p*'a en yakın `∅` kesmesi C3 (q<p*) → [s(C3)=2, s(C5)=4) adımlarından b'ye son yazan adım op3'e ait.
R13'te kesmeler C0..C2, sayılar 1,2,1 → p*=C2, M={a}, q=C0 → [0,2) içinde a'ya son yazan op0.

**`tests/reads.rs`** (`check_reads(&h)`)
| ID | Geçmiş | Beklenen |
|----|--------|----------|
| D01 | [P(a,1)✓, G(a) r=1, D(a)✓, G(a) r=∅] | Allowed(—) |
| D02 | [P(a,1)✓, G(a) r=∅] | ReadMismatch, a, [some 1], None, Some(1), None, None |
| D03 | [P(b,0)✓, T[wput(a,1), wdel(b)]✓, G(b) r=∅, G(a) r=1] | Allowed(—) |
| D04 | [] | Allowed(—) |
| D05 | [P(a,1)✓, G(a)✗] | Allowed(—) (başarısız Get kontrol edilmez) |
| D06 | [P(a,1)✓, P(a,2)✓, G(a) r=1] | ReadMismatch, a, [some 2], some 1, Some(2), None, None |

Toplam: 11 + 36 + 6 = **53 senaryo ID'si**; H10 ve H11'in panic alt-testleri ayrı `#[should_panic]`
fonksiyonları olduğundan en az **55 test fonksiyonu**. Bir ID'deki birden çok R/beklenen çifti aynı
fonksiyonda ardışık assert edilir.

### 6.3 Aşama 1 "bitti" ölçütü (hepsi temiz geçmeli, proje kökünde)
1. `cargo fmt --all -- --check` → çıktı yok, çıkış kodu 0.
2. `cargo clippy --workspace --all-targets --locked -- -D warnings` → 0 uyarı.
3. `cargo test --workspace --locked` → §6.2'deki tüm testler (+ kaba kuvvet testi) geçer, 0 başarısız,
   0 ignored.
4. `cargo tree -p cc-model --edges normal` → yalnız `cc-model` satırı (dış bağımlılık yok).
5. `grep -rn "HashMap\|HashSet" crates/cc-model/src` → sonuç yok.
6. `grep -rn "unsafe" crates/cc-model/src` → sonuç yok.
7. `.github/workflows/ci.yml` §1.5 ile birebir; `Cargo.lock` mevcut.
8. Tüm public öğeler `///` doküman yorumlu; §4.7'deki öğretici yorum konuları ilgili dosyalarda mevcut
   (code-reviewer kontrol eder).

---

## 7. Dosya Sahipliği

| Dosya / Dizin | Sahip |
|---------------|-------|
| `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `Cargo.lock` | general-implementer |
| `.github/workflows/ci.yml` | general-implementer |
| `crates/cc-model/Cargo.toml`, `crates/cc-model/src/**` | general-implementer |
| `crates/cc-model/tests/**` | test-engineer |
| `docs/**` | mimar (bu SPEC); implementer'lar DEĞİŞTİRMEZ |

---

## Görev Dağılımı (orchestrator buna göre delege eder)

| Alt Görev | Sorumlu Uygulayıcı (sonnet) | Dosya Kapsamı | Bağımlılık |
|-----------|------------------------------|----------------|------------|
| Workspace iskeleti + toolchain + CI | general-implementer | `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `.github/workflows/ci.yml` | - |
| `cc-model` kütüphanesi (op, history, guarantees, verdict, check) + öğretici yorumlar | general-implementer | `crates/cc-model/Cargo.toml`, `crates/cc-model/src/**` | iskelet (aynı implementer, sıralı) |
| Kontrat testleri (§6.1 DSL + §6.2 tablosu + kaba kuvvet testi) | test-engineer | `crates/cc-model/tests/**` | yalnız §3 public API (kütüphaneyle **paralel** yazılabilir; koşum kütüphane bitince) |

Not: Bu aşamada ROS2/web/gömülü/DB işi yoktur; Rust kütüphanesi + CI tek bir uzmanlığa düşer, bu yüzden
tek uygulayıcı (general-implementer). Testler kontrattan bağımsız yazıldığı için test-engineer'a verildi:
böylece testler implementasyonun değil SPEC'in yansıması olur.

## Doğrulama Planı (orchestrator buna göre test ettirir)

| Ne Doğrulanacak | Doğrulayıcı Ajan | Yöntem |
|------------------|------------------|--------|
| §6.2'deki 53 senaryo (≥55 test fonksiyonu) | test-engineer | `cargo test --workspace --locked` |
| fmt / clippy / bağımlılık / HashMap ve unsafe yasağı (§6.3 madde 1,2,4,5,6) | test-engineer | komutları koş, çıktıyı raporla |
| Prefix modu üyeliğinin kaba kuvvetle (tüm izinli kesmeleri `state_at` ile tek tek R ile karşılaştırma) aynı Allowed/Violated sonucunu verdiği | test-engineer | `tests/recovered.rs` içinde ek test: elle yazılmış deterministik PRNG (SplitMix64, dış crate YOK), sabit seed, ≥500 rastgele küçük geçmiş (≤8 işlem, ≤3 anahtar, ≤3 değer, tüm Guarantees kombinasyonları prefix_order=true ile), R = rastgele bir izinli kesmenin durumu veya rastgele bozulmuş hali |

ui-simulator (web yok) ve sim-runner (araç/robot yok) bu projede uygulanamaz.

## Denetim Planı

| Alan | Denetçi | Neden |
|------|---------|-------|
| kod kalitesi, SPEC'e birebir uyum, öğretici yorumlar (§4.7), determinizm | code-reviewer | her zaman |
| — | security-auditor | Aşama 1'de hassas alan yok (saf mantık, `unsafe` yok, I/O yok). **Aşama 7'de (cc-shim, LD_PRELOAD, `unsafe`) zorunlu.** |

### Seçim İçin Ajan Kataloğu (buradan seçildi)
UYGULAYICILAR: frontend-implementer, backend-implementer, ros2-implementer, embedded-implementer,
docker-implementer, k8s-implementer, java-implementer, ml-implementer, database-implementer,
general-implementer (Python/Rust/Go/CLI). DOĞRULAYICILAR: test-engineer, ui-simulator, sim-runner.
DENETÇİLER: code-reviewer, security-auditor.
