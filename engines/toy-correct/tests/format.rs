//! toy-correct bayt formatı testleri.

use cc_adapter::AdapterError;
use cc_model::{State, Write};
use toy_correct::format::{
    MAX_RECORD_PAYLOAD, WalReplay, WalStop, crc32, decode_checkpoint, decode_wal,
    encode_checkpoint, encode_wal_record, parse_wal_file_name,
};

fn b(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}
fn wput(k: &str, v: &str) -> Write {
    Write::Put {
        key: b(k),
        value: b(v),
    }
}
fn wdel(k: &str) -> Write {
    Write::Delete { key: b(k) }
}

/// Y01: standart CRC-32/IEEE test vektörü.
#[test]
fn y01_crc32_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

/// Y02: kayıtlar gidiş-dönüş; boş yazma listesi de geçerli bir kayıttır.
#[test]
fn y02_wal_roundtrip() {
    let recs = [
        vec![wput("a", "1")],
        vec![wdel("a")],
        vec![wput("b", "2"), wdel("c")],
        vec![],
    ];
    let bytes: Vec<u8> = recs
        .iter()
        .enumerate()
        .flat_map(|(i, ws)| encode_wal_record(i as u64 + 1, ws))
        .collect();
    assert_eq!(
        decode_wal(&bytes).unwrap(),
        WalReplay {
            records: recs.to_vec(),
            valid_len: bytes.len() as u64,
            stop: WalStop::CleanEnd
        }
    );
}

/// Y03: ilk bozuk kayıtta dur — her bozulma türü ayrı durma nedeni verir, önceki kayıtlar korunur.
#[test]
fn y03_stop_at_first_bad_record() {
    let r1 = encode_wal_record(1, &[wput("a", "1")]);
    let r2 = encode_wal_record(2, &[wput("b", "2")]);
    let good: Vec<u8> = [r1.clone(), r2.clone()].concat();
    let r3 = encode_wal_record(3, &[wput("c", "3")]);
    let mut too_long = encode_wal_record(3, &[]);
    too_long[8..12].copy_from_slice(&(MAX_RECORD_PAYLOAD + 1).to_le_bytes());
    let mut bad_crc = r3.clone();
    *bad_crc.last_mut().unwrap() ^= 0xFF;
    let cases: Vec<(Vec<u8>, WalStop)> = vec![
        (vec![0xAB; 10], WalStop::TornHeader),
        (
            vec![0; 16],
            WalStop::LsnMismatch {
                expected: 3,
                found: 0,
            },
        ),
        (
            encode_wal_record(5, &[]),
            WalStop::LsnMismatch {
                expected: 3,
                found: 5,
            },
        ),
        (
            too_long,
            WalStop::TooLong {
                len: MAX_RECORD_PAYLOAD + 1,
            },
        ),
        // Başlık tam, payload yarım (kaydın yarısı başlıktan kısa olabilir → o TornHeader olurdu).
        (r3[..r3.len() - 3].to_vec(), WalStop::Truncated),
        (bad_crc, WalStop::BadCrc),
    ];
    for (tail, stop) in cases {
        let bytes = [good.clone(), tail].concat();
        let got = decode_wal(&bytes).unwrap();
        assert_eq!(got.stop, stop);
        assert_eq!(
            got.records,
            vec![vec![wput("a", "1")], vec![wput("b", "2")]]
        );
        assert_eq!(got.valid_len, (r1.len() + r2.len()) as u64);
    }
}

/// Y04: checkpoint gidiş-dönüş ve bozulma tespiti; WAL dosya adı ayrıştırma.
#[test]
fn y04_checkpoint_and_names() {
    let s = State::from([(b("a"), b("1")), (b("b"), b("2"))]);
    let cp = encode_checkpoint(7, &s);
    assert_eq!(decode_checkpoint(&cp).unwrap(), (7, s));

    let mut flipped = cp.clone();
    flipped[20] ^= 1;
    let mut extended = cp.clone();
    extended.push(0);
    for bad in [flipped, extended, cp[..27].to_vec()] {
        assert!(matches!(
            decode_checkpoint(&bad),
            Err(AdapterError::Corruption(_))
        ));
    }

    assert_eq!(parse_wal_file_name("wal-0000000000000001"), Some(1));
    assert_eq!(parse_wal_file_name("wal-1"), None);
    assert_eq!(parse_wal_file_name("wal-00000000000000AB"), None);
}
