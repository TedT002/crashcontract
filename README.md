# CrashContract

Depolama motorlarının **çökme sonrası kurtarma doğruluğunu** test eden bir araç (Rust).

Bir motora seed'li bir iş yükü uygular, dosya sistemi çağrılarını **kaydeder**, bu kayıttan **izinli
çökme durumları** üretir, motoru her durumdan yeniden açar ve sonucu bir **referans modelle**
karşılaştırır. Hata bulunursa karşı örnek otomatik küçültülür ve tek komutla yeniden oynatılabilen
bir JSON rapor yazılır.

> Bu README'deki her sayı `docs/MEASUREMENTS.md`'deki gerçekten koşulmuş bir komuttan gelir.
> Doğrulanmamış her şey aşağıdaki **Doğrulanmayanlar** bölümündedir.

## Nasıl çalışır

1. **Kaydet, öldürme.** Süreci dışarıdan `kill` etmek deterministik değildir ve sayfa önbelleği
   yazıları koruduğu için fsync hatalarını yakalamaz. Bunun yerine motorun `write/pwrite`, `fsync`,
   `fdatasync`, `rename`, `ftruncate`, `unlink`, dizin fsync'i çağrıları bir **iz** olarak kaydedilir:
   toy motorlarda `Vfs` katmanıyla, gerçek motorlarda `LD_PRELOAD` shim'iyle (`cc-shim`).
2. **Çökme durumları izden üretilir.** "İzin N. olayında kes" + "fsync ile sabitlenmemiş yazıların ve
   dizin fsync'i ile sabitlenmemiş ad değişikliklerinin herhangi bir alt kümesi kalıcı olmuş olabilir"
   (+ isteğe bağlı 512 B sektörde yırtık yazma). Aynı iz + seed → aynı durumlar.
3. **Model bir küme döndürür, tek durum değil.** Çökme anında yarıda kalan işlem uygulanmış da
   olabilir, uygulanmamış da; motor yalnız **bildirdiği** garantilerle (dayanıklılık modu, txn
   atomikliği, sıralama) yargılanır. Prefix modunda üyelik testi tamdır (yanlış alarm da kaçırma da
   yok); izinli durumlar tek tek listelenmez, artımlı bir uyuşmazlık sayacıyla tek geçişte bakılır.
4. **Araç kendini kanıtlar.** Bilerek hatalı 7 mutant motor yazıldı; araç hepsini yakalıyor ve CI her
   commit'te bunu yeniden doğruluyor.

## Hızlı başlangıç

```sh
cargo build --release --workspace          # crashcontract, redb-driver, libcc_shim.so
B=target/release/crashcontract

$B run --seeds 0..1000                     # toy-correct: her kesme, her çökme durumu
$B run --engine toy-mutant:ignore-tombstones --seeds 0..20   # bulgu → küçült → failures/*.json
$B replay failures/toy-mutant-ignore-tombstones-seed0.json   # bire bir aynıysa çıkış 0
$B mutants                                 # her mutantın yakalanma oranı
$B run --engine redb --fs-model linux --skip-open-phase --cuts 40 --seeds 0..50
$B minimize --seed 0 --engine redb --cuts all               # redb oluşturma hatası
```

Çıkış kodları: `0` bulgu yok, `1` bulgu var (rapor yazıldı), `2` kullanım/altyapı hatası.

## Yakalanan hata sınıfları (mutantlar)

Her mutant `toy-correct`'in tek bir dayanıklılık kararını bozar (`toy_correct::Mutation`; ayrı kod
kopyası yok). Hatanın var olabildiği yapılandırmada, seed 0..40, tüm kesmeler
(`cargo test -p toy-mutants`). Aynı yapılandırmalar mutasyonsuz: **0 bulgu**.

| Mutant | Hata sınıfı | Yakalanan seed | Bulgu | Küçültülmüş karşı örnek |
|--------|-------------|----------------|-------|-------------------------|
| `skip-commit-fsync` | fsync'i atla | 40/40 | LostDurableWrite | `put a=""` (55 → 1 işlem) |
| `unlink-before-dir-sync` | fsync sırasını değiştir | 40/40 | RecoveryFailed | 3 işlem (= 1 checkpoint) |
| `drop-last-record` | WAL replay'de son kaydı at | 40/40 | LostDurableWrite | `put a=""` (55 → 1) |
| `ignore-tombstones` | tombstone'u kurtarmada yoksay | 40/40 | ResurrectedDelete, PartialTxn | `put a; delete a` (55 → 2) |
| `no-fsync-before-rename` | rename'den önce fsync yapma | 40/40 | RecoveryFailed | 0 işlem (açılış checkpoint'i) |
| `accept-corrupt-record` | kısmi kaydı geçerli say | 40/40 | RecoveryFailed, PhantomValue, PrefixViolation | 55 → 14 |
| `split-txn-records` | (ek) txn atomikliğini boz | 39/40 | PartialTxn (1× LostDurableWrite) | 1 txn |

**Küçültme:** 7 mutant toplam 385 → 22 işlem (**%94,3**). Her rapor `crashcontract replay` ile
bire bir yeniden üretiliyor; `failures/` altındaki 9 raporun hepsi CI'da replay edilir.

## Koşulan senaryolar ve yanlış alarm

| Motor | Komut | Çökme durumu | Bulgu (= yanlış alarm) |
|-------|-------|--------------|------------------------|
| toy-correct | `run --seeds 0..10000 --cuts all` | 14.002.928 | **0** (305,7 sn) |
| toy-correct | `... --torn` (512 B yırtık yazma) | 14.319.734 | **0** |
| redb 4.3.0 | `run --engine redb --fs-model linux --skip-open-phase --cuts 40 --seeds 0..200` | 130.938 | **0** |
| redb 4.3.0 | aynısı `--torn --cuts 20 --seeds 0..40` | 13.024 | **0** |

redb satırları açılış fazını atlar (oluşturma hatası ayrıca raporlandı, aşağıda) ve Linux FS modelini
kullanır; katı POSIX modelinde 40/40 seed'de dizin fsync'i eksikliği bulunur (aşağıda). Bu koşularda
toplam **28.466.624 çökme durumu** denendi; `toy-correct`'te ve redb'nin işlem fazında (Linux modeli)
yanlış alarm **0**.

## Gerçek motor: redb 4.3.0

Önce ölçüldü (`strace -f -e trace=file,desc`, `nm -D`): veri dosyası **mmap edilmiyor**, ek iş
parçacığı yok → kapsam içinde. G/Ç: `pwrite64` (320 B başlık + 4 KiB sayfalar), commit başına
`fdatasync`, `ftruncate` ön tahsis; **dizin fsync'i hiç yok**. Shim'in kaydı strace ile karşılaştırıldı:
300 işlemlik iş yükünde 2.148 olay, tür/ofset/uzunluk/sıra **birebir aynı**.

Bulgular:

1. **Hata — oluşturma sırasında çökme dosyayı kalıcı olarak açılamaz bırakır.** `Database::create`
   ~1 MiB ön tahsisten sonra magic'siz başlığı, en son magic'li başlığı yazar ("crash safe" amacıyla);
   ama açılış, boyu > 0 olup magic'i olmayan dosyayı `Not a redb database: magic number mismatch` ile
   reddeder. Araç 0 işlemlik karşı örnek üretti (`failures/redb-create-crash-seed0.json`).
   **Araçtan bağımsız doğrulama:** `strace -e inject=pwrite64:signal=SIGKILL:when=1` (ve `when=2`) ile
   gerçek `SIGKILL` → yeniden açılış aynı hatayı verir; `when=3` (magic yazıldıktan sonra) → açılır.
   Güç kaybı gerekmez, `kill -9` yeter. Yukarı akışta bilinip bilinmediği kontrol edilmedi.
2. **Katı POSIX'te dayanıklılık boşluğu — dizin fsync'i yok.** Dosya `O_CREAT` ile yaratılıp dizin hiç
   fsync'lenmediğinden, POSIX'e göre ilk ACK'li commit'ten sonra bile dosyanın adı kaybolabilir
   (`failures/redb-posix-dirent-seed0.json`, 1 işlem). Linux dosya sistemlerinde gözlenip gözlenmediği
   doğrulanmadı; `--fs-model linux` bu boşluğu varsayımla kabul eder.

## Mimari

```
crates/
  cc-model/     referans model: geçmiş, garantiler, izinli küme, ihlal sınıflandırması
  cc-trace/     iz formatı, Vfs/DirVfs, çökme durumu üreteci, shim satır formatı
  cc-workload/  SplitMix64 + seed'li Put/Get/Delete/Txn/Sync üreteci
  cc-adapter/   StorageAdapter trait'i, apply_op, runner
  cc-minimize/  ddmin + txn içi ddmin + anahtar birleştirme + değer kısaltma
  cc-shim/      (cdylib, unsafe) LD_PRELOAD ile libc G/Ç kaydı
  cc-cli/       crashcontract: run / minimize / replay / mutants, JSON rapor
engines/
  toy-correct/  WAL + checkpoint'li doğru motor (Vfs üzerinden)
  toy-mutants/  mutant kataloğu + yakalama testleri
  redb-engine/  redb sürücü süreci + adaptör
failures/       replay edilebilir JSON raporlar
docs/           PLAN, aşama SPEC'leri, MEASUREMENTS
```

Dış bağımlılıklar: `serde`/`serde_json` (rapor), `libc` (shim), `redb` (test edilen motor) ve
bunların geçişli bağımlılıkları. Her biri ilgili `Cargo.toml`'da gerekçeli.

## Kapsam dışı (v1)

Gerçek güç kaybı ve blok seviyesi hatalar (bit bozulması, yanlış adrese yazma, sürücü önbelleği),
eşzamanlı iş yükleri / linearizability, mmap ya da io_uring kullanan motorlar, fsync hata semantiği
("fsyncgate"), Linux dışı platformlar (shim yalnız `linux` + `x86_64`).

## Doğrulanmayanlar

- **Dosya sistemi modeli bir varsayımdır.** Varsayılan model katı POSIX'tir (ad yalnız dizin
  fsync'iyle kalıcı, bekleyen yazmaların her alt kümesi mümkün, sıra garantisi yok). `--fs-model
  linux`'un "yeni dosyanın fsync'i adını da sabitler" varsayımı ext4/XFS/btrfs için **test edilmedi**.
  Gerçek bir güç kaybı düzeneğiyle (dm-log-writes, dm-flakey vb.) hiçbir şey doğrulanmadı.
- **Toy motorlar gerçek diskte dayanıklı değildir:** `DirVfs` gerçek `fsync` çağırmaz; doğruluk iddiası
  yalnız izden üretilen çökme durumları içindir.
- **Sektör atomikliği (512 B) varsayılır;** yırtık yazma yalnız bekleyen yazmalarda ve sektör
  sınırlarında modellenir.
- **Bulgu yokluğu kanıt değildir.** 8'den fazla bekleyen işlemde tüm alt kümeler değil, yapısal aileler
  (önekler, biri-hariç) + seed'li örnekler denenir; redb ölçümünde seed başına 40 kesme örneklenir.
- **Shim yalnız dinamik libc sembol çağrılarını görür.** glibc'nin iç çağrıları (`fopen` → `__open`),
  doğrudan `syscall(2)`, io_uring, `pwritev`, `copy_file_range`, `sendfile`, `fcntl(F_DUPFD)` görünmez.
  Bu yüzden her motor için shim izi strace ile karşılaştırılır (redb: tek bir 300 işlemlik iş yükünde
  birebir); başka iş yüklerinde/sürümlerde aynı olduğu doğrulanmadı.
- **Anahtar-bazlı modda** (motor sıralama garantisi bildirmezse) atomik txn kontrolü sağlamdır ama tam
  değildir: bazı yarım txn'ler kaçabilir.
- **İhlal etiketi** birden çok geçerli açıklamadan birini seçer (ör. yarım txn'in ara değeri bir silme
  ise `LostDurableWrite`); hüküm (ihlal var/yok) etkilenmez.
- redb için yalnız `Durability::Immediate` (varsayılan) ve tek tablo denendi; diğer modlar ve
  yapılandırmalar test edilmedi.

## Geliştirme

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --locked     # cargo test cdylib'i (.so) derlemez — redb testleri için gerekli
cargo test --workspace --locked      # mutant yakalama + failures/ replay dahil
```

Tasarım ve aşama sözleşmeleri: `docs/PLAN_crashcontract.md`, `docs/SPEC_stage*.md`;
ölçümler: `docs/MEASUREMENTS.md`.
