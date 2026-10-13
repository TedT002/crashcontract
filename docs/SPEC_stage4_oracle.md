# Aşama 4 — Oracle: iş yükü üreteci + runner + `crashcontract run` — Teknik Sözleşme

> Bağlam: `PLAN_crashcontract.md` Aşama 4; `SPEC_stage2` §7.1 (kurtarma reddi), `SPEC_stage3`.
> Dış bağımlılık yok (argüman ayrıştırma elle; paralellik `std::thread::scope`).

## 0. Kararlar
1. **`cc-workload`**: SplitMix64 + `generate(seed, &WorkloadConfig) -> Vec<Op>`. seed → iş yükü eşlemesi
   kontrattır (Aşama 6 replay): algoritma ve dağılım değişirse rapor şeması sürümü artar.
2. **Runner `cc_adapter::runner`** içinde (yeni crate yok): her `StorageAdapter` için genel; Aşama 6
   minimizer'ı da aynı fonksiyonu çağırır.
3. **Kurtarma reddi cc-model'e EKLENMEZ** (Aşama 2 açık soru 1): runner kendi `Finding` tipini taşır —
   `Violation(cc_model::Violation)` | `RecoveryFailed` | `ScanFailed` | `IntegrityFailed`. Gerekçe:
   `Violation` anahtar/değer alanları açılamayan motor için anlamsız; cc-model kontratı değişmez.
   Motorun hiç açılamaması her zaman bulgudur (doğru motor her çökme durumundan açılabilmelidir).
4. **Seed başına ilk bulguda dur** (minimizer tek bir karşı örnekle çalışır).
5. **Kesme seçimi**: `All` (0..=olay sayısı) ya da `Sample(k)` — seed'li, tekrarsız, artan; olay sayısı ≤ k
   ise hepsi. 10k seed ölçümü `Sample` ile yapılır (süre gerçekçi kalsın); duman testleri `All`.
6. Okuma doğrulaması (`check_reads`) çökmesiz koşu üzerinde, çökme durumlarından önce.

## 1. API
```rust
// cc-workload
pub struct SplitMix64(pub u64);  impl { pub fn next(&mut self) -> u64; pub fn below(&mut self, n: u64) -> u64; }
pub struct WorkloadConfig { pub min_ops: u32, pub extra_ops: u32, pub key_space: u32, pub max_value_len: u32,
                            pub max_txn_writes: u32, pub weights: OpWeights }  // Default: 20, 100, 8, 16, 4
pub struct OpWeights { pub put: u32, pub delete: u32, pub get: u32, pub txn: u32, pub sync: u32 } // 40/15/25/15/5
pub fn generate(seed: u64, cfg: &WorkloadConfig) -> Vec<Op>;

// cc_adapter::runner
pub enum CutSelection { All, Sample(usize) }
pub struct RunConfig { pub crash: CrashConfig, pub cuts: CutSelection }
pub enum Finding { Violation(Violation), RecoveryFailed(String), ScanFailed(String), IntegrityFailed(String) }
pub struct FailureCase { pub cut: Option<usize> /* None = çökmesiz okuma */, pub crash_point: Option<CrashPoint>,
                         pub choice: Option<Choice>, pub recovered: Option<State>, pub finding: Finding }
pub struct SeedOutcome { pub ops: usize, pub events: usize, pub cuts: usize, pub states: usize,
                         pub failure: Option<FailureCase> }
pub fn run_case<A: StorageAdapter>(ops: Vec<Op>, config: &A::Config, workdir: &Path, seed: u64,
                                   rc: &RunConfig) -> Result<SeedOutcome, RunError>;
```
`workdir` altında `run/` (kayıtlı koşu) ve `crash/` (her durum için silinip yeniden yaratılır) kullanılır.
`RunError` = altyapı hatası (çökmesiz koşuda açılış/IO, iz tutarsızlığı) — bulgu değildir.

## 2. CLI
`crashcontract run [--engine toy-correct] [--seeds A..B] [--policy every-commit|on-sync]
[--checkpoint-every N] [--cuts all|N] [--torn] [--jobs N] [--workdir DIR]`
Çıktı: seed, işlem, olay, kesme, durum sayıları, bulgu sayısı, süre; bulgu varsa ilkinin ayrıntısı.
Çıkış kodu: 0 temiz, 1 bulgu, 2 kullanım/altyapı hatası.

## 3. Kabul
- `cc-workload` testleri: determinizm, sınırlar, dağılım (10k işlemde ağırlıklara ±%3).
- Runner testleri (toy-correct dev-test): temiz seed'ler bulgu vermez; elle bozuk bir adaptör
  `Violation` ve `RecoveryFailed` üretir; `Sample(k)` determinizmi.
- CLI testi: küçük aralıkta çıkış 0 ve özet satırı; hatalı argüman → 2.
- **Ölçüm:** `toy-correct` × 10.000 seed (iki politika, checkpoint açık/kapalı karışık) → bulgu 0;
  kesme/durum sayısı ve süre README'ye girecek şekilde kaydedilir (`docs/MEASUREMENTS.md`).
