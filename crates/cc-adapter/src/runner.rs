//! Runner: bir iş yükünü motora uygular, izden çökme durumları üretir, motoru her durumdan yeniden
//! açar ve kurtarılan durumu modelle karşılaştırır (SPEC_stage4).
//!
//! Akış (tek seed):
//! 1. Boş `workdir/run/` dizininde kayıtlı açılış + işlemler (`apply_op`): geçmiş + iz.
//! 2. Çökmesiz okuma doğrulaması (`check_reads`).
//! 3. Seçilen her kesme noktası için: `crash_point` → `History::crash_view` (o anda hangi işlemler
//!    ACK'lenmişti, hangisi yarıdaydı) ve `crash_states` (diskte ne kalmış olabilir).
//! 4. Her durum `workdir/crash/`'e materyalize edilir, motor kayıtsız açılır, `scan` sonucu
//!    `check_recovered` ile motorun **bildirdiği** garantilere göre yargılanır.
//!
//! İlk bulguda durulur: minimizer (Aşama 6) tek bir karşı örnekle çalışır.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use cc_model::{
    History, HistoryError, Op, State, Verdict, Violation, check_reads, check_recovered,
};
use cc_trace::{Choice, CrashConfig, CrashError, CrashPoint, TraceLog, crash_states, materialize};
use cc_workload::SplitMix64;

use crate::{AdapterError, StorageAdapter, apply_op};

/// Hangi kesme noktaları denenecek.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutSelection {
    /// Tüm kesmeler: `0..=olay sayısı`.
    All,
    /// En fazla `k` kesme: seed'li, tekrarsız, artan. Olay sayısı + 1 ≤ k ise hepsi.
    Sample(usize),
}

/// Runner ayarları.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    /// Çökme durumu üreteci ayarları.
    pub crash: CrashConfig,
    /// Kesme seçimi.
    pub cuts: CutSelection,
}

impl Default for RunConfig {
    fn default() -> Self {
        RunConfig {
            crash: CrashConfig::default(),
            cuts: CutSelection::All,
        }
    }
}

/// Bir bulgunun türü.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// Model ihlali (çökme sonrası ya da çalışma sırasındaki okumada).
    Violation(Violation),
    /// Motor çökme durumundan açılamadı. Doğru bir motor her izinli çökme durumundan açılabilmeli:
    /// açılamayan veritabanı, kaybolmuş veritabanıdır.
    RecoveryFailed(String),
    /// Motor açıldı ama durumunu okuyamadı.
    ScanFailed(String),
    /// Motorun kendi bütünlük kontrolü başarısız.
    IntegrityFailed(String),
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Finding::Violation(v) => write!(f, "{v}"),
            Finding::RecoveryFailed(e) => write!(f, "kurtarma başarısız: {e}"),
            Finding::ScanFailed(e) => write!(f, "durum okunamadı: {e}"),
            Finding::IntegrityFailed(e) => write!(f, "bütünlük kontrolü başarısız: {e}"),
        }
    }
}

/// Bir bulgunun tam bağlamı (Aşama 6 raporu ve replay bunu kullanır).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureCase {
    /// Kesme noktası; `None` = çökmesiz koşudaki okuma ihlali.
    pub cut: Option<usize>,
    /// Kesmenin geçmişe eşlenmesi.
    pub crash_point: Option<CrashPoint>,
    /// Durumu üreten seçim.
    pub choice: Option<Choice>,
    /// Motorun kurtardığı durum (açılabildiyse).
    pub recovered: Option<State>,
    /// Bulgu.
    pub finding: Finding,
}

/// Tek bir iş yükü koşusunun sonucu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedOutcome {
    /// Geçmişe giren işlem sayısı.
    pub ops: usize,
    /// İzdeki olay sayısı.
    pub events: usize,
    /// Denenen kesme sayısı.
    pub cuts: usize,
    /// Denenen çökme durumu sayısı.
    pub states: usize,
    /// İlk bulgu (yoksa None).
    pub failure: Option<FailureCase>,
}

/// Altyapı hatası (bulgu değil): dizin hazırlanamadı, çökmesiz koşu başarısız, iz tutarsız.
#[derive(Debug)]
pub enum RunError {
    /// Dosya sistemi hatası (çalışma dizini).
    Io(io::Error),
    /// Çökmesiz koşuda motor hatası (açılış ya da kapanış).
    Setup(AdapterError),
    /// İz, çökme üretecine göre tutarsız.
    Trace(CrashError),
    /// Geçmiş değişmezi bozuldu (runner hatası).
    History(HistoryError),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Io(e) => write!(f, "çalışma dizini: {e}"),
            RunError::Setup(e) => write!(f, "çökmesiz koşu: {e}"),
            RunError::Trace(e) => write!(f, "iz: {e}"),
            RunError::History(e) => write!(f, "geçmiş: {e}"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<io::Error> for RunError {
    fn from(e: io::Error) -> Self {
        RunError::Io(e)
    }
}

impl From<CrashError> for RunError {
    fn from(e: CrashError) -> Self {
        RunError::Trace(e)
    }
}

/// Dizini silip boş olarak yeniden yaratır.
fn fresh(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fs::create_dir_all(dir)
}

/// Denenecek kesmeler (artan, tekrarsız).
pub fn select_cuts(events: usize, seed: u64, selection: CutSelection) -> Vec<usize> {
    let all = events + 1;
    match selection {
        CutSelection::Sample(k) if k < all => {
            // Kısmi Fisher–Yates: ilk k eleman tekdüze rastgele bir k-alt kümedir.
            let mut v: Vec<usize> = (0..all).collect();
            // Kesme seçiminin seed akışı iş yükününkinden ayrılsın diye sabit bir maske.
            let mut r = SplitMix64(seed ^ 0xC0FF_EE00_C0FF_EE00);
            for i in 0..k {
                let j = i + r.below((all - i) as u64) as usize;
                v.swap(i, j);
            }
            v.truncate(k);
            v.sort_unstable();
            v
        }
        _ => (0..all).collect(),
    }
}

/// Bir iş yükünü koşar ve çökme durumlarını yargılar (ilk bulguda durur).
///
/// `workdir` altında `run/` ve `crash/` alt dizinleri kullanılır (her çağrıda silinip yaratılır).
pub fn run_case<A: StorageAdapter>(
    ops: Vec<Op>,
    config: &A::Config,
    workdir: &Path,
    seed: u64,
    rc: &RunConfig,
) -> Result<SeedOutcome, RunError> {
    let run_dir = workdir.join("run");
    let crash_dir = workdir.join("crash");
    fresh(&run_dir)?;

    // 1. Kayıtlı çökmesiz koşu.
    let log = TraceLog::new();
    let mut adapter = A::open(&run_dir, config, Some(log.clone())).map_err(RunError::Setup)?;
    let mut history = History::new();
    for op in ops {
        match apply_op(&mut adapter, &mut history, Some(&log), op) {
            Ok(_) => {}
            // Başarısız işlem geçmişi kapatır: belirsizlik tek noktada kalsın diye iş yükü durur.
            Err(HistoryError::Closed) => break,
            Err(e) => return Err(RunError::History(e)),
        }
    }
    adapter.close().map_err(RunError::Setup)?;
    let trace = log.snapshot();
    let mut outcome = SeedOutcome {
        ops: history.len(),
        events: trace.events.len(),
        cuts: 0,
        states: 0,
        failure: None,
    };

    // 2. Çalışma sırasındaki okumalar.
    if let Verdict::Violated(v) = check_reads(&history) {
        outcome.failure = Some(FailureCase {
            cut: None,
            crash_point: None,
            choice: None,
            recovered: None,
            finding: Finding::Violation(v),
        });
        return Ok(outcome);
    }

    // 3–4. Çökme durumları.
    let guarantees = A::guarantees(config);
    for cut in select_cuts(trace.events.len(), seed, rc.cuts) {
        outcome.cuts += 1;
        let cp = trace.crash_point(cut);
        let view = history.crash_view(cp.completed, cp.in_flight);
        for state in crash_states(&trace.events, cut, seed, &rc.crash)? {
            outcome.states += 1;
            fresh(&crash_dir)?;
            materialize(&state.image, &crash_dir)?;
            let (recovered, finding) = judge::<A>(&crash_dir, config, &view, guarantees);
            if let Some(finding) = finding {
                outcome.failure = Some(FailureCase {
                    cut: Some(cut),
                    crash_point: Some(cp),
                    choice: Some(state.choice),
                    recovered,
                    finding,
                });
                return Ok(outcome);
            }
        }
    }
    Ok(outcome)
}

/// Materyalize edilmiş bir çökme durumunu açıp yargılar: (kurtarılan durum, bulgu).
fn judge<A: StorageAdapter>(
    dir: &Path,
    config: &A::Config,
    view: &History,
    guarantees: cc_model::Guarantees,
) -> (Option<State>, Option<Finding>) {
    let mut db = match A::open(dir, config, None) {
        Ok(db) => db,
        Err(e) => return (None, Some(Finding::RecoveryFailed(e.to_string()))),
    };
    if let Some(Err(e)) = db.integrity_check() {
        return (None, Some(Finding::IntegrityFailed(e.to_string())));
    }
    let recovered = match db.scan() {
        Ok(s) => s,
        Err(e) => return (None, Some(Finding::ScanFailed(e.to_string()))),
    };
    // Kapanış hatası hükmü etkilemez: durum zaten okundu.
    let _ = db.close();
    let finding = match check_recovered(view, guarantees, &recovered) {
        Verdict::Allowed { .. } => None,
        Verdict::Violated(v) => Some(Finding::Violation(v)),
    };
    (Some(recovered), finding)
}
