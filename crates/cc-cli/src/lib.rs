//! # cc-cli kütüphanesi — motor seçimi, JSON rapor, küçültme ve replay
//!
//! İkili (`crashcontract`) bunun üzerine ince bir argüman katmanıdır; testler doğrudan bu
//! fonksiyonları çağırır.
//!
//! ## Rapor neden kendi başına yeterli olmalı?
//!
//! Hedef: tek komutla yeniden oynatılabilen bir rapor. Rapor; motor + yapılandırma, seed,
//! küçültülmüş işlem dizisi, çökme seçimi (`cut` + `choice`), kurtarılan durum, tam geçmiş ve izi
//! taşır. `replay` yalnız bu dosyadan aynı koşuyu yapar ve `failure` alanını **bire bir**
//! karşılaştırır: iz, geçmiş, kesme, seçim, kurtarılan durum ve bulgu aynı olmalı.

use std::fs;
use std::path::Path;

use cc_adapter::StorageAdapter;
use cc_adapter::runner::{FailureCase, Finding, RunConfig, RunError, SeedOutcome, run_case};
use cc_minimize::{Stats, minimize};
use cc_model::{Guarantees, Op};
use cc_workload::{WorkloadConfig, generate};
use redb_engine::{REDB_VERSION, RedbConfig, RedbProc};
use toy_correct::{ToyConfig, ToyDb};

/// Rapor şeması sürümü. Alan anlamı, seed → iş yükü eşlemesi ya da çökme durumu üretimi
/// değişirse artırılır; `replay` farklı sürümü reddeder (sessizce farklı sonuç üretmesin).
pub const SCHEMA_VERSION: u32 = 1;

/// Test edilen motor ve yapılandırması.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "name", content = "config", rename_all = "kebab-case")]
pub enum EngineSpec {
    /// WAL + checkpoint'li toy motor (mutasyon yapılandırmada).
    Toy(ToyConfig),
    /// redb (gerçek motor; ayrı süreç + LD_PRELOAD shim).
    Redb(RedbConfig),
}

impl EngineSpec {
    /// İnsan için ad: `toy-correct` ya da `toy-mutant:<ad>`.
    pub fn label(&self) -> String {
        match self {
            EngineSpec::Toy(c) => match c.mutation {
                None => "toy-correct".into(),
                Some(m) => format!("toy-mutant:{}", m.name()),
            },
            EngineSpec::Redb(_) => "redb".into(),
        }
    }

    /// Motorun sürümü (rapora girer).
    pub fn version(&self) -> String {
        match self {
            EngineSpec::Toy(_) => format!("toy (crashcontract {})", env!("CARGO_PKG_VERSION")),
            EngineSpec::Redb(_) => format!("redb {REDB_VERSION}"),
        }
    }

    /// Motorun bildirdiği garantiler.
    pub fn guarantees(&self) -> Guarantees {
        match self {
            EngineSpec::Toy(c) => ToyDb::guarantees(c),
            EngineSpec::Redb(c) => RedbProc::guarantees(c),
        }
    }

    /// Bir iş yükünü bu motorla koşar.
    pub fn run(
        &self,
        ops: Vec<Op>,
        workdir: &Path,
        seed: u64,
        rc: &RunConfig,
    ) -> Result<SeedOutcome, RunError> {
        match self {
            EngineSpec::Toy(c) => run_case::<ToyDb>(ops, c, workdir, seed, rc),
            EngineSpec::Redb(c) => run_case::<RedbProc>(ops, c, workdir, seed, rc),
        }
    }

    fn minimize(
        &self,
        ops: Vec<Op>,
        workdir: &Path,
        seed: u64,
        rc: &RunConfig,
        target: &Finding,
    ) -> Result<cc_minimize::Minimized, RunError> {
        match self {
            EngineSpec::Toy(c) => minimize::<ToyDb>(ops, c, workdir, seed, rc, target),
            EngineSpec::Redb(c) => minimize::<RedbProc>(ops, c, workdir, seed, rc, target),
        }
    }
}

/// Yeniden oynatılabilir hata raporu (`failures/*.json`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Report {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Raporu üreten araç sürümü.
    pub tool_version: String,
    /// Motor + yapılandırma.
    pub engine: EngineSpec,
    /// Motor sürümü (eski raporlarda yok → boş).
    #[serde(default)]
    pub engine_version: String,
    /// Motorun bildirdiği garantiler (hüküm bunlara göre verildi).
    pub guarantees: Guarantees,
    /// Kaynak seed (iş yükü üretimi ve çökme durumu örneklemesi).
    pub seed: u64,
    /// İş yükü üreteci yapılandırması (`ops`'un kaynağı).
    pub workload: WorkloadConfig,
    /// Runner yapılandırması.
    pub run: RunConfig,
    /// Koşulan işlem dizisi (küçültüldüyse küçültülmüş hali).
    pub ops: Vec<Op>,
    /// Küçültme istatistikleri; küçültülmediyse `None`.
    pub minimization: Option<Stats>,
    /// Bulgu ve tam bağlamı.
    pub failure: FailureCase,
    /// Bulgunun insan için tek satırlık açıklaması (replay karşılaştırmaz).
    pub finding_text: String,
}

/// Seed'i koşar; bulgu varsa (istenirse küçültülmüş) raporu döndürür.
pub fn report_for_seed(
    engine: &EngineSpec,
    seed: u64,
    workload: &WorkloadConfig,
    rc: &RunConfig,
    workdir: &Path,
    do_minimize: bool,
) -> Result<Option<Report>, RunError> {
    let ops = generate(seed, workload);
    let outcome = engine.run(ops.clone(), workdir, seed, rc)?;
    let Some(failure) = outcome.failure else {
        return Ok(None);
    };
    let (ops, failure, minimization) = if do_minimize {
        let m = engine.minimize(ops, workdir, seed, rc, &failure.finding)?;
        (m.ops, m.failure, Some(m.stats))
    } else {
        (ops, failure, None)
    };
    Ok(Some(Report {
        schema_version: SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").into(),
        engine: engine.clone(),
        engine_version: engine.version(),
        guarantees: engine.guarantees(),
        seed,
        workload: workload.clone(),
        run: rc.clone(),
        ops,
        minimization,
        finding_text: failure.finding.to_string(),
        failure,
    }))
}

/// Replay sonucu.
#[derive(Debug)]
pub struct Replayed {
    /// Yeniden koşunun bulgusu (yoksa None).
    pub failure: Option<FailureCase>,
    /// Rapordaki `failure` ile bire bir aynı mı?
    pub identical: bool,
}

/// Rapordaki koşuyu yeniden yapar ve sonucu karşılaştırır.
pub fn replay(report: &Report, workdir: &Path) -> Result<Replayed, String> {
    if report.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "şema sürümü {} desteklenmiyor (beklenen {SCHEMA_VERSION})",
            report.schema_version
        ));
    }
    if report.guarantees != report.engine.guarantees() {
        return Err("rapordaki garantiler motorun bugünkü bildirimiyle uyuşmuyor".into());
    }
    let o = report
        .engine
        .run(report.ops.clone(), workdir, report.seed, &report.run)
        .map_err(|e| e.to_string())?;
    let identical = o.failure.as_ref() == Some(&report.failure);
    Ok(Replayed {
        failure: o.failure,
        identical,
    })
}

/// Raporu JSON olarak yazar (dizini gerekirse yaratır).
pub fn write_report(path: &Path, report: &Report) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    fs::write(path, json + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

/// JSON raporu okur.
pub fn read_report(path: &Path) -> Result<Report, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: geçersiz rapor: {e}", path.display()))
}

/// Raporun varsayılan dosya adı.
pub fn report_file_name(report: &Report) -> String {
    format!(
        "{}-seed{}.json",
        report.engine.label().replace(':', "-"),
        report.seed
    )
}
