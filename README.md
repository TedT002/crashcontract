# CrashContract

Bir depolama motoru "commit tamam" dediğinde, makine o anda çökse veri gerçekten diskte kalıyor mu?
CrashContract bunu test etmek için yazdığım küçük bir araç.

Fikir basit: motora rastgele (ama seed'li) bir iş yükü uygula, yaptığı dosya sistemi çağrılarını
kaydet, bu kayıttan "makine şu anda çökseydi diskte ne kalmış olabilirdi" durumlarını üret, motoru her
birinden yeniden aç ve kurtardığı veriyi bir referans modelle karşılaştır. Bir uyuşmazlık çıkarsa
karşı örneği küçültüp tek komutla yeniden oynatılabilen bir JSON rapor yazıyor.

## Neden süreci öldürmüyoruz?

Çökme testi denince akla ilk gelen, süreci rastgele anlarda `kill -9` ile öldürmek. Bunun iki sorunu
var: deterministik değil, ve daha önemlisi süreç ölünce işletim sisteminin sayfa önbelleği hayatta
kalıyor. Yani fsync'i hiç çağırmayan bir motor bu testten tertemiz geçer.

Onun yerine motorun `pwrite`, `fsync`, `fdatasync`, `rename`, `ftruncate`, `unlink` çağrılarını bir iz
olarak kaydediyorum. Çökme durumlarını bu izden üretiyorum: izi bir noktada kes, sonra fsync ile
sabitlenmemiş her yazının (ve dizin fsync'i görmemiş her ad değişikliğinin) kalıcı olmuş da olmamış da
olabileceğini varsay. İstenirse yazılar 512 baytlık sektör sınırlarında yırtılabilir. Aynı iz ve aynı
seed her zaman aynı durumları verir.

Kayıt iki yolla yapılıyor: kendi yazdığım toy motorlar dosya sistemine küçük bir `Vfs` katmanından
erişiyor; gerçek motorlar için (şimdilik redb) `LD_PRELOAD` ile yüklenen bir shim libc çağrılarını
yakalıyor.

## Model neden tek bir "doğru sonuç" bekleyemez?

Çökme anında yarıda kalan bir işlem uygulanmış da olabilir, uygulanmamış da. Motor ACK vermiş ama
veriyi henüz sabitlememiş olabilir. Bu yüzden model tek bir beklenen durum değil, izinli durumlar
kümesi üretiyor ve motoru yalnızca kendi bildirdiği garantilere göre yargılıyor: her commit'te mi
dayanıklı, yoksa yalnız açık bir `sync` sonrasında mı; txn'ler atomik mi; kurtarılan durum her zaman
geçmişin bir öneki mi. Vaat etmediği bir şeyi kaybetmek ihlal değil, ama hiç yazılmamış bir değeri
geri getirmek her zaman ihlal.

Küme açıkça listelenmiyor (üstel olurdu); kurtarılan durum, izinli kesme noktaları boyunca artımlı bir
uyuşmazlık sayacıyla tek geçişte kontrol ediliyor.

## Kullanım

```sh
cargo build --release --workspace
B=target/release/crashcontract

$B run --seeds 0..1000                          # doğru toy motor: her kesme noktası
$B run --engine toy-mutant:ignore-tombstones    # bulgu → küçült → failures/*.json
$B replay failures/toy-mutant-ignore-tombstones-seed0.json
$B mutants                                      # tüm mutantların yakalanma oranı
$B run --engine redb --fs-model linux --skip-open-phase --cuts 40 --seeds 0..50
$B minimize --seed 0 --engine redb --cuts all   # redb'de bulunan oluşturma hatası
```

Çıkış kodları: 0 bulgu yok, 1 bulgu var (rapor yazıldı), 2 kullanım ya da altyapı hatası.
`replay` sonuç rapordakiyle bire bir aynıysa 0 döner.

## Araç gerçekten hata yakalıyor mu?

Bunu görmek için doğru çalışan toy motorun bilerek bozulmuş sürümlerini yazdım. Her biri tek bir
dayanıklılık kararını bozuyor. Hatanın ortaya çıkabildiği yapılandırmada, 40 seed ve tüm kesme
noktalarıyla:

| Mutant | Ne bozuluyor | Yakalanan | Bulgu | Küçültülmüş örnek |
|--------|--------------|-----------|-------|-------------------|
| `skip-commit-fsync` | commit'te fsync atlanıyor | 40/40 | LostDurableWrite | `put a=""` |
| `unlink-before-dir-sync` | eski WAL, yeni checkpoint kalıcı olmadan siliniyor | 40/40 | RecoveryFailed | 3 işlem |
| `drop-last-record` | replay son kaydı atıyor | 40/40 | LostDurableWrite | `put a=""` |
| `ignore-tombstones` | replay silmeleri yoksayıyor | 40/40 | ResurrectedDelete | `put a; delete a` |
| `no-fsync-before-rename` | checkpoint rename'den önce fsync'lenmiyor | 40/40 | RecoveryFailed | 0 işlem |
| `accept-corrupt-record` | replay CRC'ye bakmıyor | 40/40 | RecoveryFailed / PhantomValue | 14 işlem |
| `split-txn-records` | txn tek kayıt yerine parça parça yazılıyor | 39/40 | PartialTxn | 1 txn |

Aynı yapılandırmalar mutasyon olmadan çalıştırıldığında hiç bulgu çıkmıyor. Küçültücü (ddmin, ardından
anahtar birleştirme ve değer kısaltma) bu yedi örnekte toplam 385 işlemi 22'ye indirdi. `failures/`
altındaki raporların hepsi CI'da her commit'te yeniden oynatılıp bire bir aynı sonucu verdiği kontrol
ediliyor.

## Ölçümler

Hepsi gerçekten koşulmuş komutlar; makine i7-13650HX, 20 iş parçacığı, ext4.

| Motor | Komut | Çökme durumu | Bulgu |
|-------|-------|--------------|-------|
| toy-correct | `run --seeds 0..10000 --cuts all` | 14.002.928 | 0 (305 sn) |
| toy-correct | aynısı `--torn` ile | 14.319.734 | 0 |
| redb 4.3.0 | `run --engine redb --fs-model linux --skip-open-phase --cuts 40 --seeds 0..200` | 130.938 | 0 |
| redb 4.3.0 | aynısı `--torn --cuts 20 --seeds 0..40` | 13.024 | 0 |

redb'de her çökme durumu yeni bir süreç ve ~1 MiB'lık bir dosya demek, o yüzden toy motordan yaklaşık
250 kat yavaş; kesme noktalarını örnekleyerek çalıştırıyorum.

## redb'de bulunanlar

redb'ye dokunmadan önce `strace` ile ne yaptığına baktım: veri dosyasını mmap etmiyor, ek iş parçacığı
açmıyor; `pwrite64` ile yazıyor, her commit'te bir `fdatasync` yapıyor ve dosyayı oluştururken
`ftruncate` ile yer ayırıyor. Dizini hiç fsync'lemiyor. Shim'in kaydettiği iz, 300 işlemlik bir iş
yükünde strace ile olay olay aynı çıktı (2.148 olay).

1. **Oluşturma sırasında çökme dosyayı kalıcı olarak açılamaz bırakıyor.** `Database::create` önce
   dosyayı ~1 MiB'a büyütüyor, sonra magic numarası boş bir başlık yazıp fsync'liyor, en son magic'i
   yazıyor. Koddaki yoruma göre amaç bunu çökmeye dayanıklı yapmak; ama açılıştaki kontrol, boyu sıfırdan
   büyük olup magic'i olmayan dosyayı "Not a redb database" diye reddediyor. Araç bunu hiç işlem
   gerektirmeyen bir karşı örnekle buldu (`failures/redb-create-crash-seed0.json`). Araçtan bağımsız
   olarak da doğruladım: `strace -e inject=pwrite64:signal=SIGKILL:when=1` ile süreci gerçekten
   öldürünce dosya bir daha açılmıyor; magic yazıldıktan sonra öldürünce sorun yok. Güç kesintisi
   gerekmiyor, sıradan bir `kill -9` yetiyor. Yukarı akışta bilinen bir sorun olup olmadığına
   bakmadım.

2. **Katı POSIX açısından: dizin hiç fsync'lenmiyor.** Dosya `O_CREAT` ile yaratılıyor ama dizini
   fsync'lenmiyor; POSIX'e göre bu, ilk başarılı commit'ten sonra bile dosyanın adının kaybolabileceği
   anlamına geliyor (`failures/redb-posix-dirent-seed0.json`). Linux dosya sistemlerinin pratikte bunu
   tolere edip etmediğini test etmedim; `--fs-model linux` bu konuda iyimser varsayımı kullanıyor.

## Yapı

```
crates/
  cc-model/     referans model ve hüküm
  cc-trace/     iz formatı, Vfs, çökme durumu üreteci
  cc-workload/  seed'li iş yükü üreteci
  cc-adapter/   motor arayüzü (StorageAdapter) ve runner
  cc-minimize/  karşı örnek küçültme
  cc-shim/      LD_PRELOAD shim'i (projedeki tek unsafe kod)
  cc-cli/       crashcontract komutu ve JSON raporlar
engines/
  toy-correct/  WAL + checkpoint'li küçük, doğru bir KV motoru
  toy-mutants/  bozulmuş sürümlerin kataloğu ve testleri
  redb-engine/  redb sürücüsü ve adaptörü
failures/       yeniden oynatılabilir raporlar
```

Dış bağımlılıklar az tutuldu: rapor için `serde`/`serde_json`, shim için `libc` ve test edilen motor
olarak `redb`.

## Sınırlar ve doğrulamadığım şeyler

Kapsam dışı: gerçek güç kaybı ve blok seviyesi hatalar, eşzamanlı iş yükleri, mmap ya da io_uring
kullanan motorlar, fsync'in hata döndürdüğü durumlar. Shim yalnızca Linux x86_64'te çalışıyor.

Bilinçli olarak varsayıp test etmediklerim:

- Dosya sistemi modeli bir varsayım. Varsayılan model katı POSIX; `--fs-model linux` seçeneğindeki
  "yeni bir dosyanın fsync'i adını da kalıcı yapar" varsayımını ext4/XFS üzerinde gerçek bir güç kaybı
  düzeneğiyle denemedim.
- Toy motorlar gerçek diskte dayanıklı değil; kayıt katmanı gerçek fsync çağırmıyor. Doğrulukları
  yalnızca izden üretilen çökme durumları için geçerli.
- 512 baytlık sektör yazımının atomik olduğu varsayılıyor.
- Bulgu çıkmaması kanıt değil. Sekizden fazla bekleyen işlem olduğunda bütün alt kümeler değil,
  yapılandırılmış aileler ve rastgele örnekler deneniyor; redb'de kesme noktaları örnekleniyor.
- Shim yalnız dinamik libc çağrılarını görüyor. glibc'nin kendi içinden yaptığı çağrılar (`fopen`),
  doğrudan `syscall`, io_uring, `pwritev`, `copy_file_range` görünmüyor. Bu yüzden her motorda önce
  strace ile karşılaştırma yapıyorum; redb için bu tek bir iş yükünde yapıldı.
- Motor sıralama garantisi vermiyorsa, atomik txn kontrolü yanlış alarm üretmiyor ama bazı yarım
  txn'leri kaçırabilir.
- redb'de yalnızca varsayılan dayanıklılık modu ve tek bir tablo denendi.

## Geliştirme

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --locked   # cargo test shim'in .so dosyasını derlemiyor; redb testleri için gerekli
cargo test --workspace --locked
```
