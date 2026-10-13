//! Modelin hükmü ve ihlal raporu.

use crate::{Cut, Key, Value};
use std::fmt;

/// İhlal sınıfı. Minimizer "aynı türde ihlal hâlâ var mı?" sorusunu bununla sorar.
///
/// Her sınıfın tipik gerçek hata karşılığı (BEKLENEN eşleme; Aşama 5'te mutantlarla ÖLÇÜLECEK):
/// fsync atlama → `LostDurableWrite`; WAL replay'de son kaydı atma → `LostDurableWrite` /
/// `PrefixViolation`; tombstone yoksayma → `ResurrectedDelete`; kısmi kaydı geçerli sayma →
/// `PhantomValue` / `PartialTxn`; rename öncesi fsync yapmama → `LostDurableWrite` /
/// `PrefixViolation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ViolationKind {
    /// Dayanıklılık tabanının altındaki (vaat edilmiş) bir yazmanın etkisi kayıp ya da eski değer döndü.
    LostDurableWrite,
    /// Prefix tutarlılığı bozuk: tek başına her anahtar izinli ama hiçbir prefix hepsini açıklamıyor
    /// (sonraki bir yazma duruyor, önceki kayıp — sıra dışı kayıp).
    PrefixViolation,
    /// O anahtara hiç yazılmamış bir değer belirdi (çöp/bozuk kayıt/yanlış anahtar).
    PhantomValue,
    /// Atomik bildirilen bir Txn kısmen uygulanmış.
    PartialTxn,
    /// Dayanıklı biçimde silinmiş bir anahtar eski değeriyle geri geldi (tombstone kaybı).
    ResurrectedDelete,
    /// Çökmeden önce, çalışma sırasında Get modelden farklı değer döndürdü.
    ReadMismatch,
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ViolationKind::LostDurableWrite => "dayanıklı yazma kayıp",
            ViolationKind::PrefixViolation => "prefix ihlali",
            ViolationKind::PhantomValue => "hayalet değer",
            ViolationKind::PartialTxn => "kısmi txn",
            ViolationKind::ResurrectedDelete => "silinen veri geri geldi",
            ViolationKind::ReadMismatch => "okuma uyuşmazlığı",
        })
    }
}

/// Bir ihlalin raporu (minimizer ve JSON rapor için yeterli bilgi).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Sınıf.
    pub kind: ViolationKind,
    /// İhlalin görüldüğü anahtar (birden fazlaysa bayt sırasına göre ilki).
    pub key: Key,
    /// Bu anahtar için beklenen aday değerler; sıralı (Option<Value> Ord: None önce) ve tekrarsız.
    pub expected: Vec<Option<Value>>,
    /// Kurtarılan durumda (veya ReadMismatch'te okumada) bulunan değer.
    pub found: Option<Value>,
    /// İlgili işlemin geçmişteki indeksi. PhantomValue'da None.
    pub op_index: Option<usize>,
    /// Çapraz-anahtar ihlallerinde (prefix modu) en iyi uyan kesme noktası; diğerlerinde None.
    pub best_cut: Option<Cut>,
    /// Hesaplanan dayanıklılık tabanı F; ReadMismatch'te None.
    pub durable_floor: Option<usize>,
}

/// Değeri insan okunur yazar (`None` → `yok`).
fn show(v: &Option<Value>) -> String {
    match v {
        None => "yok".to_string(),
        Some(v) => format!("\"{}\"", v.escape_ascii()),
    }
}

impl fmt::Display for Violation {
    // Biçim insan içindir, kontrat DEĞİLDİR.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let expected: Vec<String> = self.expected.iter().map(show).collect();
        write!(
            f,
            "{}: anahtar \"{}\", beklenen [{}], bulunan {}",
            self.kind,
            self.key.escape_ascii(),
            expected.join(", "),
            show(&self.found)
        )?;
        if let Some(i) = self.op_index {
            write!(f, ", işlem #{i}")?;
        }
        if let Some(floor) = self.durable_floor {
            write!(f, ", taban F={floor}")?;
        }
        if let Some(c) = self.best_cut {
            write!(f, ", en iyi kesme {}/{}", c.ops, c.txn_writes)?;
        }
        Ok(())
    }
}

/// Modelin hükmü. Hata değil, değerdir.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum Verdict {
    /// Kurtarılan durum izinli kümede.
    /// `witness`: prefix modunda durumu açıklayan EN SON izinli kesme; anahtar-bazlı modda ve
    /// `check_reads`'te None.
    Allowed {
        /// Durumu açıklayan en son izinli kesme (varsa).
        witness: Option<Cut>,
    },
    /// İzinli küme dışında; birincil ihlal.
    Violated(Violation),
}

impl Verdict {
    /// Allowed mı?
    pub fn is_allowed(&self) -> bool {
        matches!(self, Verdict::Allowed { .. })
    }

    /// Violated ise ihlale referans.
    pub fn violation(&self) -> Option<&Violation> {
        match self {
            Verdict::Violated(v) => Some(v),
            Verdict::Allowed { .. } => None,
        }
    }
}
