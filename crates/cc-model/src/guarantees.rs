//! Motorun bildirdiği garantiler.

/// Motorun BİLDİRDİĞİ dayanıklılık modu. Model motoru vaat ettiğiyle yargılar:
/// vaat etmediği bir şeyi kaybetmesi ihlal değildir.
///
/// **ACK ≠ dayanıklılık.** Motorun "tamam" dönmesi, verinin diske ulaştığı anlamına gelmez:
/// `write()` çoğu zaman yalnız OS sayfa önbelleğine yazar. BU MODELDE veri ancak
/// `fsync`/`fdatasync` döndüğünde kalıcı sayılır (fsync sınırı). Not: `O_SYNC`/`O_DSYNC` ile açılmış
/// dosyalar ve `sync`/`syncfs` de kalıcılık sağlar; bunların gerçek motorlarda nasıl göründüğü
/// Aşama 3/7'de `strace` ile ÖLÇÜLECEK, şimdilik model yalnız fsync sınırını bilir.
///
/// Sayfa önbelleği SÜREÇ ölümünde korunur; sabitlenmemiş sayfalar yalnız SİSTEM çökmesinde/güç
/// kaybında kaybolabilir. (Kullanıcı alanı tamponları — ör. henüz `write()` edilmemiş veri — ise
/// süreç ölümünde de kaybolur.) Bu enum, hangi ACK'in fsync sınırını geçtiğini söyler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Durability {
    /// Her başarılı yazma/txn/sync ACK'i dönmeden önce dayanıklıdır (ör. commit başına fsync).
    OnAck,
    /// Yazmalar yalnızca ACK'li bir `Op::Sync` sonrasında dayanıklıdır (ör. grup commit,
    /// sayfa önbelleğinde bekleyen WAL). Sync'ten önceki her şey Sync ACK'iyle sabitlenir.
    ///
    /// Sync'ten önce ACK'lenen yazmalar sayfa önbelleğinde kalmış olabilir; çökmede kaybolmaları
    /// motorun hatası değildir, çünkü motor bunu vaat etmemiştir.
    OnSync,
    /// Hiçbir dayanıklılık vaadi yok. Kurtarılan durum yine de hiç yazılmamış veri içeremez
    /// ve (prefix_order ise) bir prefix olmalıdır.
    NoGuarantee,
}

/// Motorun bildirdiği garantilerin tamamı. Tüm kombinasyonlar geçerlidir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Guarantees {
    /// Dayanıklılık modu.
    pub durability: Durability,
    /// true: bir Txn'in yazmaları çökme sonrası ya hep ya hiç görünür.
    /// false: Txn, sıralı bağımsız yazmalar gibi değerlendirilir (txn içinde kesme serbest).
    pub atomic_txn: bool,
    /// true: kurtarılan durum geçmişin bir prefix'ine eşit olmalı (sonraki bir yazma duruyorsa
    /// öncekiler kaybolmamış olmalı). false: yalnızca anahtar-bazlı kural.
    ///
    /// Bu garanti sıralı, checksum'lı bir WAL'dan doğar: replay kayıtları sırayla okur ve ilk
    /// bozuk/eksik kayıtta durur; dolayısıyla sonraki kayıtlar, öncekiler olmadan uygulanmaz.
    /// ÖN KOŞUL: kuyruğun ardında eski ama CRC'si geçerli bir kayıt kalmamalıdır (yeniden
    /// kullanılan WAL ya da kalıcılaşmamış `ftruncate` bunu bozabilir); bu yüzden kayıtlar
    /// epoch/LSN taşımalı ve replay eski epoch'u reddetmelidir.
    pub prefix_order: bool,
}
