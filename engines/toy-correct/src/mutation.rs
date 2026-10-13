//! Bilerek yerleştirilmiş hatalar (Aşama 5, CLAUDE.md karar 4: "araç kendini kanıtlamak zorunda").
//!
//! Neden ayrı bir kod kopyası değil de yapılandırma alanı? Mutant ile doğru motor arasındaki TEK fark
//! mutasyonun kendisi olmalı. Ayrı kopya zamanla sürüklenir ve "araç mutantı yakaladı" iddiası
//! başka bir farka dayanabilir. Cargo feature da reddedildi: workspace'te feature birleşmesi doğru
//! motoru sessizce mutanta çevirebilir.
//!
//! Her mutant, doğru motorun bir dayanıklılık kararını tam olarak bir yerde bozar. Hangi crash-
//! consistency kavramını hedeflediği varyant dokümanında yazılıdır.

/// Bilerek yerleştirilmiş hata. `ToyConfig::mutation == None` doğru motordur.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Mutation {
    /// M1 — **fsync'i atla:** `EveryCommit`'te kayıt fdatasync'lenmeden ACK'lenir. `OnAck` vaadi
    /// bozulur: ACK'li yazma çökmede kaybolabilir (fsync sınırı).
    SkipCommitFsync,
    /// M2 — **fsync sırasını değiştir:** checkpoint'te eski WAL, yeni CHECKPOINT'in rename'i dizin
    /// fsync'iyle kalıcılaşmadan ÖNCE silinir. Çökmede eski CHECKPOINT + silinmiş WAL kalabilir.
    UnlinkBeforeDirSync,
    /// M3 — **WAL replay'de son kaydı at:** kurtarma son geçerli kaydı uygulamaz (sınır hatası).
    DropLastRecord,
    /// M4 — **tombstone'u kurtarmada yoksay:** replay Delete yazmalarını atlar; silinen anahtar
    /// geri gelir.
    IgnoreTombstones,
    /// M5 — **rename'den önce fsync yapma:** CHECKPOINT.tmp içeriği sabitlenmeden rename edilir;
    /// rename kalıcı olup içerik olmayabilir (boş/yarım CHECKPOINT).
    NoFsyncBeforeRename,
    /// M6 — **kısmi kaydı geçerli say:** replay CRC'yi denetlemez; yırtık bir kayıt geçerli sayılır.
    AcceptCorruptRecord,
    /// M7 — **txn atomikliğini boz:** Txn'in her yazması ayrı WAL kaydı olarak yazılır, ama motor
    /// hâlâ `atomic_txn = true` bildirir. Çökme txn'in ortasında kalırsa yarım txn görünür.
    SplitTxnRecords,
}

impl Mutation {
    /// Tüm mutantlar, sabit sırayla.
    pub const ALL: [Mutation; 7] = [
        Mutation::SkipCommitFsync,
        Mutation::UnlinkBeforeDirSync,
        Mutation::DropLastRecord,
        Mutation::IgnoreTombstones,
        Mutation::NoFsyncBeforeRename,
        Mutation::AcceptCorruptRecord,
        Mutation::SplitTxnRecords,
    ];

    /// Kısa ad (CLI ve raporlar: `--engine toy-mutant:<ad>`).
    pub fn name(self) -> &'static str {
        match self {
            Mutation::SkipCommitFsync => "skip-commit-fsync",
            Mutation::UnlinkBeforeDirSync => "unlink-before-dir-sync",
            Mutation::DropLastRecord => "drop-last-record",
            Mutation::IgnoreTombstones => "ignore-tombstones",
            Mutation::NoFsyncBeforeRename => "no-fsync-before-rename",
            Mutation::AcceptCorruptRecord => "accept-corrupt-record",
            Mutation::SplitTxnRecords => "split-txn-records",
        }
    }

    /// Addan mutasyon.
    pub fn from_name(name: &str) -> Option<Mutation> {
        Mutation::ALL.into_iter().find(|m| m.name() == name)
    }
}
