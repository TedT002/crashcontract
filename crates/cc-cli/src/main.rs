//! # crashcontract — komut satırı
//!
//! `crashcontract run`: seed aralığındaki her iş yükünü motora uygular, izden çökme durumları
//! üretir ve motoru her durumdan yeniden açıp modelle yargılar (SPEC_stage4 §2).
//!
//! Argüman ayrıştırma elle yazıldı (`clap` yok: bağımlılık minimum). Paralellik
//! `std::thread::scope` ile: her iş parçacığı kendi çalışma dizininde, birbirinden bağımsız seed'ler
//! koşar — iş yükü yine tek iş parçacıklıdır (karar 5), paralel olan yalnız seed'lerdir.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use cc_adapter::StorageAdapter;
use cc_adapter::runner::{
    CutSelection, FailureCase, Finding, RunConfig, RunError, SeedOutcome, run_case,
};
use cc_cli::{EngineSpec, read_report, replay, report_file_name, report_for_seed, write_report};
use cc_trace::CrashConfig;
use cc_workload::{WorkloadConfig, generate};
use toy_correct::{Mutation, SyncPolicy, ToyConfig, ToyDb};

const USAGE: &str = "\
kullanım: crashcontract run [seçenekler]
         crashcontract minimize --seed N [seçenekler] [--out DOSYA]
         crashcontract replay RAPOR.json
         crashcontract mutants [--seeds A..B]

run — seed aralığını koşar, çökme durumlarını yargılar:
  --engine toy-correct|toy-mutant:<ad>
                                  test edilen motor (varsayılan: toy-correct)
  --seeds A..B                    seed aralığı, B hariç (varsayılan: 0..100)
  --policy every-commit|on-sync|mixed
                                  toy senkronizasyon politikası (varsayılan: mixed = seed%2)
  --checkpoint-every N|mixed      toy checkpoint aralığı (varsayılan: mixed = 0 ya da 7)
  --cuts all|N                    kesme seçimi: hepsi ya da seed başına N örnek (varsayılan: all)
  --torn                          sektör seviyesinde yırtık yazmaları da dene
  --jobs N                        paralel iş parçacığı (varsayılan: çekirdek sayısı)
  --workdir DIR                   çalışma dizini (varsayılan: geçici dizin)
  --report-dir DIR                bulgu raporlarının dizini (varsayılan: failures)
  --no-minimize                   raporu küçültmeden yaz
  bulgu varsa ilk bulgulu seed otomatik küçültülür ve JSON rapor yazılır.

minimize — tek seed'i (--seed N) koşar; bulgu varsa küçültüp raporu yazar (--out ya da --report-dir).

replay — rapordaki koşuyu yeniden yapar; sonuç rapordakiyle bire bir aynıysa çıkış 0, değilse 1.

mutants — her mutantı aralıktaki her seed'de koşar (tüm kesmeler + yırtık yazma), yakalama
  oranını basar (varsayılan seed 0..100).
  çıkış kodu 0 yalnız TÜM mutantlar yakalandıysa.

mutantlar: skip-commit-fsync, unlink-before-dir-sync, drop-last-record, ignore-tombstones,
           no-fsync-before-rename, accept-corrupt-record, split-txn-records

çıkış kodu: 0 bulgu yok, 1 bulgu var, 2 kullanım/altyapı hatası";

#[derive(Debug, Clone, Copy)]
enum Mixed<T> {
    Fixed(T),
    Mixed,
}

#[derive(Debug)]
struct RunArgs {
    engine: String,
    /// `toy-mutant:<ad>` ise mutasyon.
    mutation: Option<Mutation>,
    seeds: std::ops::Range<u64>,
    policy: Mixed<SyncPolicy>,
    checkpoint_every: Mixed<u32>,
    cuts: CutSelection,
    torn: bool,
    jobs: usize,
    workdir: Option<PathBuf>,
    report_dir: PathBuf,
    minimize: bool,
    out: Option<PathBuf>,
}

fn parse_num<T: std::str::FromStr>(flag: &str, v: &str) -> Result<T, String> {
    v.parse()
        .map_err(|_| format!("{flag}: sayı bekleniyordu, '{v}' geldi"))
}

fn parse_run(args: &[String]) -> Result<RunArgs, String> {
    let mut a = RunArgs {
        engine: "toy-correct".into(),
        mutation: None,
        seeds: 0..100,
        policy: Mixed::Mixed,
        checkpoint_every: Mixed::Mixed,
        cuts: CutSelection::All,
        torn: false,
        jobs: std::thread::available_parallelism().map_or(1, |n| n.get()),
        workdir: None,
        report_dir: PathBuf::from("failures"),
        minimize: true,
        out: None,
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--torn" => {
                a.torn = true;
                continue;
            }
            "--no-minimize" => {
                a.minimize = false;
                continue;
            }
            _ => {}
        }
        let v = it
            .next()
            .ok_or_else(|| format!("{flag}: değer eksik"))?
            .as_str();
        match flag.as_str() {
            "--engine" => a.engine = v.to_string(),
            "--seeds" => {
                let (lo, hi) = v
                    .split_once("..")
                    .ok_or_else(|| format!("--seeds: A..B bekleniyordu, '{v}' geldi"))?;
                a.seeds = parse_num(flag, lo)?..parse_num(flag, hi)?;
                if a.seeds.is_empty() {
                    return Err("--seeds: boş aralık".into());
                }
            }
            "--policy" => {
                a.policy = match v {
                    "every-commit" => Mixed::Fixed(SyncPolicy::EveryCommit),
                    "on-sync" => Mixed::Fixed(SyncPolicy::OnSyncOnly),
                    "mixed" => Mixed::Mixed,
                    _ => return Err(format!("--policy: bilinmeyen '{v}'")),
                }
            }
            "--checkpoint-every" => {
                a.checkpoint_every = match v {
                    "mixed" => Mixed::Mixed,
                    n => Mixed::Fixed(parse_num(flag, n)?),
                }
            }
            "--cuts" => {
                a.cuts = match v {
                    "all" => CutSelection::All,
                    n => CutSelection::Sample(parse_num(flag, n)?),
                }
            }
            "--jobs" => a.jobs = parse_num::<usize>(flag, v)?.max(1),
            "--workdir" => a.workdir = Some(PathBuf::from(v)),
            "--report-dir" => a.report_dir = PathBuf::from(v),
            "--out" => a.out = Some(PathBuf::from(v)),
            "--seed" => {
                let n: u64 = parse_num(flag, v)?;
                a.seeds = n..n + 1;
            }
            _ => return Err(format!("bilinmeyen seçenek '{flag}'")),
        }
    }
    a.mutation = match a.engine.strip_prefix("toy-mutant:") {
        Some(name) => {
            Some(Mutation::from_name(name).ok_or_else(|| format!("bilinmeyen mutant '{name}'"))?)
        }
        None if a.engine == "toy-correct" => None,
        None => return Err(format!("bilinmeyen motor '{}'", a.engine)),
    };
    Ok(a)
}

/// Seed → toy yapılandırması (mixed: politika seed%2, checkpoint (seed/2)%2 ile 0 ya da 7).
fn toy_config(a: &RunArgs, seed: u64) -> ToyConfig {
    ToyConfig {
        sync_policy: match a.policy {
            Mixed::Fixed(p) => p,
            Mixed::Mixed if seed.is_multiple_of(2) => SyncPolicy::EveryCommit,
            Mixed::Mixed => SyncPolicy::OnSyncOnly,
        },
        checkpoint_every: match a.checkpoint_every {
            Mixed::Fixed(n) => n,
            Mixed::Mixed if (seed / 2).is_multiple_of(2) => 0,
            Mixed::Mixed => 7,
        },
        mutation: a.mutation,
    }
}

fn run_config(a: &RunArgs) -> RunConfig {
    RunConfig {
        crash: CrashConfig {
            torn_writes: a.torn,
            ..CrashConfig::default()
        },
        cuts: a.cuts,
    }
}

fn default_workdir(a: &RunArgs, tag: &str) -> PathBuf {
    a.workdir.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("crashcontract-{tag}-{}", std::process::id()))
    })
}

/// Seed'in raporunu üretir (istenirse küçültür) ve yazar; yazılan yolu döndürür.
fn write_seed_report(a: &RunArgs, seed: u64, workdir: &Path) -> Result<Option<PathBuf>, String> {
    let engine = EngineSpec::Toy(toy_config(a, seed));
    let started = Instant::now();
    let report = report_for_seed(
        &engine,
        seed,
        &WorkloadConfig::default(),
        &run_config(a),
        &workdir.join("report"),
        a.minimize,
    )
    .map_err(|e| format!("seed {seed}: rapor üretilemedi: {e}"))?;
    let Some(report) = report else {
        return Ok(None);
    };
    let path = a
        .out
        .clone()
        .unwrap_or_else(|| a.report_dir.join(report_file_name(&report)));
    write_report(&path, &report)?;
    if let Some(m) = &report.minimization {
        println!(
            "küçültme: {} → {} işlem, {} → {} anahtar, {} → {} değer baytı ({} deneme, {:.1} sn)",
            m.original_ops,
            m.minimized_ops,
            m.original_keys,
            m.minimized_keys,
            m.original_value_bytes,
            m.minimized_value_bytes,
            m.tests_run,
            started.elapsed().as_secs_f64()
        );
    }
    println!("bulgu: {}", report.finding_text);
    println!("rapor: {}", path.display());
    println!("yeniden oynat: crashcontract replay {}", path.display());
    Ok(Some(path))
}

type SeedResult<C> = (u64, C, Result<SeedOutcome, RunError>);

/// Seed'leri `jobs` iş parçacığına (seed % jobs) dağıtır; sonuçlar seed sırasıyla döner.
fn run_all<A: StorageAdapter>(
    a: &RunArgs,
    workdir: &Path,
    config_for: impl Fn(u64) -> A::Config + Sync,
) -> Vec<SeedResult<A::Config>>
where
    A::Config: Send,
{
    let rc = run_config(a);
    let jobs = a.jobs.min(a.seeds.clone().count()).max(1);
    let mut results: Vec<SeedResult<A::Config>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..jobs)
            .map(|j| {
                let (rc, config_for, seeds) = (&rc, &config_for, a.seeds.clone());
                let dir = workdir.join(format!("j{j}"));
                s.spawn(move || {
                    seeds
                        .filter(|seed| (seed % jobs as u64) as usize == j)
                        .map(|seed| {
                            let cfg = config_for(seed);
                            let ops = generate(seed, &WorkloadConfig::default());
                            let r = run_case::<A>(ops, &cfg, &dir, seed, rc);
                            (seed, cfg, r)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("iş parçacığı panikledi"))
            .collect()
    });
    results.sort_by_key(|(seed, _, _)| *seed);
    results
}

fn cmd_run(args: &[String]) -> Result<ExitCode, String> {
    let a = parse_run(args)?;
    let workdir = default_workdir(&a, "run");
    println!(
        "crashcontract run: motor={} seed={}..{} kesme={:?} yırtık={} iş={}",
        a.engine, a.seeds.start, a.seeds.end, a.cuts, a.torn, a.jobs
    );
    let start = Instant::now();
    let results = run_all::<ToyDb>(&a, &workdir, |seed| toy_config(&a, seed));
    let elapsed = start.elapsed();

    let (mut ops, mut events, mut cuts, mut states) = (0usize, 0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    for (seed, cfg, r) in &results {
        let o = r
            .as_ref()
            .map_err(|e| format!("seed {seed} ({cfg:?}): altyapı hatası: {e}"))?;
        ops += o.ops;
        events += o.events;
        cuts += o.cuts;
        states += o.states;
        if let Some(f) = &o.failure {
            failures.push((seed, cfg, f));
        }
    }
    println!(
        "seed: {}  işlem: {ops}  olay: {events}  kesme: {cuts}  çökme durumu: {states}",
        results.len()
    );
    println!(
        "bulgulu seed: {}  süre: {:.1} sn  ({:.0} durum/sn)",
        failures.len(),
        elapsed.as_secs_f64(),
        states as f64 / elapsed.as_secs_f64().max(1e-9)
    );
    let first = failures.first().map(|(seed, _, _)| **seed);
    let code = match failures.first() {
        None => ExitCode::SUCCESS,
        Some((seed, cfg, f)) => {
            println!("\nilk bulgu: seed {seed}  yapılandırma {cfg:?}");
            match (f.cut, f.crash_point) {
                (Some(cut), Some(cp)) => println!(
                    "  kesme: {cut} (tamamlanan işlem {}, yarıda: {})",
                    cp.completed, cp.in_flight
                ),
                _ => println!("  kesme: yok (çökmesiz koşuda okuma)"),
            }
            if let Some(choice) = &f.choice {
                println!("  seçim: {choice:?}");
            }
            println!("  bulgu: {}", f.finding);
            if let Some(r) = &f.recovered {
                println!("  kurtarılan: {} anahtar", r.len());
            }
            ExitCode::from(1)
        }
    };
    if let Some(seed) = first {
        println!();
        write_seed_report(&a, seed, &workdir)?;
    }
    let _ = std::fs::remove_dir_all(&workdir);
    Ok(code)
}

fn cmd_minimize(args: &[String]) -> Result<ExitCode, String> {
    if !args.iter().any(|x| x == "--seed") {
        return Err("minimize: --seed N gerekli".into());
    }
    let a = parse_run(args)?;
    let workdir = default_workdir(&a, "min");
    let written = write_seed_report(&a, a.seeds.start, &workdir);
    let _ = std::fs::remove_dir_all(&workdir);
    Ok(match written? {
        Some(_) => ExitCode::from(1),
        None => {
            println!("seed {}: bulgu yok", a.seeds.start);
            ExitCode::SUCCESS
        }
    })
}

fn cmd_replay(args: &[String]) -> Result<ExitCode, String> {
    let [path] = args else {
        return Err("replay: tek bir rapor yolu gerekli".into());
    };
    let report = read_report(Path::new(path))?;
    let workdir = std::env::temp_dir().join(format!("crashcontract-replay-{}", std::process::id()));
    println!(
        "replay: {} seed {} — {} işlem",
        report.engine.label(),
        report.seed,
        report.ops.len()
    );
    let r = replay(&report, &workdir);
    let _ = std::fs::remove_dir_all(&workdir);
    let r = r?;
    if r.identical {
        println!(
            "AYNI: bulgu bire bir yeniden üretildi — {}",
            report.finding_text
        );
        Ok(ExitCode::SUCCESS)
    } else {
        println!("FARKLI: rapordaki sonuç yeniden üretilemedi");
        println!("  rapordaki: {}", report.finding_text);
        match &r.failure {
            Some(f) => println!("  şimdiki:   {} (kesme {:?})", f.finding, f.cut),
            None => println!("  şimdiki:   bulgu yok"),
        }
        Ok(ExitCode::from(1))
    }
}

/// Bulgunun kısa sınıf adı (tablolar için).
fn finding_kind(f: &Finding) -> String {
    match f {
        Finding::Violation(v) => format!("{:?}", v.kind),
        Finding::RecoveryFailed(_) => "RecoveryFailed".into(),
        Finding::ScanFailed(_) => "ScanFailed".into(),
        Finding::IntegrityFailed(_) => "IntegrityFailed".into(),
    }
}

/// Bir mutantın avı: kaç seed'de yakalandı, ilk yakalama ve ona kadar denenen durum sayısı.
struct Hunt {
    mutation: Mutation,
    seeds_tried: u64,
    seeds_caught: u64,
    /// İlk yakalamaya kadar (o seed dahil) denenen çökme durumu.
    states_to_first: usize,
    first: Option<(u64, FailureCase)>,
}

/// Mutantı aralıktaki HER seed'de koşar (tüm kesmeler + yırtık yazma: M6 ancak "ortada yırtık
/// kayıt" durumlarıyla yakalanır). Seed başına ilk bulguda durulur; oran = yakalanan seed / seed.
fn hunt(m: Mutation, a: &RunArgs, workdir: &Path) -> Result<Hunt, String> {
    let rc = RunConfig {
        crash: CrashConfig {
            torn_writes: true,
            ..CrashConfig::default()
        },
        cuts: CutSelection::All,
    };
    let mut h = Hunt {
        mutation: m,
        seeds_tried: 0,
        seeds_caught: 0,
        states_to_first: 0,
        first: None,
    };
    for seed in a.seeds.clone() {
        let cfg = ToyConfig {
            mutation: Some(m),
            ..toy_config(a, seed)
        };
        let ops = generate(seed, &WorkloadConfig::default());
        let o = run_case::<ToyDb>(ops, &cfg, workdir, seed, &rc)
            .map_err(|e| format!("{} seed {seed}: altyapı hatası: {e}", m.name()))?;
        h.seeds_tried += 1;
        if h.first.is_none() {
            h.states_to_first += o.states;
        }
        if let Some(f) = o.failure {
            h.seeds_caught += 1;
            if h.first.is_none() {
                h.first = Some((seed, f));
            }
        }
    }
    Ok(h)
}

fn cmd_mutants(args: &[String]) -> Result<ExitCode, String> {
    let mut a = parse_run(args)?;
    if args.iter().all(|x| x != "--seeds") {
        a.seeds = 0..100;
    }
    let workdir = a.workdir.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("crashcontract-mut-{}", std::process::id()))
    });
    println!(
        "crashcontract mutants: seed={}..{} (yapılandırma mixed), tüm kesmeler, yırtık yazma açık",
        a.seeds.start, a.seeds.end
    );
    let start = Instant::now();
    // Mutantlar birbirinden bağımsız: her biri kendi iş parçacığında ve dizininde.
    let hunts: Vec<Result<Hunt, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = Mutation::ALL
            .into_iter()
            .map(|m| {
                let (a, dir) = (&a, workdir.join(m.name()));
                s.spawn(move || hunt(m, a, &dir))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("iş parçacığı panikledi"))
            .collect()
    });
    let _ = std::fs::remove_dir_all(&workdir);
    println!(
        "{:<24} {:>9} {:>6} {:>10}  ilk bulgu",
        "mutant", "yakalanan", "ilk", "ilke dek"
    );
    let mut all_caught = true;
    for h in hunts {
        let h = h?;
        let rate = format!("{}/{}", h.seeds_caught, h.seeds_tried);
        match &h.first {
            Some((seed, f)) => println!(
                "{:<24} {:>9} {:>6} {:>10}  {}",
                h.mutation.name(),
                rate,
                seed,
                h.states_to_first,
                finding_kind(&f.finding)
            ),
            None => {
                all_caught = false;
                println!(
                    "{:<24} {:>9} {:>6} {:>10}  YAKALANMADI",
                    h.mutation.name(),
                    rate,
                    "-",
                    "-"
                );
            }
        }
    }
    println!("süre: {:.1} sn", start.elapsed().as_secs_f64());
    Ok(if all_caught {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => cmd_run(&args[1..]),
        Some("mutants") => cmd_mutants(&args[1..]),
        Some("minimize") => cmd_minimize(&args[1..]),
        Some("replay") => cmd_replay(&args[1..]),
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => Err("komut eksik ya da bilinmiyor".into()),
    };
    match result {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("hata: {msg}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
