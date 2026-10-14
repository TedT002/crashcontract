//! # redb-engine — gerçek motor adaptörü
//!
//! redb, ayrı bir sürücü sürecinde (`redb-driver`) çalışır; adaptör onunla satır başına JSON konuşur
//! ([`protocol`]). Kayıtlı açılışta sürücü `LD_PRELOAD=libcc_shim.so` ile başlatılır: shim, redb'nin
//! veri dizinindeki dosya sistemi çağrılarını bir iz dosyasına yazar; adaptör her yanıttan sonra yeni
//! satırları okuyup `TraceLog`'a ekler. Böylece `apply_op`'un çizdiği span'ler, o işlem sırasında
//! gerçekleşen olayları tam olarak kapsar.
//!
//! ## Neden ayrı süreç?
//! (1) LD_PRELOAD bütün süreci etkiler: runner'ın kendi G/Ç'si ize karışmamalı. (2) Çökme durumundan
//! yeniden açılış, önceki koşunun hiçbir süreç içi durumunu (sayfa önbelleği dışında — o da zaten
//! materyalize edilen görüntüdür) görmemeli: her açılış yeni bir süreçtir.
//!
//! ## Bildirilen garantiler (iddia — ölçülerek sınandı, bkz. README)
//! Her işlem tek bir redb yazma işlemi ve `Durability::Immediate` (varsayılan) ile commit edilir:
//! `OnAck`; txn atomik; durum her zaman bir prefix.

pub mod protocol;

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use cc_adapter::{AdapterError, StorageAdapter};
use cc_model::{Durability, Guarantees, Op, State, Value, Write};
use cc_trace::TraceLog;
use cc_trace::shim_format::{ShimLine, decode};
use protocol::{Request, Response};

/// Ölçülen ve test edilen redb sürümü (Cargo.lock ile eşitliği testte doğrulanır).
pub const REDB_VERSION: &str = "4.3.0";

/// redb adaptörü yapılandırması.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RedbConfig {
    /// Veri dizinindeki veritabanı dosyasının adı.
    pub db_file: String,
}

impl Default for RedbConfig {
    fn default() -> Self {
        RedbConfig {
            db_file: "data.redb".into(),
        }
    }
}

fn io_err(msg: impl Into<String>) -> AdapterError {
    AdapterError::Io(std::io::Error::other(msg.into()))
}

/// Yardımcı ikiliyi bulur: önce ortam değişkeni, sonra çalışan ikilinin dizini ve bir üstü
/// (`target/<profil>/` ve testler için `target/<profil>/deps/`).
pub fn locate(env: &str, file: &str) -> Result<PathBuf, AdapterError> {
    if let Some(p) = std::env::var_os(env) {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe()?;
    let mut dir = exe.parent();
    for _ in 0..2 {
        let Some(d) = dir else { break };
        let candidate = d.join(file);
        if candidate.exists() {
            return Ok(candidate);
        }
        dir = d.parent();
    }
    Err(io_err(format!(
        "{file} bulunamadı ({env} ayarlı değil). Önce: cargo build -p cc-shim -p redb-engine \
         (aynı profille)"
    )))
}

/// Kayıtlı koşunun iz dosyası okuyucusu: shim'in eklediği satırları artımlı okur.
struct TraceTail {
    file: File,
    pending: Vec<u8>,
    log: TraceLog,
}

impl TraceTail {
    /// Yeni tam satırları çözer ve günlüğe ekler.
    ///
    /// # Panics
    /// Shim `unsupported` yazdıysa ya da satır çözülemiyorsa: iz eksik/bozuk demektir; devam etmek
    /// sessizce yanlış çökme durumları üretirdi. Bu bir bulgu değil, kapsam/altyapı ihlalidir.
    fn ingest(&mut self) {
        let mut buf = Vec::new();
        self.file
            .read_to_end(&mut buf)
            .expect("iz dosyası okunamadı");
        self.pending.extend_from_slice(&buf);
        while let Some(nl) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=nl).collect();
            let line = std::str::from_utf8(&line[..line.len() - 1]).expect("iz satırı UTF-8 değil");
            match decode(line) {
                Ok(ShimLine::Event(e)) => self.log.push(e),
                Ok(ShimLine::Unsupported { call, detail }) => panic!(
                    "cc-shim: motor modellenmeyen bir çağrı yaptı: {call} {detail} — iz eksik \
                     olurdu; bu motor/yapılandırma kapsam dışı"
                ),
                Err(e) => panic!("cc-shim: çözülemeyen iz satırı: {e}"),
            }
        }
    }
}

/// Ayrı süreçte çalışan redb.
pub struct RedbProc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    tail: Option<TraceTail>,
}

impl std::fmt::Debug for RedbProc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedbProc")
            .field("pid", &self.child.id())
            .field("recording", &self.tail.is_some())
            .finish_non_exhaustive()
    }
}

impl RedbProc {
    fn read_response(&mut self) -> Result<Response, AdapterError> {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line)?;
        if n == 0 {
            let mut err = String::new();
            if let Some(e) = self.child.stderr.as_mut() {
                let _ = e.read_to_string(&mut err);
            }
            return Err(io_err(format!(
                "sürücü beklenmedik şekilde çıktı: {}",
                err.trim()
            )));
        }
        let resp: Response = serde_json::from_str(line.trim_end())
            .map_err(|e| io_err(format!("geçersiz sürücü yanıtı: {e}")))?;
        // Yanıt geldiğinde işlemin tüm G/Ç'si yapılmıştır (sürücü tek iş parçacıklı ve yanıtı işlem
        // bittikten sonra yazar; shim her olayı çağrı dönmeden yazar).
        if let Some(t) = self.tail.as_mut() {
            t.ingest();
        }
        Ok(resp)
    }

    fn call(&mut self, req: &Request) -> Result<Response, AdapterError> {
        let line = serde_json::to_string(req).map_err(|e| io_err(e.to_string()))?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;
        match self.read_response()? {
            Response::Err { message } => Err(io_err(message)),
            r => Ok(r),
        }
    }

    fn write(&mut self, op: Op) -> Result<(), AdapterError> {
        match self.call(&Request::Op { op })? {
            Response::Ok => Ok(()),
            r => Err(io_err(format!("beklenmeyen yanıt: {r:?}"))),
        }
    }
}

impl Drop for RedbProc {
    fn drop(&mut self) {
        // Kapanış yapılmadıysa (hata yolu) süreci sızdırma.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl StorageAdapter for RedbProc {
    type Config = RedbConfig;

    fn guarantees(_: &RedbConfig) -> Guarantees {
        Guarantees {
            durability: Durability::OnAck,
            atomic_txn: true,
            prefix_order: true,
        }
    }

    fn open(dir: &Path, config: &RedbConfig, log: Option<TraceLog>) -> Result<Self, AdapterError> {
        let dir = fs::canonicalize(dir)?;
        let driver = locate("CC_REDB_DRIVER", "redb-driver")?;
        let mut cmd = Command::new(driver);
        cmd.arg(dir.join(&config.db_file))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("LD_PRELOAD")
            .env_remove("CC_SHIM_DIR")
            .env_remove("CC_SHIM_TRACE");
        let mut tail = None;
        if let Some(log) = log {
            // İz boş dizinde başlar: başlangıç görüntüsü = boş dizin.
            if fs::read_dir(&dir)?.next().is_some() {
                return Err(AdapterError::Io(std::io::Error::new(
                    std::io::ErrorKind::DirectoryNotEmpty,
                    "kayıtlı açılış boş dizin ister",
                )));
            }
            let trace = dir.with_extension("cctrace");
            // Dosyayı sürücüden ÖNCE yarat: shim O_TRUNC|O_APPEND ile aynı inode'u açar, okuyucu
            // baştan okur (yarış yok).
            File::create(&trace)?;
            let shim = locate("CC_SHIM_LIB", "libcc_shim.so")?;
            cmd.env("LD_PRELOAD", shim)
                .env("CC_SHIM_DIR", &dir)
                .env("CC_SHIM_TRACE", &trace);
            let mut file = File::open(&trace)?;
            file.seek(SeekFrom::Start(0))?;
            tail = Some(TraceTail {
                file,
                pending: Vec::new(),
                log,
            });
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("stdin borulandı");
        let stdout = BufReader::new(child.stdout.take().expect("stdout borulandı"));
        let mut proc = RedbProc {
            child,
            stdin,
            stdout,
            tail,
        };
        match proc.read_response()? {
            Response::Ready => Ok(proc),
            // Açılış (kurtarma) reddedildi: runner bunu RecoveryFailed olarak yargılar.
            Response::Err { message } => Err(AdapterError::Corruption(message)),
            r => Err(io_err(format!("beklenmeyen açılış yanıtı: {r:?}"))),
        }
    }

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), AdapterError> {
        self.write(Op::Put {
            key: key.to_vec(),
            value: value.to_vec(),
        })
    }

    fn get(&mut self, key: &[u8]) -> Result<Option<Value>, AdapterError> {
        match self.call(&Request::Op {
            op: Op::Get { key: key.to_vec() },
        })? {
            Response::Value { value } => Ok(value),
            r => Err(io_err(format!("beklenmeyen yanıt: {r:?}"))),
        }
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), AdapterError> {
        self.write(Op::Delete { key: key.to_vec() })
    }

    fn txn(&mut self, writes: &[Write]) -> Result<(), AdapterError> {
        self.write(Op::Txn {
            writes: writes.to_vec(),
        })
    }

    fn sync(&mut self) -> Result<(), AdapterError> {
        self.write(Op::Sync)
    }

    fn scan(&mut self) -> Result<State, AdapterError> {
        match self.call(&Request::Scan)? {
            Response::State { state } => Ok(state),
            r => Err(io_err(format!("beklenmeyen yanıt: {r:?}"))),
        }
    }

    fn close(mut self) -> Result<(), AdapterError> {
        self.call(&Request::Close)?;
        self.child.wait()?;
        Ok(())
    }
}
