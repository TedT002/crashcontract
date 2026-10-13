//! İz verisinin (`TraceEvent::Write.data`) raporda onaltılık gösterimi.
//!
//! cc-model'in `bytes_repr`'ı yerine burada küçük bir kopya: cc-trace modelden bağımsız alt katmandır,
//! bir serileştirme yardımcısı için ona bağımlı olmamalı.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(crate) fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
    let h: String = v.iter().map(|b| format!("{b:02x}")).collect();
    h.serialize(s)
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(d)?;
    if s.len() % 2 != 0 {
        return Err(D::Error::custom("tek uzunluklu onaltılık dize"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| D::Error::custom("geçersiz onaltılık dize"))
        })
        .collect()
}
