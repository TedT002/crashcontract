//! Shim iz satırı formatı: `cc-shim` (LD_PRELOAD) yazar, motor adaptörü okur.
//!
//! Neden JSON değil de sade metin? Shim, başka bir sürecin libc çağrılarının ortasında çalışır;
//! orada mümkün olduğunca az iş yapılmalı. Her olay tek satırdır ve tek `write` ile eklenir; adlar ve
//! veri onaltılık (boşluk/satır sonu içeremez). Tek kaynak: shim de okuyucu da bu modülü kullanır.
//!
//! ```text
//! open <ad-hex> <dosya> <0|1>      write <dosya> <ofset> <veri-hex>     setlen <dosya> <uzunluk>
//! fsync <dosya>   fdatasync <dosya>   rename <ad-hex> <ad-hex>   unlink <ad-hex>   syncdir
//! unsupported <çağrı> <ayrıntı-hex>
//! ```

use crate::{FileId, TraceEvent};

/// Shim'in yazdığı bir satır.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShimLine {
    /// Modellenen bir olay.
    Event(TraceEvent),
    /// İzlenen bir dosyaya modellenmeyen bir çağrı yapıldı (dup, mmap, writev, fallocate…).
    /// Sessizce yok sayılamaz: iz eksik olur ve çökme durumları yanlış üretilir.
    Unsupported {
        /// Çağrının adı.
        call: String,
        /// Ayrıntı.
        detail: String,
    },
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("tek uzunluklu onaltılık".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| "geçersiz onaltılık".to_string())
        })
        .collect()
}

fn name(s: &str) -> Result<String, String> {
    String::from_utf8(unhex(s)?).map_err(|_| "UTF-8 olmayan ad".to_string())
}

/// Bir olayı satıra çevirir (sonunda `\n`).
pub fn encode(event: &TraceEvent) -> String {
    let body = match event {
        TraceEvent::Open {
            name,
            file,
            created,
        } => format!(
            "open {} {} {}",
            hex(name.as_bytes()),
            file.0,
            u8::from(*created)
        ),
        TraceEvent::Write { file, offset, data } => {
            format!("write {} {offset} {}", file.0, hex(data))
        }
        TraceEvent::SetLen { file, len } => format!("setlen {} {len}", file.0),
        TraceEvent::Fsync { file } => format!("fsync {}", file.0),
        TraceEvent::Fdatasync { file } => format!("fdatasync {}", file.0),
        TraceEvent::Rename { from, to } => {
            format!("rename {} {}", hex(from.as_bytes()), hex(to.as_bytes()))
        }
        TraceEvent::Unlink { name } => format!("unlink {}", hex(name.as_bytes())),
        TraceEvent::SyncDir => "syncdir".into(),
    };
    body + "\n"
}

/// Modellenmeyen çağrı satırı.
pub fn encode_unsupported(call: &str, detail: &str) -> String {
    format!("unsupported {call} {}\n", hex(detail.as_bytes()))
}

/// Tek satırı (sonundaki `\n` olmadan) çözer.
pub fn decode(line: &str) -> Result<ShimLine, String> {
    let f: Vec<&str> = line.split(' ').collect();
    let num = |s: &str| {
        s.parse::<u64>()
            .map_err(|_| format!("sayı bekleniyordu: {s}"))
    };
    let file = |s: &str| {
        s.parse::<u32>()
            .map(FileId)
            .map_err(|_| format!("dosya kimliği bekleniyordu: {s}"))
    };
    let ev = match f.as_slice() {
        ["open", n, id, c] => TraceEvent::Open {
            name: name(n)?,
            file: file(id)?,
            created: match *c {
                "1" => true,
                "0" => false,
                _ => return Err(format!("created 0/1 bekleniyordu: {c}")),
            },
        },
        ["write", id, off, data] => TraceEvent::Write {
            file: file(id)?,
            offset: num(off)?,
            data: unhex(data)?,
        },
        ["setlen", id, len] => TraceEvent::SetLen {
            file: file(id)?,
            len: num(len)?,
        },
        ["fsync", id] => TraceEvent::Fsync { file: file(id)? },
        ["fdatasync", id] => TraceEvent::Fdatasync { file: file(id)? },
        ["rename", a, b] => TraceEvent::Rename {
            from: name(a)?,
            to: name(b)?,
        },
        ["unlink", n] => TraceEvent::Unlink { name: name(n)? },
        ["syncdir"] => TraceEvent::SyncDir,
        ["unsupported", call, detail] => {
            return Ok(ShimLine::Unsupported {
                call: call.to_string(),
                detail: String::from_utf8_lossy(&unhex(detail)?).into_owned(),
            });
        }
        _ => return Err(format!("tanınmayan shim satırı: {line:?}")),
    };
    Ok(ShimLine::Event(ev))
}
