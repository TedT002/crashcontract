//! CRC-32/IEEE (yansıtılmış polinom 0xEDB88320, init/xorout 0xFFFFFFFF).
//!
//! Neden elle? ~25 satır için dış crate eklemek bağımlılık-minimum kuralına aykırı. Tablo derleme
//! zamanında (`const fn`) üretilir.

const POLY: u32 = 0xEDB8_8320;

const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 { POLY ^ (c >> 1) } else { c >> 1 };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// `data` üzerindeki CRC-32/IEEE. `crc32(b"123456789") == 0xCBF43926`.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_parts(&[data])
}

/// Birden çok parçanın art arda eklenmiş hali üzerinde CRC (kopyalamadan).
pub(crate) fn crc32_parts(parts: &[&[u8]]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for part in parts {
        for &b in *part {
            c = TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xFFFF_FFFF
}
