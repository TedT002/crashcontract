//! Adaptör ↔ sürücü protokolü: satır başına bir JSON istek (stdin) ve bir JSON yanıt (stdout).
//!
//! Neden ayrı süreç? LD_PRELOAD shim'i bütün süreci etkiler. Motor runner'la aynı süreçte olsaydı
//! runner'ın kendi dosya işlemleri (materialize, rapor) de ize karışırdı; ayrıca bir çökme durumundan
//! yeniden açılış, önceki koşunun süreç içi önbelleklerinden tamamen bağımsız olmalıdır.

use cc_model::{Op, State, Value};

/// Sürücüye istek.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "req", rename_all = "snake_case")]
pub enum Request {
    /// Bir işlemi uygula.
    Op {
        /// İşlem.
        op: Op,
    },
    /// Tam mantıksal durumu döndür.
    Scan,
    /// Veritabanını kapat ve çık.
    Close,
}

/// Sürücünün yanıtı.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "resp", rename_all = "snake_case")]
pub enum Response {
    /// Açılış tamamlandı (ilk satır).
    Ready,
    /// İşlem başarılı (Get dışı) ya da kapanış tamam.
    Ok,
    /// Get sonucu.
    Value {
        /// Okunan değer.
        #[serde(with = "cc_model::bytes_repr::opt_bytes")]
        value: Option<Value>,
    },
    /// Scan sonucu.
    State {
        /// Tam durum.
        #[serde(with = "cc_model::bytes_repr::state")]
        state: State,
    },
    /// Hata (açılış ya da işlem).
    Err {
        /// Açıklama.
        message: String,
    },
}
