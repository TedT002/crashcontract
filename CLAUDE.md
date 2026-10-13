# CrashContract

Rust ile yazılmış, depolama motorlarının çökme sonrası kurtarma doğruluğunu test eden bir araç.
Bir motora seed'li bir iş yükü uygular, I/O işlemlerini kaydeder, bu kayıttan **izinli çökme durumları**
üretir, motoru her durumdan yeniden açar ve sonucu bir referans modelle karşılaştırır.
Hata bulunursa dizi otomatik küçültülür ve tek komutla yeniden oynatılabilir bir rapor üretilir.

## Çalışma kuralları (Claude Code için)

- Kodun içindeki **öğretici yorumları koru ve yaz**: neden bu tasarım seçildi, hangi crash-consistency
  kavramı (fsync sınırı, torn write, WAL replay vb.) burada devreye giriyor. Yorumları "temizleme".
- Her aşamayı ayrı ayrı bitir. Bir aşamanın testleri geçmeden sonrakine geçme.
- Bilmediğin bir davranışı (ör. bir motorun fsync düzeni) tahmin etme; önce `strace` ile ölç, sonra yaz.
- README'de ve kodda yalnızca **gerçekten doğrulanmış** iddialar olsun. Doğrulanmayan garantileri
  "Doğrulanmayanlar" bölümüne yaz.
- Bağımlılıkları minimumda tut. Her yeni crate için nedenini bir yorumla belirt.

## Tasarım kararları (değiştirme, tartışmadan)

1. **Süreci rastgele öldürmeyeceğiz.** Dışarıdan `kill` deterministik değildir ve OS sayfa önbelleği
   yazıları koruduğu için fsync hatalarını yakalamaz. Bunun yerine **kaydet-ve-oynat** yaklaşımı:
   motorun dosya sistemi çağrıları (`write`, `pwrite`, `fsync`, `fdatasync`, `rename`, `ftruncate`,
   `unlink`) bir iz (trace) olarak kaydedilir.
2. **Çökme durumları izden üretilir.** "İzin N. olayında kes" ve "fsync ile sabitlenmemiş yazıları
   izinli sıralamalarla düşür" kurallarıyla dizin durumları oluşturulur. Aynı seed ve aynı iz her zaman
   aynı durumları üretir; yeniden üretilebilirlik buradan gelir.
3. **Model tek bir beklenen durum değil, izinli durumlar kümesidir.** Çökme anında yarıda kalan işlem
   uygulanmış da olabilir, uygulanmamış da. Ayrıca prefix tutarlılığı aranır: sonraki bir ACK'li yazma
   duruyorsa öncekiler kaybolmamalı (tek iş parçacıklı sıralı iş yükü için).
4. **Araç kendini kanıtlamak zorunda.** Bilerek hatalı ("mutant") motorlar yazılır; araç bunları
   yakalayamıyorsa proje bitmemiştir.
5. **Kapsam sınırı:** tek iş parçacıklı sıralı iş yükü, Linux, süreç ve dosya sistemi çağrısı seviyesi.
   Gerçek güç kaybı, blok seviyesi hata, eşzamanlı linearizability ve mmap/io_uring kullanan motorlar
   v1 kapsamı dışındadır ve README'de açıkça yazılır.

## Mimari

```
crashcontract/
  crates/
    cc-model/       # Referans model: izinli durumlar kümesi, ACK/dayanıklılık kuralları
    cc-trace/       # I/O iz formatı, kaydedici, iz üzerinde crash-state üreteci
    cc-workload/    # Seed'li PUT/GET/DELETE/txn üreteci
    cc-adapter/     # StorageAdapter trait'i + motor adaptörleri
    cc-minimize/    # ddmin: işlem dizisi, çökme noktası, anahtar uzayı, değer boyutu
    cc-shim/        # (cdylib) LD_PRELOAD ile gerçek motorların libc çağrılarını kaydeden katman
    cc-cli/         # run / replay / minimize
  engines/
    toy-correct/    # Doğru çalışan küçük WAL'lı referans motor (Vfs trait'i üzerinden yazar)
    toy-mutants/    # Bilerek hatalı sürümler (aşağıda)
  failures/         # Üretilen JSON raporlar (replay edilebilir)
```

`StorageAdapter` trait'i en az şunları içermeli: `open`, `put`, `get`, `delete`, `begin/commit`
(destekliyorsa), `sync`, `integrity_check` (varsa) ve motorun **bildirdiği garantiler**
(`Durability` modu, işlem atomikliği, sıralama garantisi). Model bu bildirimlere göre hüküm verir.

## Aşamalar

1. **İskelet + model:** workspace, CI (fmt, clippy, test), `cc-model` ve birim testleri.
2. **Trace + toy motor:** `Vfs` trait'i, iz kaydı, `toy-correct` motoru (WAL + checkpoint).
3. **Crash-state üreteci:** iz kesme + fsync sınırına saygılı yazı düşürme (+ opsiyonel sektör
   seviyesinde torn write). Determinizm testleri.
4. **Oracle:** kurtarılan durumu izinli kümeyle karşılaştır. `toy-correct` üzerinde yanlış alarm = 0
   hedefi, 10k+ seed ile.
5. **Mutantlar:** en az 6 hata sınıfı. Örnek: fsync'i atla, fsync sırasını değiştir, WAL replay'de son
   kaydı at, silmeyi (tombstone) kurtarmada yoksay, rename'den önce fsync yapma, kısmi kaydı geçerli say.
   Her mutant için "araç yakalıyor mu" testi.
6. **Minimizer + rapor:** ddmin ile minimum karşı örnek; JSON rapor (motor sürümü, yapılandırma, seed,
   işlem geçmişi, iz, kurtarma sonrası durum); `replay` komutu.
7. **Gerçek motor adaptörü:** önce `strace -f -e trace=file,desc` ile motorun (ör. redb) hangi
   çağrıları yaptığını ölç; sonra `cc-shim` ile kaydet. mmap/io_uring kullanıyorsa kapsam dışı notu düş.
8. **Sunum:** README (yakalanan hata sınıfları tablosu, kaç senaryo koşuldu, minimizasyon oranı,
   doğrulanmayanlar), CI'da mutant testlerinin çalışması, ikinci motor opsiyonel.

## Bitti sayılma ölçütü

- Tüm mutantlar yakalanıyor ve minimize ediliyor.
- `toy-correct` ve en az bir gerçek motor üzerinde yanlış alarm yok.
- `crashcontract replay` bir JSON rapordan aynı sonucu bire bir üretiyor.
- README'de ölçülmüş sayılar var, tahmini iddia yok.
- Gerçek bir motorda hata bulmak **bonus**; projenin tamamlanması buna bağlı değil.
