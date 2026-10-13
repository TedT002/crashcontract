# CrashContract — Mimari Plan (yol haritası)

> Kaynak: `/CLAUDE.md` (tasarım kararları 1–5 bağlayıcıdır, bu plan onları değiştirmez).
> Bu belge 8 aşamanın **ne** üreteceğini ve **nasıl kabul edileceğini** sabitler. Her aşamanın
> bağlayıcı sözleşmesi ayrı bir `docs/SPEC_stageN_*.md` dosyasında, o aşama başlamadan önce yazılır.
> Şu an yazılmış olan: `docs/SPEC_stage1_model.md`.

## 1. Problem & Kısıtlar

- **Fonksiyonel:** Bir depolama motoruna seed'li iş yükü uygula → dosya sistemi çağrılarını kaydet →
  izden izinli çökme durumları üret → motoru her durumdan aç → kurtarılan durumu izinli küme modeliyle
  karşılaştır → hatayı küçült → yeniden oynatılabilir JSON rapor üret.
- **NFR — determinizm:** Aynı (seed, motor, yapılandırma, araç sürümü) → bit düzeyinde aynı çökme
  durumları ve aynı hüküm. `HashMap` iterasyon sırası, saat, iş parçacığı zamanlaması hüküm yoluna girmez.
- **NFR — doğruluk:** `toy-correct` üzerinde yanlış alarm = 0 (≥10.000 seed). Mutantların tamamı yakalanır.
- **NFR — ölçek (hedef, Aşama 4'te ölçülecek):** seed başına yüzlerce işlem, onlarca–yüzlerce çökme
  durumu; tek makinede 10k seed'in dakikalar mertebesinde koşması. Ölçülmeden README'ye yazılmaz.
- **Kapsam (karar 5):** tek iş parçacıklı sıralı iş yükü, Linux, syscall seviyesi. Güç kaybı, blok
  seviyesi, eşzamanlılık, mmap/io_uring kullanan motorlar kapsam dışı.
- **Ekip/bakım:** tek geliştirici + ajanlar; bağımlılık minimum, her crate gerekçeli.

## 2. Mimari Genel Bakış

```
 seed ──► cc-workload ──ops──► cc-cli runner ──► StorageAdapter (cc-adapter) ──► motor
                                   │                                       │ (Vfs / LD_PRELOAD shim)
                                   │ History (cc-model)                    ▼
                                   │                              iz (cc-trace: TraceEvent[])
                                   │                                       │
                                   │      crash-state üreteci (cc-trace) ◄─┘
                                   │                │ dizin görüntüsü + kesme noktası
                                   ▼                ▼
                        History.crash_view() ─► motoru yeniden aç → kurtarılan State
                                   │                │
                                   └──► cc-model::check_recovered(history, guarantees, state) ──► Verdict
                                                     │ Violated
                                                     ▼
                                       cc-minimize (ddmin) ──► failures/*.json ──► `replay`
```
Tamamı **senkron, tek süreç** (gerçek motor hariç: Aşama 7'de motor, shim yüklü alt süreçte koşar).
Asenkron/servis yok; bu bir CLI test aracıdır.

## 3. Değerlendirilen Seçenekler (bu planın verdiği kararlar)

| Konu | Seçenek A | Seçenek B | SEÇİM |
|------|-----------|-----------|-------|
| İzinli küme temsili | Tüm izinli durumları açıkça listele | Kümeyi "geçerli kesme noktaları aralığı" olarak sembolik tut; üyeliği artımlı uyuşmazlık sayacıyla tek geçişte test et | **B** — A üsteldir (atomik olmayan txn, OnSync penceresi); B O((n+W+\|R\|)·log K) ve tam (exact) üyelik testi |
| Workspace iskeleti | Tüm crate'ler Aşama 1'de boş iskelet | Her crate ihtiyaç duyulduğu aşamada eklenir | **B** — boş crate'ler tahmini API'ye davetiye; `members` açık liste, eklemek tek satır |
| Seed → iş yükü PRNG | `rand` crate | Kendi küçük PRNG'miz (SplitMix64) | **B** — `rand`'ın `StdRng` algoritması sürümler arası garanti edilmez; eski raporların replay'i bozulur (tek-yönlü kapı) |
| Serileştirme | Aşama 1'den serde | Aşama 6'da (JSON rapor) ekle | **B** — Aşama 1–5'te JSON ihtiyacı yok; derive'lar sonradan eklenir, kontrat bozulmaz |
| Toolchain | floating `stable` | `rust-toolchain.toml` ile 1.98.1 sabit | **B** — `clippy -D warnings` yeni stable'daki yeni lint'lerle CI'ı keyfi kırar |

## 4. Teknoloji Seçimleri

| Katman | Seçim | Neden | Alternatif (neden değil) |
|--------|-------|-------|--------------------------|
| Dil/derleyici | Rust 1.98.1, edition 2024, resolver 3 | Kurulu sürüm; 1.98.0'daki vtable miscompile 1.98.1'de düzeltildi | 1.98.0 (bilinen miscompile) |
| CI | GitHub Actions: fmt, clippy `-D warnings`, test | Talep edilen; ek servis yok | ek cache action'ı (şimdilik dış bağımlılık yok, gereksiz) |
| JSON (Aş. 6) | `serde` + `serde_json` | Rapor formatı; fiili standart | elle JSON (hata kaynağı) |
| CLI argümanı (Aş. 4/6) | elle ayrıştırma veya `lexopt` (o aşamanın SPEC'inde karar) | 3 alt komut; `clap` ağır | `clap` |
| Shim (Aş. 7) | `cdylib` + `libc` | LD_PRELOAD için C ABI | ptrace (yavaş, karmaşık) |

## 5. Aşamalar (modül dökümü + yol haritası)

### Aşama 1 — İskelet + model (`cc-model`)
- **Çıktı:** workspace kökü, `rust-toolchain.toml`, CI, `crates/cc-model` (sıfır dış bağımlılık).
- **İçerik:** `Op`/`History`/`Outcome`, `Guarantees{Durability, atomic_txn, prefix_order}`,
  `check_recovered`, `check_reads`, `Verdict`/`Violation`.
- **Kabul:** `cargo fmt --check`, `clippy -D warnings`, `cargo test` temiz; SPEC'teki ≥40 test geçer;
  `cargo tree -p cc-model` bağımlılık göstermez.
- **Bağımlılık:** yok. **Risk:** kontrat (Op/History/Guarantees) sonraki tüm aşamalarca kullanılır →
  tek-yönlü kapı; SPEC'te tam imzalarla sabitlendi.

### Aşama 2 — Trace + toy motor (`cc-trace`, `cc-adapter`, `engines/toy-correct`)
- **Çıktı:** `Vfs` trait'i (open/pwrite/write/fsync/fdatasync/rename/ftruncate/unlink/read/list),
  kaydeden `DirVfs` (gerçek dizin; `MemVfs` eklenmedi — SPEC_stage2 karar 8) → `Vec<TraceEvent>`; `StorageAdapter` trait'i
  (`guarantees()` cc-model tipini döndürür); `toy-correct`: WAL (uzunluk+CRC'li kayıt) + checkpoint
  (geçici dosyaya yaz → fsync → rename → dizin fsync).
- **Kabul:** toy-correct çökme olmadan 1.000 seed'de `check_reads` = Allowed; iz, aynı seed'de
  bayt-bayt aynı (determinizm testi); her `Vfs` çağrısı tam bir `TraceEvent` üretir.
- **Bağımlılık:** Aşama 1. **Risk:** `Vfs` trait'i mutantlar ve toy motorlar için tek-yönlü kapı;
  CRC için dış crate yerine ~20 satırlık CRC32 (gerekçe: bağımlılık minimum).

### Aşama 3 — Crash-state üreteci (`cc-trace::crash`)
- **Çıktı:** (iz, kesme noktası N, seed) → dizin görüntüsü. Kurallar: N. olayda kes; fsync/fdatasync
  ile sabitlenmemiş yazıların izinli alt kümelerini düşür; `rename`/`unlink`/oluşturma yalnızca dizin
  fsync'i sonrası kalıcı (muhafazakâr POSIX — ext4 `data=ordered` varsayılmaz); opsiyonel sektör (512B)
  seviyesinde torn write.
- **Kabul:** aynı (iz, seed) → aynı durum listesi (hash eşitliği testi); her kesme noktası için durum
  sayısı üst sınırlı (seed'li örnekleme, sınır SPEC'te); el ile yazılmış 10+ iz için beklenen durum
  kümeleri birebir.
- **Bağımlılık:** Aşama 2. **Risk:** alt küme patlaması (üstel) → deterministik örnekleme;
  muhafazakâr FS modeli gerçek motorda yanlış alarm üretebilir → Aşama 7'de ölçülüp not düşülür.

### Aşama 4 — Oracle (`cc-workload`, `cc-cli run`)
- **Çıktı:** SplitMix64 tabanlı seed'li iş yükü üreteci; runner: her işlem için (issue_event, ack_event)
  iz indekslerini tutar, kesme noktası N için `History::crash_view(completed, in_flight)` kurar,
  motoru görüntüden açar, `check_recovered` çağırır.
- **Kabul:** `toy-correct` üzerinde 10.000 seed, yanlış alarm = 0; koşulan çökme durumu sayısı ve süre
  ölçülüp kaydedilir (README için).
- **Bağımlılık:** Aşama 1–3. **Risk:** işlem↔iz eşlemesi hatası (in-flight işlemi yanlış belirlemek)
  sessiz yanlış alarm/kaçırma üretir → eşleme için ayrı birim testleri.

### Aşama 5 — Mutantlar (`engines/toy-mutants`)
- **Çıktı:** ≥6 mutant: fsync atla, fsync sırasını değiştir, WAL replay'de son kaydı at, tombstone'u
  kurtarmada yoksay, rename öncesi fsync yapma, kısmi (CRC'si bozuk) kaydı geçerli say.
- **Kabul:** her mutant için test: sabit seed aralığında (SPEC'te, ör. ≤1.000 seed) en az bir
  `Violated` ve **beklenen `ViolationKind`** (tablo SPEC'te); toy-correct aynı aralıkta temiz.
- **Bağımlılık:** Aşama 4. **Risk:** bir mutant yakalanmıyorsa sorun araçtadır (karar 4) — mutantı
  zayıflatmak yasak; üreteç/oracle düzeltilir.

### Aşama 6 — Minimizer + rapor + replay (`cc-minimize`, `cc-cli replay/minimize`)
- **Çıktı:** ddmin (işlem dizisi → çökme noktası → anahtar uzayı → değer boyutu); JSON rapor
  (`schema_version`, araç/motor sürümü, yapılandırma, seed, işlem geçmişi, iz, kesme noktası,
  kurtarılan durum, `Violation`); `serde`/`serde_json` burada eklenir (cc-model'e derive'lar).
- **Kabul:** her mutant için minimize rapor üretilir; `replay rapor.json` aynı `Violation`'ı bire bir
  verir (byte-eşit JSON çıktısı testi); minimizasyon oranı (önce/sonra işlem sayısı) ölçülür.
- **Bağımlılık:** Aşama 5. **Risk:** rapor formatı yazıldıktan sonra tek-yönlü kapı → `schema_version`.

### Aşama 7 — Gerçek motor adaptörü (`cc-shim`, `cc-adapter::<motor>`)
- **KURAL: Önce ölç, tahmin etme.** `strace -f -e trace=file,desc` (gerekirse `-e trace=memory` ile
  mmap kontrolü) ile motorun (ör. redb) gerçek çağrı dizisini ölç, çıktıyı `docs/` altına kaydet;
  fsync düzeni ve dosya düzeni hakkında **yalnızca ölçülen** şey yazılır.
- **Çıktı:** `cc-shim` (cdylib, LD_PRELOAD; yalnızca motorun veri dizinindeki fd'leri kaydeder),
  motor ayrı alt süreçte koşar; mmap/io_uring tespit edilirse kapsam dışı notu düşülür.
- **Kabul:** strace ölçümü ile shim izinin aynı çağrı sırasını verdiği test; gerçek motorda ≥1.000
  seed yanlış alarm = 0 (ya da her alarm incelenip "gerçek hata" veya "model kısıtı" olarak belgelenir).
- **Bağımlılık:** Aşama 6. **Risk:** doğrudan syscall yapan / statik bağlı motorlar LD_PRELOAD'u atlar;
  redb'nin kendi `StorageBackend` trait'i alternatif kayıt noktası olabilir — **doğrulanacak**, karar
  bu aşamanın SPEC'inde. `unsafe` yalnızca cc-shim'de, security-auditor denetimi zorunlu.

### Aşama 8 — Sunum
- **Çıktı:** README: yakalanan hata sınıfları tablosu, koşulan senaryo sayısı, minimizasyon oranı,
  "Doğrulanmayanlar" bölümü; CI'da mutant testleri; ikinci motor opsiyonel.
- **Kabul:** README'deki her sayı bir komutla yeniden üretilebilir (komut README'de yazılı);
  CI yeşil; tahmini iddia yok.
- **Bağımlılık:** Aşama 4–7.

## 6. Çapraz Kesen Konular

- **Gözlemlenebilirlik:** CLI çıktısı: koşulan seed/durum sayısı, ihlal özeti; ayrıntı JSON raporda.
- **Hata yönetimi:** kütüphane crate'leri `Result` döner; programlama hatası (kontrat ihlali) `panic`
  + `# Panics` dokümanı. Model hükmü (`Verdict`) bir hata değil, değerdir.
- **Determinizm:** hüküm yolunda yalnız `BTreeMap/BTreeSet`; PRNG kendi kodumuz; sıralama kuralları
  (ör. "en son eşleşen kesme noktası") SPEC'te sabit.
- **Güvenlik:** `unsafe_code = "deny"` workspace genelinde; yalnız cc-shim crate seviyesinde gerekçeli
  `allow`. Shim yalnızca hedef veri dizinini kaydeder.
- **Dağıtım/rollback:** CLI aracı; sürüm = workspace `version`; rapor `schema_version` taşır.

## 7. Geri Dönülmez Kararlar (tek-yönlü kapılar) & Açık Riskler

| Karar | Aşama | Neden tek-yönlü | Koruma |
|-------|-------|-----------------|--------|
| `Op`/`History`/`Guarantees`/`Violation` kontratı | 1 | tüm crate'ler + JSON rapor buna dayanır | SPEC'te tam imza; değişiklik = SPEC revizyonu |
| `Vfs` trait'i + `TraceEvent` formatı | 2 | toy motorlar, mutantlar, shim, rapor | Aşama 2 SPEC'inde sabitlenir |
| Crash-state FS semantiği (dizin fsync, fdatasync) | 3 | yanlış alarm/kaçırma oranını belirler | muhafazakâr seçim + README'de açık yazım |
| seed → iş yükü eşlemesi (PRNG) | 4 | eski raporların replay'i | kendi PRNG'miz, `schema_version` |
| JSON rapor şeması | 6 | `failures/` dosyaları | `schema_version` alanı |

**Açık riskler:** (1) crash-state sayısı patlaması → örnekleme sınırı; (2) muhafazakâr FS modeli gerçek
motorda yanlış alarm; (3) gerçek motorun arka plan iş parçacıkları tek-iş-parçacığı varsayımını bozabilir
(strace `-f` ile ölçülecek; varsa kapsam dışı ya da senkron mod); (4) LD_PRELOAD'u atlayan motorlar.
