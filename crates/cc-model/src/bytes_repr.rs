//! Bayt dizilerinin JSON gösterimi (`#[serde(with = "...")]` yardımcıları).
//!
//! Neden serde'nin varsayılanı (sayı dizisi) değil? Rapor bir insan tarafından okunacak: `"k3"` ile
//! `[107, 51]` arasındaki fark, bir hatayı dakikalar içinde anlamakla saatler harcamak arasındaki
//! farktır. Gösterim **tersinirdir** (replay bayt bayt aynı girdiyi üretmeli): yazdırılabilir ASCII
//! (`0x20..=0x7E`) aynen, `\` → `\\`, diğer her bayt → `\xNN` (küçük harf).

use std::collections::BTreeMap;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Baytları kaçışlı dizeye çevirir.
pub fn encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\\' => s.push_str("\\\\"),
            0x20..=0x7E => s.push(b as char),
            _ => s.push_str(&format!("\\x{b:02x}")),
        }
    }
    s
}

/// [`encode`]'un tersi. Geçersiz kaçış ya da ASCII olmayan karakter → hata.
pub fn decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len());
    let mut it = s.bytes();
    while let Some(b) = it.next() {
        match b {
            b'\\' => match it.next() {
                Some(b'\\') => out.push(b'\\'),
                Some(b'x') => {
                    let hi = it.next().ok_or("yarım \\x kaçışı")?;
                    let lo = it.next().ok_or("yarım \\x kaçışı")?;
                    let hex = [hi, lo];
                    let hex = std::str::from_utf8(&hex).map_err(|_| "geçersiz \\x kaçışı")?;
                    let v = u8::from_str_radix(hex, 16).map_err(|_| "geçersiz \\x kaçışı")?;
                    out.push(v);
                }
                _ => return Err("bilinmeyen kaçış".into()),
            },
            0x20..=0x7E => out.push(b),
            _ => return Err(format!("kaçışsız bayt 0x{b:02x}")),
        }
    }
    Ok(out)
}

/// `Vec<u8>` alanı için.
pub mod bytes {
    use super::*;

    /// Serileştir.
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        encode(v).serialize(s)
    }

    /// Ters serileştir.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        decode(&String::deserialize(d)?).map_err(D::Error::custom)
    }
}

/// `Option<Vec<u8>>` alanı için (`null` = yok).
pub mod opt_bytes {
    use super::*;

    /// Serileştir.
    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        v.as_deref().map(encode).serialize(s)
    }

    /// Ters serileştir.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| decode(&s).map_err(D::Error::custom))
            .transpose()
    }
}

/// `Vec<Option<Vec<u8>>>` alanı için.
pub mod vec_opt_bytes {
    use super::*;

    /// Serileştir.
    pub fn serialize<S: Serializer>(v: &[Option<Vec<u8>>], s: S) -> Result<S::Ok, S::Error> {
        let enc: Vec<Option<String>> = v.iter().map(|x| x.as_deref().map(encode)).collect();
        enc.serialize(s)
    }

    /// Ters serileştir.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Option<Vec<u8>>>, D::Error> {
        Vec::<Option<String>>::deserialize(d)?
            .into_iter()
            .map(|x| x.map(|s| decode(&s).map_err(D::Error::custom)).transpose())
            .collect()
    }
}

/// `State` (`BTreeMap<Vec<u8>, Vec<u8>>`) için: JSON nesnesi, anahtar ve değer kaçışlı dize.
pub mod state {
    use super::*;

    /// Serileştir.
    pub fn serialize<S: Serializer>(v: &crate::State, s: S) -> Result<S::Ok, S::Error> {
        let enc: BTreeMap<String, String> = v.iter().map(|(k, v)| (encode(k), encode(v))).collect();
        enc.serialize(s)
    }

    /// Ters serileştir.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<crate::State, D::Error> {
        BTreeMap::<String, String>::deserialize(d)?
            .into_iter()
            .map(|(k, v)| {
                Ok((
                    decode(&k).map_err(D::Error::custom)?,
                    decode(&v).map_err(D::Error::custom)?,
                ))
            })
            .collect()
    }
}

/// `Option<State>` alanı için.
pub mod opt_state {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct Wrap(#[serde(with = "super::state")] crate::State);

    /// Serileştir.
    pub fn serialize<S: Serializer>(v: &Option<crate::State>, s: S) -> Result<S::Ok, S::Error> {
        v.as_ref().map(|st| Wrap(st.clone())).serialize(s)
    }

    /// Ters serileştir.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<crate::State>, D::Error> {
        Ok(Option::<Wrap>::deserialize(d)?.map(|w| w.0))
    }
}
