# Ölçümler

README'deki sayılar buradan gelir. Her satır gerçekten koşulmuş bir komutun çıktısıdır; tahmin yoktur.

**Makine:** Intel Core i7-13650HX (20 iş parçacığı), 31 GiB RAM, Linux 6.8.0, ext4 (NVMe).
Çalışma dizinleri `/tmp` altında; `DirVfs` gerçek `fsync` çağırmaz (bkz. Doğrulanmayanlar).

## Aşama 4 — toy-correct, yanlış alarm ölçümü (2026-10-13, commit: Aşama 4)

İş yükü: `WorkloadConfig::default()` (20–119 işlem; 8 anahtar; Put %40, Delete %15, Get %25,
Txn %15, Sync %5). Yapılandırma `mixed`: politika seed%2 (EveryCommit / OnSyncOnly), checkpoint
(seed/2)%2 ile 0 ya da 7. Her izin **her** kesme noktası (`--cuts all`).

| Komut | Seed | İşlem | Olay | Kesme | Çökme durumu | Bulgu | Süre |
|-------|------|-------|------|-------|--------------|-------|------|
| `crashcontract run --seeds 0..10000 --cuts all` | 10.000 | 696.605 | 1.055.608 | 1.065.608 | 14.002.928 | **0** | 305,7 sn |
| `crashcontract run --seeds 0..10000 --cuts all --torn` | 10.000 | 696.605 | 1.055.608 | 1.065.608 | 14.319.734 | **0** | 366,3 sn¹ |

Yırtık yazma (512 B sektör) 316.806 ek durum getirdi. ¹ Bu koşunun bir kısmı tam test paketiyle eşzamanlı
çalıştı; süre bu yüzden bir miktar şişkindir.

## Aşama 5 — mutantlar (2026-10-13)

Her mutant `toy-correct`'in tek bir dayanıklılık kararını bozar (`toy_correct::Mutation`). İki ölçüm:

**(a) Hatanın ortaya çıkabildiği yapılandırmada** — `cargo test -p toy-mutants` (seed 0..40, tüm
kesmeler; yırtık yazma yalnız `accept-corrupt-record` için). Kontrol grubu (aynı yapılandırma,
mutasyonsuz): **0 bulgu**.

| Mutant | CLAUDE.md sınıfı | Yapılandırma | Yakalanan seed | Bulgu türleri |
|--------|------------------|--------------|----------------|---------------|
| `skip-commit-fsync` | fsync'i atla | EveryCommit | 40/40 | LostDurableWrite 40 |
| `unlink-before-dir-sync` | fsync sırasını değiştir | EveryCommit, checkpoint 3 | 40/40 | RecoveryFailed 40 |
| `drop-last-record` | WAL replay'de son kaydı at | EveryCommit | 40/40 | LostDurableWrite 40 |
| `ignore-tombstones` | tombstone'u yoksay | EveryCommit | 40/40 | ResurrectedDelete 28, PartialTxn 12 |
| `no-fsync-before-rename` | rename'den önce fsync yapma | EveryCommit, checkpoint 3 | 40/40 | RecoveryFailed 40 |
| `accept-corrupt-record` | kısmi kaydı geçerli say | OnSyncOnly, yırtık yazma | 40/40 | RecoveryFailed 26, PhantomValue 13, PrefixViolation 1 |
| `split-txn-records` | (ek) txn atomikliğini boz | EveryCommit | 39/40 | PartialTxn 38, LostDurableWrite 1 |

**(b) Karışık yapılandırmada** — `crashcontract mutants` (seed 0..100, `mixed`: politika seed%2,
checkpoint 0/7; tüm kesmeler + yırtık yazma), 12,4 sn:

| Mutant | Yakalanan seed | İlk yakalayan seed | İlk yakalamaya dek durum |
|--------|----------------|--------------------|--------------------------|
| `skip-commit-fsync` | 50/100 | 0 | 16 |
| `unlink-before-dir-sync` | 50/100 | 2 | 2.954 |
| `drop-last-record` | 94/100 | 0 | 18 |
| `ignore-tombstones` | 100/100 | 0 | 30 |
| `no-fsync-before-rename` | 100/100 | 0 | 13 |
| `accept-corrupt-record` | 23/100 | 1 | 1.377 |
| `split-txn-records` | 96/100 | 0 | 26 |

50/100'ler eksik yakalama değil: `skip-commit-fsync` yalnız EveryCommit seed'lerinde (yarısı),
`unlink-before-dir-sync` yalnız checkpoint açık seed'lerde (yarısı) var olabilir; bu seed'lerin hepsinde
yakalandı. `split-txn-records`'taki 1 LostDurableWrite etiketi: yarım txn'in görünen ara değeri bir
silme olduğunda sınıflandırıcı eski-değer açıklamasını önceler (SPEC_stage1 §4.6(a)); hüküm aynıdır.

## Aşama 6 — küçültme (2026-10-13)

`crashcontract minimize --seed 0 --engine toy-mutant:<ad>` (mutantın yapılandırmasıyla; raporlar
`failures/`). Her rapor `crashcontract replay` ile **bire bir** yeniden üretildi (CI testi:
`committed_reports_replay_identically`).

| Mutant | İşlem | Anahtar | Değer baytı | Deneme | Küçültülmüş karşı örnek |
|--------|-------|---------|-------------|--------|-------------------------|
| `skip-commit-fsync` | 55 → 1 | 8 → 1 | 239 → 0 | 11 | `put a=""` |
| `unlink-before-dir-sync` | 55 → 3 | 8 → 1 | 239 → 0 | 36 | `put a; delete a; put a` (3 kayıt = 1 checkpoint) |
| `drop-last-record` | 55 → 1 | 8 → 1 | 239 → 0 | 11 | `put a=""` |
| `ignore-tombstones` | 55 → 2 | 8 → 1 | 239 → 0 | 42 | `put a; delete a` |
| `no-fsync-before-rename` | 55 → 0 | 8 → 0 | 239 → 0 | 2 | (boş — açılıştaki checkpoint yeterli) |
| `accept-corrupt-record` | 55 → 14 | 8 → 1 | 239 → 107 | 323 | sektör sınırını aşan kayıtlar gerekir |
| `split-txn-records` | 55 → 1 | 8 → 1 | 239 → 1 | 20 | `txn[put a=""; put a="\xda"]` |

Toplam: 385 → 22 işlem (**%94,3 küçülme**; `accept-corrupt-record` hariç 330 → 8, %97,6).
`accept-corrupt-record`'da anahtarlar `a`'ya yeniden adlandırılamadı: kısalan kayıtlar sektör
sınırlarını kaydırıyor ve yırtılma artık kaydın ortasına düşmüyor — minimizer sınıf değiştiren adayı
reddetti (doğru davranış).

## Aşama 7 — gerçek motor: redb 4.3.0 (2026-10-14)

**Ölçüm (strace, nm):** veri dosyasına dosya destekli `mmap` 0, `clone` 0; çağrılar `openat(O_CREAT)`,
`ftruncate` (1.056.768 B ön tahsis), `pwrite64` (320 B başlık + 4 KiB sayfa), commit başına `fdatasync`,
`pread64`, `flock`/`fcntl`/`statx`, `close`; `fsync` ve dizin fsync'i **yok**. Ayrıntı:
`docs/SPEC_stage7_real_engine.md`.

**Shim ↔ strace paritesi:** 300 işlemlik rastgele iş yükü (değer ≤ 3 KiB, txn, silme, sync): strace 2.148
veri dosyası olayı, shim 2.148; tür, ofset, uzunluk ve sıra birebir aynı; iki koşunun son veri dosyası
bayt bayt aynı.

| Komut (release, 20 iş parçacığı) | Seed | Kesme | Çökme durumu | Bulgulu seed | Süre |
|----------------------------------|------|-------|--------------|--------------|------|
| `run --engine redb --fs-model linux --skip-open-phase --cuts 40 --seeds 0..200` | 200 | 8.000 | 130.938 | **0** | 738,3 sn¹ |
| `run --engine redb --fs-model linux --skip-open-phase --torn --cuts 20 --seeds 0..40 --jobs 12` | 40 | 800 | 13.024 | **0** | 136,3 sn¹ |
| `run --engine redb --skip-open-phase --cuts 40 --seeds 0..40` (katı POSIX) | 40 | 74 | 258 | 40/40 (LostDurableWrite) | 6,1 sn |

¹ Koşuların bir kısmı test paketiyle eşzamanlıydı; süreler şişkindir. redb'de her çökme durumu yeni bir
süreç + ~1 MiB dosya materyalizasyonu demektir (toy'a göre ~250× yavaş).

**Bulgular:** (1) `Database::create` sırasında çökme → dosya kalıcı olarak açılamaz ("Not a redb
database: magic number mismatch"); araç 0 işlemlik karşı örnek üretti (`failures/redb-create-crash-
seed0.json`, kesme 2: oluşturma + ön tahsis kalıcı, başlık yok). Araçtan bağımsız doğrulama:
`strace -e inject=pwrite64:signal=SIGKILL:when=N` → N=1 ve N=2'de yeniden açılış başarısız, N=3'te
(magic yazıldıktan sonra) başarılı. (2) Katı POSIX'te dizin fsync'i eksikliği: ilk ACK'li commit sonrası
dosyanın adı kaybolabilir (`failures/redb-posix-dirent-seed0.json`, 1 işlem). Linux modelinde (yeni
dosyanın fdatasync'i adı da sabitler — doğrulanmamış varsayım) işlem fazında bulgu yok.

**Derleme profili:** redb'nin `debug_assertions` dalları disk görüntüsünü değiştiriyordu (aynı koşuda
debug/release başlığında 48 bayt fark → replay yalnız aynı profilde bire birdi). `[profile.dev.package.redb]
debug-assertions = false` ile iki profilin raporu eşit (doğrulandı).

## Son regresyon (Aşama 8, 2026-10-14)

Tüm değişikliklerden sonra: `cargo test --workspace` 154/154; `crashcontract mutants` tablosu Aşama 5 ile
aynı; `run --seeds 0..1000 --cuts all` 1.400.625 durum, 0 bulgu (28,7 sn); `failures/` 9 raporun 9'u
bire bir replay.

## Aşama 3 — mutant ön kontrolü (elle enjekte, geri alındı)

`engines/toy-correct/tests/crash.rs` duman testi (18 seed, tüm kesmeler, yırtık geçişi dahil) altı hata
sınıfının her birini yakaladı: M1 → `LostDurableWrite`, M2 → kurtarma hatası (WAL eksik),
M3 → `LostDurableWrite`, M4 → `ResurrectedDelete`/`PartialTxn`, M5 → kurtarma hatası (boş
checkpoint), M6 → kurtarma hatası (çözülemeyen kayıt; yalnız "ortada yırtık kayıt" ailesiyle).
Kalıcı mutant crate'i ve CI testleri Aşama 5'te.
