//! # cc-adapter — test edilen motorun sözleşmesi
//!
//! [`StorageAdapter`] her motoru (toy ya da gerçek) aynı arayüze indirger. En önemli parçası
//! [`StorageAdapter::guarantees`]: model motoru **bildirdiği** garantilerle yargılar
//! (CLAUDE.md karar 3), bu yüzden bildirim adaptörün sorumluluğundadır.
//!
//! [`apply_op`] runner'ın (Aşama 4) tek uygulama noktasıdır: geçmişe kayıt, iz aralığı (span) ve
//! adaptör çağrısı aynı sırayla, tek yerde yapılır. İşlem↔iz eşlemesindeki bir hata sessiz yanlış
//! alarm ya da kaçırma üretir; tek nokta bu riski küçültür.

pub mod runner;

use std::fmt;
use std::path::Path;

use cc_model::{Guarantees, History, HistoryError, Op, State, Value, Write};
use cc_trace::TraceLog;

/// Adaptör hatası.
#[derive(Debug)]
pub enum AdapterError {
    /// Alttaki G/Ç hatası.
    Io(std::io::Error),
    /// Kalıcı veri, motorun kendi biçim kurallarına göre tutarsız (kurtarma reddedildi).
    Corruption(String),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdapterError::Io(e) => write!(f, "G/Ç hatası: {e}"),
            AdapterError::Corruption(msg) => write!(f, "bozuk kalıcı veri: {msg}"),
        }
    }
}

impl std::error::Error for AdapterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AdapterError::Io(e) => Some(e),
            AdapterError::Corruption(_) => None,
        }
    }
}

impl From<std::io::Error> for AdapterError {
    fn from(e: std::io::Error) -> Self {
        AdapterError::Io(e)
    }
}

/// Test edilen motorun sözleşmesi. Tek iş parçacıklı; her çağrı dönene kadar engeller.
pub trait StorageAdapter: Sized {
    /// Motor yapılandırması (Aşama 6 raporuna girer).
    type Config: Clone + std::fmt::Debug;

    /// Bildirilen garantiler YAPILANDIRMANIN fonksiyonudur (açık örnek gerektirmez: açılamayan
    /// çökme durumunda da hüküm verilebilsin).
    fn guarantees(config: &Self::Config) -> Guarantees;

    /// `dir` mevcut olmalı. `log` Some ise motorun tüm dosya sistemi olayları ona eklenir
    /// (toy: `DirVfs`; gerçek motor: Aşama 7 shim). Açılış olayları ilk span'den önce yer alır.
    fn open(dir: &Path, config: &Self::Config, log: Option<TraceLog>)
    -> Result<Self, AdapterError>;

    /// Anahtara değer yazar.
    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), AdapterError>;

    /// Anahtarın değerini okur (None = yok).
    fn get(&mut self, key: &[u8]) -> Result<Option<Value>, AdapterError>;

    /// Anahtarı siler.
    fn delete(&mut self, key: &[u8]) -> Result<(), AdapterError>;

    /// `Op::Txn` karşılığı (begin…commit tek çağrıda). Yerel txn'i olmayan motor yazmaları sırayla
    /// uygular ve `atomic_txn = false` bildirir — cc-model semantiği tam budur; ayrı yetenek bayrağı yok.
    fn txn(&mut self, writes: &[Write]) -> Result<(), AdapterError>;

    /// Dayanıklılık bariyeri (`Op::Sync`).
    fn sync(&mut self) -> Result<(), AdapterError>;

    /// Motorun gördüğü tam mantıksal durum (`check_recovered`'a girer; hayalet anahtarları da yakalar).
    fn scan(&mut self) -> Result<State, AdapterError>;

    /// Motorun kendi bütünlük kontrolü; None = motor desteklemiyor ("geçti" iddiası değil).
    fn integrity_check(&mut self) -> Option<Result<(), AdapterError>> {
        None
    }

    /// Temiz kapanış. Dayanıklılık vaadi EKLEMEZ (yalnız kaynakları bırakır).
    fn close(self) -> Result<(), AdapterError>;
}

/// Runner'ın (Aşama 4) tek uygulama noktası: `history.issue(op)` → `log.begin_op(i)` → adaptör
/// çağrısı → `log.end_op(i)` → Put/Delete/Txn/Sync başarı: `ack(i)`; Get başarı: `ack_read(i, v)`;
/// adaptör hatası: `fail(i, err.to_string())`. `i` döner.
///
/// Hata: yalnız `issue` hatası (ör. `Closed`) — o durumda adaptör çağrılmaz, span açılmaz.
pub fn apply_op<A: StorageAdapter>(
    adapter: &mut A,
    history: &mut History,
    log: Option<&TraceLog>,
    op: Op,
) -> Result<usize, HistoryError> {
    // Sıra önemli: işlem geçmişe span'den ÖNCE girer; böylece span indeksi == geçmiş indeksi.
    let i = history.issue(op.clone())?;
    if let Some(log) = log {
        log.begin_op(i);
    }
    // Get sonucu Some(okunan), diğerleri None taşır.
    let result: Result<Option<Option<Value>>, AdapterError> = match &op {
        Op::Put { key, value } => adapter.put(key, value).map(|()| None),
        Op::Delete { key } => adapter.delete(key).map(|()| None),
        Op::Get { key } => adapter.get(key).map(Some),
        Op::Txn { writes } => adapter.txn(writes).map(|()| None),
        Op::Sync => adapter.sync().map(|()| None),
    };
    if let Some(log) = log {
        log.end_op(i);
    }
    // Aşağıdaki ack/fail hataları imkânsızdır: `i` az önce issue edilen, hâlâ bekleyen son kayıttır
    // ve sonucun türü op türüyle eşleşir.
    match result {
        Ok(None) => history.ack(i),
        Ok(Some(value)) => history.ack_read(i, value),
        Err(e) => history.fail(i, e.to_string()),
    }
    .expect("değişmez: az önce issue edildi");
    Ok(i)
}
