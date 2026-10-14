# Aşama 7 — Gerçek motor adaptörü: redb + `cc-shim` (LD_PRELOAD)

> CLAUDE.md: "önce `strace -f -e trace=file,desc` ile motorun hangi çağrıları yaptığını ölç; sonra
> `cc-shim` ile kaydet. mmap/io_uring kullanıyorsa kapsam dışı notu düş."

## 0. Ölçüm (tahmin değil) — redb 4.3.0, `redb-driver`
- `strace -f -e trace=file,desc`: veri dosyasına `openat(O_RDWR|O_CREAT)`, `flock`, `fcntl(F_OFD_*)`,
  `statx`, `ftruncate` (oluşturmada 1.056.768 B ön tahsis, kapanışta küçültme), `pwrite64` (320 B başlık
  @0 + 4 KiB sayfalar), `pread64`, `fdatasync` (commit başına; **hiç `fsync` yok**), `close`.
  **Dizin fsync'i hiç yok.**
- `strace -e trace=memory,process`: veri dosyası açıldıktan sonra dosya destekli `mmap` **0**, `clone`
  **0** → kapsam içinde (mmap yok, tek iş parçacığı).
- `nm -D --undefined-only redb-driver`: `open64, pwrite64, fdatasync, ftruncate64, close, dup, fcntl,
  flock, statx, mmap64, write, writev, syscall`. Shim'in gölgelediği küme buradan.

## 1. Tasarım
1. **Ayrı süreç:** `redb-driver` (redb + satır başına JSON protokolü). LD_PRELOAD bütün süreci
   etkiler; runner'ın G/Ç'si ize karışmamalı. Her çökme durumundan açılış yeni bir süreçtir.
2. **`cc-shim` (cdylib):** `open/open64/openat/openat64/creat/creat64`, `pwrite/pwrite64`, `ftruncate/
   ftruncate64`, `fsync/fdatasync` (dizin tanıtıcısında `SyncDir`), `close`, `rename`, `unlink`
   kaydedilir; `write/writev` (izlenen dosyada), `dup*`, `mmap*`, `fallocate*`, `sync_file_range`,
   `sync/syncfs`, `truncate*`, `renameat/unlinkat` → `unsupported` satırı → adaptör **panik** (eksik iz
   ile devam etmek sessiz yanlış sonuç üretirdi). Satır formatı `cc_trace::shim_format`.
3. **`unsafe` yalnız `cc-shim`'de** (crate seviyesinde gerekçeli `allow`). `open*` C'de değişken
   argümanlı: sabit argümanlı tanım x86_64 SysV ABI'de doğru → crate yalnız `linux`+`x86_64`.
4. **Span hizası:** sürücü yanıtı işlem bittikten sonra yazar, shim her olayı çağrı dönmeden yazar →
   adaptör yanıttan sonra iz dosyasını okuyunca o işlemin tüm olayları span içindedir.
5. **İki FS modeli** (`CrashConfig::fsync_persists_new_dirent`): `posix` (varsayılan; ad yalnız dizin
   fsync'iyle kalıcı) ve `linux` (yeni dosyanın fsync/fdatasync'i adını da sabitler — ext4/XFS/btrfs'in
   bildirilen davranışı, bu projede **doğrulanmadı**).
6. **`RunConfig::skip_open_phase`:** açılış fazındaki kesmeleri atlar. Gerekçe: redb'nin oluşturma
   hatası (§2) her seed'de ilk kesmelerde çıkar; runner seed başına ilk bulguda durduğu için işlem fazı
   hiç sınanamazdı. Hata ayrı raporlanır, işlem fazı ayrıca ölçülür.

## 2. Bulgular
- **(Gerçek hata, redb 4.3.0) Oluşturma sırasında çökme dosyayı kalıcı olarak açılamaz bırakır.**
  `Database::create` önce `ftruncate` ile ~1 MiB ön tahsis eder, sonra magic numarası SIFIR başlığı
  yazıp fdatasync'ler, en son magic'li başlığı yazar ("crash safe" yorumuyla). Ama açılıştaki kontrol
  (`page_manager.rs`) boyu > 0 olup magic'i olmayan dosyayı "Not a redb database: magic number
  mismatch" diye reddeder. Araç bunu 0 işlemlik karşı örnekle buldu (`failures/redb-create-crash-
  seed0.json`); **model dışı doğrulama:** `strace -e inject=pwrite64:signal=SIGKILL:when=1` (ve `=2`)
  ile gerçek süreç öldürme → yeniden açılış aynı hatayı verir; `when=3` (magic yazıldıktan sonra) →
  açılır. Güç kaybı gerekmez; `kill -9` / OOM yeter. Yukarı akışta bilinip bilinmediği kontrol edilmedi.
- **(Katı POSIX'te boşluk) Dizin fsync'i yok:** veritabanı dosyası `O_CREAT` ile yaratılır, dizin hiç
  fsync'lenmez. POSIX'e göre ilk ACK'li commit'ten sonra bile dosyanın adı kaybolabilir
  (`failures/redb-posix-dirent-seed0.json`, 1 işlem). Linux dosya sistemlerinde gözlenip
  gözlenmeyeceği doğrulanmadı (linux modeli bu boşluğu kabul eder).
