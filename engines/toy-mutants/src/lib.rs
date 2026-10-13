//! # toy-mutants — bilerek hatalı motorlar kataloğu
//!
//! CLAUDE.md karar 4: "Araç kendini kanıtlamak zorunda." Her mutant, `toy-correct`'in tek bir
//! dayanıklılık kararını bozar ([`toy_correct::Mutation`]); bu crate hangi hata sınıfını temsil
//! ettiklerini, hangi yapılandırmada ortaya çıkabildiklerini ve aracın onları hangi bulgu türüyle
//! yakalaması beklendiğini tutar. `tests/catch.rs` her mutantın gerçekten yakalandığını doğrular.
//!
//! Neden mutant başına bir yapılandırma? Bazı hatalar yalnız belirli koşullarda var olur: commit
//! fsync'ini atlamak (`skip-commit-fsync`) zaten fsync yapmayan `OnSyncOnly` politikasında fark
//! yaratmaz; eski WAL'ı erken silmek (`unlink-before-dir-sync`) checkpoint olmadan tetiklenmez.
//! "Yakalanmadı" iddiası ancak hatanın var olabildiği yapılandırmada anlamlıdır.

use cc_model::ViolationKind;
use toy_correct::{Mutation, SyncPolicy, ToyConfig};

/// Aracın bir mutant için üretmesi beklenen bulgu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Expected {
    /// Model ihlali, bu sınıfta.
    Violation(ViolationKind),
    /// Motor çökme durumundan açılamadı.
    RecoveryFailed,
}

/// Bir mutantın kataloğu.
#[derive(Debug, Clone)]
pub struct MutantSpec {
    /// Mutasyon.
    pub mutation: Mutation,
    /// CLAUDE.md Aşama 5'teki hata sınıfı.
    pub bug_class: &'static str,
    /// Hedeflenen crash-consistency kavramı.
    pub concept: &'static str,
    /// Hatanın ortaya çıkabildiği yapılandırma (mutasyon alanı dolu).
    pub config: ToyConfig,
    /// Yırtık yazma gerekiyor mu? (yalnız `accept-corrupt-record`)
    pub needs_torn: bool,
    /// Beklenen bulgu türleri (ölçülerek doğrulandı, bkz. `tests/catch.rs`).
    pub expected: &'static [Expected],
}

const fn cfg(m: Mutation, sync_policy: SyncPolicy, checkpoint_every: u32) -> ToyConfig {
    ToyConfig {
        sync_policy,
        checkpoint_every,
        mutation: Some(m),
    }
}

use Expected::{RecoveryFailed, Violation as V};
use ViolationKind::{
    LostDurableWrite, PartialTxn, PhantomValue, PrefixViolation, ResurrectedDelete,
};

/// Tüm mutantlar, [`Mutation::ALL`] sırasıyla.
pub fn catalog() -> Vec<MutantSpec> {
    use Mutation::*;
    use SyncPolicy::{EveryCommit, OnSyncOnly};
    vec![
        MutantSpec {
            mutation: SkipCommitFsync,
            bug_class: "fsync'i atla",
            concept: "fsync sınırı: ACK ≥ dayanıklılık (OnAck)",
            config: cfg(SkipCommitFsync, EveryCommit, 0),
            needs_torn: false,
            expected: &[V(LostDurableWrite), V(ResurrectedDelete)],
        },
        MutantSpec {
            mutation: UnlinkBeforeDirSync,
            bug_class: "fsync sırasını değiştir",
            concept: "ad değişikliği yalnız dizin fsync'iyle kalıcı; eski WAL yeni CHECKPOINT'ten önce silinmez",
            config: cfg(UnlinkBeforeDirSync, EveryCommit, 3),
            needs_torn: false,
            expected: &[RecoveryFailed],
        },
        MutantSpec {
            mutation: DropLastRecord,
            bug_class: "WAL replay'de son kaydı at",
            concept: "WAL replay sınırı",
            config: cfg(DropLastRecord, EveryCommit, 0),
            needs_torn: false,
            expected: &[V(LostDurableWrite), V(ResurrectedDelete)],
        },
        MutantSpec {
            mutation: IgnoreTombstones,
            bug_class: "silmeyi (tombstone) kurtarmada yoksay",
            concept: "tombstone replay",
            config: cfg(IgnoreTombstones, EveryCommit, 0),
            needs_torn: false,
            expected: &[V(ResurrectedDelete), V(PartialTxn)],
        },
        MutantSpec {
            mutation: NoFsyncBeforeRename,
            bug_class: "rename'den önce fsync yapma",
            concept: "rename ile atomik değiştirme: içerik rename'den önce sabitlenmeli",
            config: cfg(NoFsyncBeforeRename, EveryCommit, 3),
            needs_torn: false,
            expected: &[RecoveryFailed],
        },
        MutantSpec {
            mutation: AcceptCorruptRecord,
            bug_class: "kısmi kaydı geçerli say",
            concept: "yırtık yazma: checksum'sız kayıt geçerli sayılamaz",
            config: cfg(AcceptCorruptRecord, OnSyncOnly, 0),
            needs_torn: true,
            // Sıfırla dolu yırtık kuyruk geçerli sayılınca çoğunlukla hiç yazılmamış değer
            // (PhantomValue) görünür; çözülemeyen çöp ise kurtarmayı düşürür.
            expected: &[RecoveryFailed, V(PhantomValue), V(PrefixViolation)],
        },
        MutantSpec {
            mutation: SplitTxnRecords,
            bug_class: "txn atomikliğini boz (ek sınıf)",
            concept: "atomik txn = tek checksum'lı kayıt",
            config: cfg(SplitTxnRecords, EveryCommit, 0),
            needs_torn: false,
            // Yarım txn'in görünen ara değeri bir Delete (yok) ya da eski bir değerse, sınıflandırıcı
            // eski-değer açıklamasını önceler (SPEC_stage1 §4.6(a) kural sırası) → LostDurableWrite.
            // Hüküm (ihlal) aynıdır; yalnız etiket iki geçerli açıklamadan birini seçer.
            expected: &[V(PartialTxn), V(LostDurableWrite)],
        },
    ]
}
