//! Anahtar-değer tipleri ve işlemler (`Op`). Model için "adım" kavramı da burada tanımlıdır.

use std::collections::BTreeMap;

/// Anahtar: ham bayt dizisi. Sıralama bayt-sözlük (lexicographic) sırasıdır;
/// tüm "ilk anahtar" kuralları bu sıraya göredir.
pub type Key = Vec<u8>;

/// Değer: ham bayt dizisi.
pub type Value = Vec<u8>;

/// Bir anahtar-değer deposunun mantıksal içeriği. Silinmiş anahtar haritada YOKTUR
/// (tombstone burada temsil edilmez; tombstone motorun iç ayrıntısıdır).
///
/// `BTreeMap` seçildi: yineleme sırası belirlidir, bu da hükmün (hangi anahtarın raporlandığı)
/// aynı girdiyle her zaman aynı çıkmasını sağlar (determinizm).
pub type State = BTreeMap<Key, Value>;

/// Bir işlem (txn) içindeki tek yazma.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// Anahtara değer yaz (varsa üzerine yaz).
    Put {
        /// Yazılan anahtar.
        key: Key,
        /// Yazılan değer.
        value: Value,
    },
    /// Anahtarı sil (yoksa etkisiz).
    Delete {
        /// Silinen anahtar.
        key: Key,
    },
}

/// İş yükünün motora gönderdiği tek işlem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Tek yazma; ACK'i motorun Durability bildirimine göre dayanıklılık vaat eder/etmez.
    Put {
        /// Yazılan anahtar.
        key: Key,
        /// Yazılan değer.
        value: Value,
    },
    /// Tek silme.
    Delete {
        /// Silinen anahtar.
        key: Key,
    },
    /// Okuma; durumu değiştirmez. Sonucu `Outcome::Read` ile kaydedilir.
    Get {
        /// Okunan anahtar.
        key: Key,
    },
    /// begin…commit arası yazmalar; ACK = commit'in dönmesi. Txn içinde Get yoktur (v1 kapsamı).
    /// Boş `writes` geçerlidir (etkisiz).
    Txn {
        /// Sırayla uygulanan yazmalar.
        writes: Vec<Write>,
    },
    /// Açık dayanıklılık bariyeri (motorun sync/flush çağrısı).
    Sync,
}

/// Tek bir durum değiştiren adım: (anahtar, yeni değer); `None` = silme.
pub(crate) type Step<'a> = (&'a Key, Option<&'a Value>);

/// Bir işlemin adımları: `Put`/`Delete` = 1, `Txn` = yazma sayısı kadar, `Get`/`Sync` = 0.
///
/// Neden "adım"? Atomik olmayan bir Txn'de çökme, yazmaların ortasında olabilir; kesme noktaları
/// işlem değil adım granülaritesinde tanımlanır.
pub(crate) fn steps_of(op: &Op) -> Vec<Step<'_>> {
    match op {
        Op::Put { key, value } => vec![(key, Some(value))],
        Op::Delete { key } => vec![(key, None)],
        Op::Txn { writes } => writes
            .iter()
            .map(|w| match w {
                Write::Put { key, value } => (key, Some(value)),
                Write::Delete { key } => (key, None),
            })
            .collect(),
        Op::Get { .. } | Op::Sync => Vec::new(),
    }
}

/// Bir adımı duruma uygular (`None` → anahtarı kaldır).
pub(crate) fn apply_step(state: &mut State, key: &Key, value: Option<&Value>) {
    match value {
        Some(v) => {
            state.insert(key.clone(), v.clone());
        }
        None => {
            state.remove(key);
        }
    }
}
