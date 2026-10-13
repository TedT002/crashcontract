//! İşlem geçmişi: ne gönderildi, ne döndü.
//!
//! ## Neden yalnız SON işlem belirsiz olabilir?
//!
//! İş yükü tek iş parçacıklı ve sıralıdır: bir işlem dönmeden sonrakini göndermeyiz. Ayrıca
//! runner bir işlem `Failed` döndürdüğünde iş yükünü durdurur. Böylece "etkisi belirsiz" işlem
//! en fazla bir tane olur ve geçmişin sonundadır. Bu, izinli küme kontrolünü polinom tutar
//! (belirsiz işlem başına 2 dallanma yerine tek nokta).

use crate::op::{apply_step, steps_of};
use crate::{Durability, Op, State, Value};
use std::fmt;

/// Bir işlemin sonucu.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// Gönderildi, henüz dönmedi (kayıt sürerken geçici durum). Kontrolde InFlight gibi davranır.
    Issued,
    /// Get dışındaki bir işlem başarıyla döndü.
    Acked,
    /// Get başarıyla döndü; `value` okunan değer (None = anahtar yok).
    Read {
        /// Okunan değer; `None` anahtarın bulunmadığı anlamına gelir.
        #[serde(with = "crate::bytes_repr::opt_bytes")]
        value: Option<Value>,
    },
    /// İşlem hata döndürdü. Etkisi belirsizdir (uygulanmış olabilir). Geçmişi kapatır.
    Failed {
        /// Hata açıklaması.
        reason: String,
    },
    /// Çökme anında yarıda kaldı. Etkisi belirsizdir. Geçmişi kapatır.
    InFlight,
}

impl Outcome {
    /// Issued | Failed | InFlight → true.
    pub fn is_uncertain(&self) -> bool {
        matches!(
            self,
            Outcome::Issued | Outcome::Failed { .. } | Outcome::InFlight
        )
    }
}

/// Geçmişteki tek kayıt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    /// İşlem.
    pub op: Op,
    /// Sonucu.
    pub outcome: Outcome,
}

/// Geçmiş üzerinde bir konum: ilk `ops` işlem tamamen + `ops` indeksli işlemin (bir Txn olmalı)
/// ilk `txn_writes` yazması uygulanmış. Kanonik biçim: `txn_writes == 0` veya
/// `0 < txn_writes < writes.len()` (tam uygulanmış txn `Cut{ops+1, 0}` ile gösterilir).
/// Sıra (derive Ord) = geçmişteki konum sırası.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Cut {
    /// Tamamen uygulanmış işlem sayısı.
    pub ops: usize,
    /// `ops` indeksli Txn'in uygulanmış yazma sayısı (çoğunlukla 0).
    pub txn_writes: usize,
}

impl Cut {
    /// `Cut { ops, txn_writes: 0 }`.
    pub const fn at_op(ops: usize) -> Cut {
        Cut { ops, txn_writes: 0 }
    }
}

/// Geçmiş oluşturma/doğrulama hataları.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    /// Önceki işlem (`index`) hâlâ Issued iken yeni işlem gönderilmeye çalışıldı.
    PendingOp {
        /// Hâlâ bekleyen işlemin indeksi.
        index: usize,
    },
    /// Son işlem Failed/InFlight; geçmiş kapalı, yeni işlem eklenemez.
    Closed,
    /// `index` bekleyen (Issued durumundaki son) işlem değil.
    NotPending {
        /// Sonuçlandırılmak istenen indeks.
        index: usize,
    },
    /// Get için `ack` ya da Get olmayan için `ack_read` (veya from_entries'te Read/Acked uyumsuzluğu).
    OutcomeMismatch {
        /// Uyumsuz kaydın indeksi.
        index: usize,
    },
    /// from_entries: belirsiz sonuçlu bir kayıt son kayıt değil.
    UncertainNotLast {
        /// Belirsiz ama son olmayan kaydın indeksi.
        index: usize,
    },
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HistoryError::PendingOp { index } => {
                write!(f, "önceki işlem ({index}) hâlâ bekliyor")
            }
            HistoryError::Closed => write!(f, "geçmiş kapalı: son işlem Failed/InFlight"),
            HistoryError::NotPending { index } => {
                write!(f, "{index} indeksli işlem bekleyen son işlem değil")
            }
            HistoryError::OutcomeMismatch { index } => {
                write!(f, "{index} indeksli işlem ile sonuç türü uyumsuz")
            }
            HistoryError::UncertainNotLast { index } => {
                write!(f, "{index} indeksli kayıt belirsiz ama son kayıt değil")
            }
        }
    }
}

impl std::error::Error for HistoryError {}

/// İşlem geçmişi. Değişmezler (her public yol korur):
///  (I1) Yalnızca son kaydın sonucu belirsiz (Issued/Failed/InFlight) olabilir.
///  (I2) `Outcome::Read` yalnız `Op::Get` için; `Outcome::Acked` yalnız Get olmayan için.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct History {
    // private: değişmezler yalnız API ile korunur
    entries: Vec<Entry>,
}

impl History {
    /// Boş geçmiş.
    pub fn new() -> History {
        History::default()
    }

    /// Hazır kayıtlardan geçmiş kurar; I1/I2'yi doğrular. (Testler ve Aşama 6 rapor okuma için.)
    /// Hata önceliği: kayıtlar baştan sona taranır; ilk bozulan kayıt için önce I2
    /// (`OutcomeMismatch`), sonra I1 (`UncertainNotLast`) kontrol edilir.
    /// Son kaydın `Issued` olması geçerlidir.
    pub fn from_entries(entries: Vec<Entry>) -> Result<History, HistoryError> {
        let last = entries.len().wrapping_sub(1);
        for (index, e) in entries.iter().enumerate() {
            let is_get = matches!(e.op, Op::Get { .. });
            let mismatch = match &e.outcome {
                Outcome::Read { .. } => !is_get,
                Outcome::Acked => is_get,
                _ => false,
            };
            if mismatch {
                return Err(HistoryError::OutcomeMismatch { index });
            }
            if e.outcome.is_uncertain() && index != last {
                return Err(HistoryError::UncertainNotLast { index });
            }
        }
        Ok(History { entries })
    }

    /// Kayıtlar (salt-okunur).
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Kayıt sayısı (= n).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Boş mu?
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Yeni işlemi `Issued` olarak ekler, indeksini döndürür.
    /// Hata: son kayıt Issued → `PendingOp{index: son}`; son kayıt Failed/InFlight → `Closed`.
    pub fn issue(&mut self, op: Op) -> Result<usize, HistoryError> {
        if let Some(last) = self.entries.last() {
            let index = self.entries.len() - 1;
            match last.outcome {
                Outcome::Issued => return Err(HistoryError::PendingOp { index }),
                Outcome::Failed { .. } | Outcome::InFlight => return Err(HistoryError::Closed),
                Outcome::Acked | Outcome::Read { .. } => {}
            }
        }
        self.entries.push(Entry {
            op,
            outcome: Outcome::Issued,
        });
        Ok(self.entries.len() - 1)
    }

    /// `index` gerçekten bekleyen son kayıt mı? Değilse `NotPending`.
    fn check_pending(&self, index: usize) -> Result<(), HistoryError> {
        // `index + 1` yerine `len - 1 == index`: index = usize::MAX iken taşma (panic) olmaz.
        let is_pending_last = self.entries.len().checked_sub(1) == Some(index)
            && matches!(self.entries[index].outcome, Outcome::Issued);
        if is_pending_last {
            Ok(())
        } else {
            Err(HistoryError::NotPending { index })
        }
    }

    /// Bekleyen Get olmayan işlemi `Acked` yapar.
    /// Hata: `index` son kayıt değil ya da son kayıt Issued değil → `NotPending{index}`;
    /// op Get → `OutcomeMismatch{index}`. (Önce NotPending, sonra OutcomeMismatch kontrol edilir.)
    pub fn ack(&mut self, index: usize) -> Result<(), HistoryError> {
        self.check_pending(index)?;
        if matches!(self.entries[index].op, Op::Get { .. }) {
            return Err(HistoryError::OutcomeMismatch { index });
        }
        self.entries[index].outcome = Outcome::Acked;
        Ok(())
    }

    /// Bekleyen Get'i `Read{value}` yapar. Hatalar `ack` ile aynı sırada; op Get değil → OutcomeMismatch.
    pub fn ack_read(&mut self, index: usize, value: Option<Value>) -> Result<(), HistoryError> {
        self.check_pending(index)?;
        if !matches!(self.entries[index].op, Op::Get { .. }) {
            return Err(HistoryError::OutcomeMismatch { index });
        }
        self.entries[index].outcome = Outcome::Read { value };
        Ok(())
    }

    /// Bekleyen işlemi `Failed{reason}` yapar (her op türü için). Hata: `NotPending{index}`.
    pub fn fail(&mut self, index: usize, reason: impl Into<String>) -> Result<(), HistoryError> {
        self.check_pending(index)?;
        self.entries[index].outcome = Outcome::Failed {
            reason: reason.into(),
        };
        Ok(())
    }

    /// Çökme: son kayıt Issued ise InFlight yapar; değilse hiçbir şey yapmaz.
    pub fn mark_crashed(&mut self) {
        if let Some(last) = self.entries.last_mut()
            && last.outcome == Outcome::Issued
        {
            last.outcome = Outcome::InFlight;
        }
    }

    /// Aşama 4 runner'ı için: tam koşu geçmişinden, çökme noktasına göre bir görünüm üretir.
    /// Sonuç: kayıtlar [0, completed) aynen; `in_flight` ise `completed` indeksli işlem
    /// `Outcome::InFlight` ile eklenir; geri kalan atılır.
    ///
    /// Aşama 4'te iz (trace) bir olay indeksinden kesilir; kesilen noktada "kaç işlem tamamen
    /// bitti, sıradaki işlemin ortasında mıyız" sorusunun cevabı bu iki argümandır. Böylece
    /// aynı tam koşudan her çökme noktası için tutarlı bir geçmiş görünümü çıkar.
    ///
    /// # Panics
    /// `completed > len()`; ya da `in_flight && completed >= len()`;
    /// ya da sonuç I1'i bozarsa ([0, completed) içinde son eleman dışında belirsiz kayıt kalırsa,
    /// veya `in_flight` iken [0, completed) içinde belirsiz kayıt varsa).
    pub fn crash_view(&self, completed: usize, in_flight: bool) -> History {
        assert!(
            completed <= self.entries.len(),
            "crash_view: completed ({completed}) > len ({})",
            self.entries.len()
        );
        assert!(
            !in_flight || completed < self.entries.len(),
            "crash_view: in_flight için completed ({completed}) < len olmalı"
        );
        let mut entries: Vec<Entry> = self.entries[..completed].to_vec();
        if in_flight {
            entries.push(Entry {
                op: self.entries[completed].op.clone(),
                outcome: Outcome::InFlight,
            });
        }
        History::from_entries(entries)
            .unwrap_or_else(|e| panic!("crash_view: sonuç değişmezleri bozuyor: {e}"))
    }

    /// `cut` konumuna kadar tüm adımları (sonuçtan bağımsız) boş durumdan uygulayıp durumu döndürür.
    /// # Panics
    /// `cut.ops > len()`; `cut.txn_writes > 0` iken `cut.ops` indeksli işlem yok/Txn değil ya da
    /// `cut.txn_writes >= writes.len()`.
    pub fn state_at(&self, cut: Cut) -> State {
        assert!(
            cut.ops <= self.entries.len(),
            "state_at: cut.ops ({}) > len ({})",
            cut.ops,
            self.entries.len()
        );
        let mut state = State::new();
        for e in &self.entries[..cut.ops] {
            for (k, v) in steps_of(&e.op) {
                apply_step(&mut state, k, v);
            }
        }
        if cut.txn_writes > 0 {
            let entry = self
                .entries
                .get(cut.ops)
                .unwrap_or_else(|| panic!("state_at: {} indeksli işlem yok", cut.ops));
            let Op::Txn { writes } = &entry.op else {
                panic!("state_at: {} indeksli işlem Txn değil", cut.ops);
            };
            assert!(
                cut.txn_writes < writes.len(),
                "state_at: txn_writes ({}) >= writes.len() ({})",
                cut.txn_writes,
                writes.len()
            );
            for (k, v) in steps_of(&entry.op).into_iter().take(cut.txn_writes) {
                apply_step(&mut state, k, v);
            }
        }
        state
    }

    /// Dayanıklılık tabanı F.
    ///
    /// `F`, fsync sınırının modeldeki karşılığıdır: ilk `F` işlemin etkisi çökmeden sonra
    /// kesinlikle görülmelidir.
    /// * `OnAck`: baştaki ardışık `Acked`/`Read` kayıtların sayısı (I1 gereği son kayıt
    ///   belirsizse `n-1`, değilse `n`).
    /// * `OnSync`: ACK'li son `Sync`'in indeksi + 1; yoksa 0. Henüz dönmemiş (Issued/InFlight)
    ///   ya da başarısız Sync tabanı YÜKSELTMEZ: sync hiç dönmediyse (Issued/InFlight) ya da
    ///   hatayla döndüyse (Failed) bariyerin kurulduğuna dair kanıt yoktur; fsync yarıda kesilmiş
    ///   ya da başarısız olmuş olabilir.
    /// * `NoGuarantee`: 0.
    pub fn durable_floor(&self, durability: Durability) -> usize {
        match durability {
            Durability::OnAck => self
                .entries
                .iter()
                .take_while(|e| matches!(e.outcome, Outcome::Acked | Outcome::Read { .. }))
                .count(),
            Durability::OnSync => self
                .entries
                .iter()
                .rposition(|e| matches!(e.op, Op::Sync) && e.outcome == Outcome::Acked)
                .map_or(0, |i| i + 1),
            Durability::NoGuarantee => 0,
        }
    }
}

/// Serileştirme: kayıt listesi. Okurken `from_entries` değişmezleri (I1/I2) doğrular — elle
/// düzenlenmiş ya da bozuk bir rapor geçersiz bir geçmiş kuramaz.
impl serde::Serialize for History {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.entries.serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for History {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<History, D::Error> {
        let entries = Vec::<Entry>::deserialize(d)?;
        History::from_entries(entries).map_err(serde::de::Error::custom)
    }
}
