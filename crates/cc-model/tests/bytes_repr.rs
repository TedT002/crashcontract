//! Bayt gösterimi testleri (SPEC_stage6 §2): tersinirlik ve bozuk girdi reddi.

use cc_model::bytes_repr::{decode, encode};

/// Tüm 256 bayt ve özel karakterler gidiş-dönüş.
#[test]
fn roundtrip_all_bytes() {
    let all: Vec<u8> = (0..=255).collect();
    assert_eq!(decode(&encode(&all)).unwrap(), all);
    for s in [&b""[..], b"\\", b"\\x41", b"\"", b"k3", b"a b~"] {
        assert_eq!(decode(&encode(s)).unwrap(), s);
    }
    assert_eq!(encode(b"k3"), "k3");
    assert_eq!(encode(b"\\"), "\\\\");
    assert_eq!(encode(&[0, 0xff]), "\\x00\\xff");
}

/// Bozuk kaçışlar ve ASCII olmayan karakterler reddedilir.
#[test]
fn rejects_malformed() {
    for bad in ["\\", "\\x", "\\x4", "\\xzz", "\\q", "ç", "\u{7f}"] {
        assert!(decode(bad).is_err(), "{bad:?} kabul edildi");
    }
}
