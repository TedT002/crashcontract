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

## Aşama 3 — mutant ön kontrolü (elle enjekte, geri alındı)

`engines/toy-correct/tests/crash.rs` duman testi (18 seed, tüm kesmeler, yırtık geçişi dahil) altı hata
sınıfının her birini yakaladı: M1 → `LostDurableWrite`, M2 → kurtarma hatası (WAL eksik),
M3 → `LostDurableWrite`, M4 → `ResurrectedDelete`/`PartialTxn`, M5 → kurtarma hatası (boş
checkpoint), M6 → kurtarma hatası (çözülemeyen kayıt; yalnız "ortada yırtık kayıt" ailesiyle).
Kalıcı mutant crate'i ve CI testleri Aşama 5'te.
