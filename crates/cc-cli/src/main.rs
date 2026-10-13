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
use cc_adapter::runner::{CutSelection, RunConfig, RunError, SeedOutcome, run_case};
use cc_trace::CrashConfig;
use cc_workload::{WorkloadConfig, generate};
use toy_correct::{SyncPolicy, ToyConfig, ToyDb};

const USAGE: &str = "\
kullanım: crashcontract run [seçenekler]

  --engine toy-correct            test edilen motor (varsayılan: toy-correct)
  --seeds A..B                    seed aralığı, B hariç (varsayılan: 0..100)
  --policy every-commit|on-sync|mixed
                                  toy senkronizasyon politikası (varsayılan: mixed = seed%2)
  --checkpoint-every N|mixed      toy checkpoint aralığı (varsayılan: mixed = 0 ya da 7)
  --cuts all|N                    kesme seçimi: hepsi ya da seed başına N örnek (varsayılan: all)
  --torn                          sektör seviyesinde yırtık yazmaları da dene
  --jobs N                        paralel iş parçacığı (varsayılan: çekirdek sayısı)
  --workdir DIR                   çalışma dizini (varsayılan: geçici dizin)

çıkış kodu: 0 bulgu yok, 1 bulgu var, 2 kullanım/altyapı hatası";

#[derive(Debug, Clone, Copy)]
enum Mixed<T> {
    Fixed(T),
    Mixed,
}

#[derive(Debug)]
struct RunArgs {
    engine: String,
    seeds: std::ops::Range<u64>,
    policy: Mixed<SyncPolicy>,
    checkpoint_every: Mixed<u32>,
    cuts: CutSelection,
    torn: bool,
    jobs: usize,
    workdir: Option<PathBuf>,
}

fn parse_num<T: std::str::FromStr>(flag: &str, v: &str) -> Result<T, String> {
    v.parse()
        .map_err(|_| format!("{flag}: sayı bekleniyordu, '{v}' geldi"))
}

fn parse_run(args: &[String]) -> Result<RunArgs, String> {
    let mut a = RunArgs {
        engine: "toy-correct".into(),
        seeds: 0..100,
        policy: Mixed::Mixed,
        checkpoint_every: Mixed::Mixed,
        cuts: CutSelection::All,
        torn: false,
        jobs: std::thread::available_parallelism().map_or(1, |n| n.get()),
        workdir: None,
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        if flag == "--torn" {
            a.torn = true;
            continue;
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
            _ => return Err(format!("bilinmeyen seçenek '{flag}'")),
        }
    }
    if a.engine != "toy-correct" {
        return Err(format!("bilinmeyen motor '{}'", a.engine));
    }
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
    }
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
    let rc = RunConfig {
        crash: CrashConfig {
            torn_writes: a.torn,
            ..CrashConfig::default()
        },
        cuts: a.cuts,
    };
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
    let workdir = a.workdir.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("crashcontract-{}", std::process::id()))
    });
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
    let _ = std::fs::remove_dir_all(&workdir);
    match failures.first() {
        None => Ok(ExitCode::SUCCESS),
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
            Ok(ExitCode::from(1))
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => cmd_run(&args[1..]),
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
