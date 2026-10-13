# Aşama 3 — Çökme durumu üreteci (`cc_trace::crash`) — Teknik Sözleşme

> Bağlam: `CLAUDE.md` karar 2 ("çökme durumları izden üretilir"), `SPEC_stage2_trace_toy.md` §0 ve §7.2.
> Public imzalar ve FS semantiği bağlayıcıdır. Dış bağımlılık yok.

## 0. Dosya sistemi modeli (muhafazakâr POSIX — bağlayıcı)

İz `events[0..cut)` sırayla oynatılır. Her an iki katman vardır: **dayanıklı** (çökmeden sağ çıkması
garanti) ve **bekleyen** (çökmede kalıcı olmuş da olabilir, olmamış da).

| Olay | Etki |
|------|------|
| `Open{created: true}` | yeni inode (dayanıklı içerik = boş); bekleyen **ad** işlemi `Link(name → id)` |
| `Open{created: false}` | durum değişmez (ad uçucu dizinde olmalı) |
| `Write` / `SetLen` | o inode'a bekleyen **veri** işlemi |
| `Fsync` / `Fdatasync` | o inode'un tüm bekleyen veri işlemleri sırayla dayanıklı içeriğe uygulanır |
| `Rename{from,to}` | bekleyen ad işlemi `Rename(from, to, id)` — iki yarısı tek birim (rename atomiktir) |
| `Unlink{name}` | bekleyen ad işlemi `Unlink(name)` |
| `SyncDir` | tüm bekleyen ad işlemleri sırayla dayanıklı dizine uygulanır |

Kararlar ve gerekçeleri:
1. **fsync ≡ fdatasync** (içerik + boyut). Boyut, veriyi okumak için gereken meta veridir (fdatasync(2)).
2. **Dosya fsync'i ad girdisini kalıcı kılmaz;** yalnız `SyncDir` kılar. ext4 yeni dosyada bunu fiilen
   yapar ama POSIX garanti etmez → muhafazakâr seçim. Gerçek motorda yanlış alarm üretirse Aşama 7'de
   ölçülüp not düşülür.
3. **Bekleyen işlemlerin herhangi bir alt kümesi kalıcı olabilir** (inode'lar arası ve ad/veri arası sıra
   garantisi yok — `data=ordered` varsayılmaz). Seçilen alt küme **program sırasıyla** uygulanır: sayfa
   önbelleği her baytın en son değerini tutar, geri yazma daha eski bir değeri yenisinin üstüne yazmaz.
4. Kalıcı olmayan bir yazmanın geride bıraktığı delik **sıfır** okunur (sonraki bir yazma dosyayı
   uzattıysa).
5. **Yırtık yazma (opsiyonel):** bekleyen bir `Write`, mutlak ofsete hizalı sektör (varsayılan 512 B)
   parçalarına bölünür; parçaların bir alt kümesi kalıcı olabilir. Sektör yazımı atomik kabul edilir.
6. İz boş dizinde başlar (Aşama 2 karar 5); bilinmeyen `FileId`/ad → `CrashError`.

## 1. Public API (`cc_trace::crash`, crate kökünden re-export)

```rust
pub type Image = BTreeMap<String, Vec<u8>>;          // ad → içerik: materyalize edilecek dizin

pub struct CrashConfig {                              // Default: 8, 8, false, 512
    pub exhaustive_limit: u32,   // bekleyen işlem sayısı ≤ bu ise TÜM alt kümeler
    pub random_samples: u32,     // aksi halde ek seed'li rastgele alt küme sayısı
    pub torn_writes: bool,
    pub sector_size: u64,
}
pub struct TornWrite { pub event: usize, pub pieces: Vec<usize> }   // kalıcı sektör parçaları
pub struct Choice { pub persisted: Vec<usize>, pub torn: Option<TornWrite> }  // olay indeksleri, artan
pub struct CrashState { pub cut: usize, pub choice: Choice, pub image: Image }
pub enum CrashError { CutOutOfRange{cut,len}, UnknownFile{event,file}, UnknownName{event,name},
                      NotPending{event}, BadTorn{event} }

pub fn pending_ops(events: &[TraceEvent], cut: usize) -> Result<Vec<usize>, CrashError>;
pub fn build_image(events: &[TraceEvent], cut: usize, choice: &Choice) -> Result<Image, CrashError>; // sektör 512
pub fn build_image_with_sector(events, cut, choice, sector_size: u64) -> Result<Image, CrashError>;
pub fn crash_states(events: &[TraceEvent], cut: usize, seed: u64, cfg: &CrashConfig)
    -> Result<Vec<CrashState>, CrashError>;
pub fn materialize(image: &Image, dir: &Path) -> io::Result<()>;   // dir boş olmalı
```
`(events, cut, choice)` bir durumu **tek başına** belirler → Aşama 6 `replay` bunu saklar.

## 2. Durum listesi (`crash_states`) — deterministik

`P` = bekleyen işlemlerin olay indeksleri (artan), `n = |P|`. Seçimler bu sırayla üretilir:
1. `n ≤ exhaustive_limit` (ve n < 32) → maske `0..2^n` artan (bit i ↔ `P[i]`).
2. Aksi → (a) önekler: ilk i işlem kalıcı, `i = 0..=n`; (b) biri hariç hepsi, `j = 0..n`;
   (c) `random_samples` alt küme: SplitMix64(seed ⊕ cut·φ), her bit ½.
3. `torn_writes` → `P`'deki her ≥2 parçalı `Write` için o yazma yırtık: (i) yalnız ilk parça,
   (ii) ilk parça hariç hepsi; her biri iki bağlamda: (a) ondan önceki bekleyenler kalıcı, sonrakiler
   düşmüş; (b) o hariç TÜM bekleyenler kalıcı — yırtık kayıt dosyanın ortasında kalır, yalnız checksum
   reddedebilir (M6 "kısmi kaydı geçerli say" ancak (b) ile yakalanır — ölçüldü).
Görüntüler eşitse ilk geleni tutulur (tekrarsız). Üst sınır: tam modda 2^limit; aksi `2n+1+samples+4w`.
Her listede "hiçbiri kalıcı değil" (= dayanıklı durum) ve "hepsi kalıcı" (= çökmesiz durum) bulunur.

## 3. Testler

`crates/cc-trace/tests/crash.rs` — elle yazılmış izler, beklenen görüntü kümeleri birebir:
C01 boş iz · C02 oluştur+yaz, SyncDir yok (3 görüntü) · C03 fsync sonrası bekleyen ekleme ·
C04 çakışan bekleyen yazmalar (program sırası) · C05 delik = sıfır · C06 rename ile atomik değiştirme,
dizin fsync'i yok · C07 aynısı SyncDir ile (tek görüntü) · C08 rename öncesi fsync yok → boş CHECKPOINT
görüntüsü mümkün · C09 bekleyen unlink · C10 SetLen + sonraki yazma · C11 fdatasync ≡ fsync ·
C12 kesme sınırı ve `CutOutOfRange` · C13 tam olmayan mod: aileler, sınır, uç durumlar, determinizm ·
C14 yırtık yazma parçaları · C15 hatalı iz/seçim hataları · C16 materialize.

`engines/toy-correct/tests/crash.rs` — **duman testi (Aşama 4 öncesi):** seed'li koşuların HER kesme
noktasındaki HER çökme durumu materyalize edilir, `ToyDb` açılır, `check_recovered(crash_view, …)`
`Allowed` olmalı (yanlış alarm = 0); açılış hatası da yanlış alarmdır. Yırtık yazmalı geçiş dahil.

## 4. Bitti ölçütü
fmt / clippy `-D warnings` / tüm testler; dış crate yok; README'ye gidecek "muhafazakâr FS modeli"
varsayımları bu dosyanın §0'ında.
