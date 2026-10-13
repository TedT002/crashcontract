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

## Aşama 3 — mutant ön kontrolü (elle enjekte, geri alındı)

`engines/toy-correct/tests/crash.rs` duman testi (18 seed, tüm kesmeler, yırtık geçişi dahil) altı hata
sınıfının her birini yakaladı: M1 → `LostDurableWrite`, M2 → kurtarma hatası (WAL eksik),
M3 → `LostDurableWrite`, M4 → `ResurrectedDelete`/`PartialTxn`, M5 → kurtarma hatası (boş
checkpoint), M6 → kurtarma hatası (çözülemeyen kayıt; yalnız "ortada yırtık kayıt" ailesiyle).
Kalıcı mutant crate'i ve CI testleri Aşama 5'te.
