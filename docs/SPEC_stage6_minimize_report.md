# Aşama 6 — Minimizer + JSON rapor + `replay` — Teknik Sözleşme

> Bağlam: CLAUDE.md "Hata bulunursa dizi otomatik küçültülür ve tek komutla yeniden oynatılabilir bir
> rapor üretilir"; Aşama 6 maddesi; `SPEC_stage4` (runner).

## 0. Kararlar
1. **İlk dış bağımlılıklar:** `serde` (derive) — rapor tipleri cc-model/cc-trace/cc-adapter/toy-correct/
   cc-workload'da; `serde_json` — yalnız `cc-cli`. Gerekçe: JSON'u elle yazmak/okumak hataya açık; serde
   Rust'ta fiili standart. Her `Cargo.toml`'da neden yorumu.
2. **Bayt gösterimi:** anahtar/değer = okunabilir, tersinir kaçışlı dize (`0x20..=0x7E` aynen, `\` →
   `\\`, diğerleri `\xNN`); iz verisi (`Write.data`) = onaltılık dize. `cc_model::bytes_repr`.
3. **`History` serileştirmesi** kayıt listesidir; okurken `History::from_entries` doğrular (I1/I2).
4. **`FailureCase` genişler:** tam koşu geçmişi (`history`) ve izi (`trace`) taşır — rapor kendi başına
   incelenebilsin, replay iz eşitliğini de doğrulasın. (Eklemeli; Aşama 4 alanları değişmez.)
5. **Minimizer (`cc-minimize`)**, sırayla: (a) işlem dizisi ddmin (Zeller–Hildebrandt, 1-minimal);
   (a') her txn'in yazma listesi ddmin; (b) anahtar uzayı: bir anahtarı daha önceki bir anahtarla birleştir; (c) değer boyutu: boş, sonra
   yarıya; (d) değiştiyse ddmin tekrar; (e) anahtarları görünüş sırasıyla `a`,`b`,… yeniden adlandır.
   Her adım yalnız **aynı bulgu sınıfı** (Finding varyantı + ViolationKind) korunursa kabul edilir.
   **Çökme noktası:** runner kesmeleri artan, durumları "en az kalıcı" sırasıyla dener → küçültülmüş
   dizide raporlanan, ilk (en erken) bozuk kesme ve o kesmedeki ilk seçimdir.
6. **Seed sabit kalır** (çökme durumu örneklemesi seed'e bağlı); rapordaki `ops` küçültülmüş dizidir,
   `seed` yalnız örnekleme ve kaynağı belirtir.
7. **Rapor şeması** `schema_version = 1`: `tool_version`, `engine` ({`name`, `config`}), `guarantees`,
   `seed`, `workload` (üreteç yapılandırması), `run` (RunConfig), `ops`, `minimization` (önce/sonra işlem,
   test sayısı), `failure` (FailureCase), `finding_text` (insan için; replay karşılaştırmaz).
8. **Replay:** raporu oku → `run_case(ops, config, seed, run)` → `failure` alanı **bire bir eşit** mi
   (kesme, crash_point, seçim, kurtarılan durum, bulgu, geçmiş, iz). Çıkış 0 = aynı, 1 = farklı, 2 = hata.

## 1. CLI
- `run`: bulgu varsa ilk bulgulu seed otomatik küçültülür, rapor `--report-dir` (varsayılan `failures/`)
  altına `<motor>-seed<N>.json` olarak yazılır; `--no-minimize` küçültmeyi atlar (rapor yine yazılır).
- `minimize --seed N [run seçenekleri] [--out FILE]`: tek seed'i koşar, bulgu varsa küçültüp raporlar.
- `replay FILE`: yukarıdaki §0.8.

## 2. Kabul
- ddmin birim testleri (bilinen minimum, 1-minimallik, boş girdi, determinizm).
- `bytes_repr` gidiş-dönüş (tüm 256 bayt, `\`, `"`, boş) ve bozuk girdi reddi.
- Her mutant: küçültülmüş rapor aynı sınıfı korur; JSON gidiş-dönüş eşit; **replay bire bir aynı**.
- Ölçüm: mutant başına küçültme oranı (önce/sonra işlem) → `docs/MEASUREMENTS.md`.
