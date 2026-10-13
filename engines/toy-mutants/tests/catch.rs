//! "Araç yakalıyor mu?" testleri (CLAUDE.md Aşama 5): her mutant, ortaya çıkabildiği yapılandırmada,
//! ilk SEEDS seed'in en az MIN_RATE oranında yakalanmalı ve bulgu türü beklenen sınıflardan olmalı.
//! Ayrıca doğru motor aynı yapılandırmalarda hiç bulgu vermemeli (yakalama mutasyondan gelir).

use std::collections::BTreeMap;
use std::path::PathBuf;

use cc_adapter::runner::{CutSelection, Finding, RunConfig, run_case};
use cc_trace::CrashConfig;
use cc_workload::{WorkloadConfig, generate};
use toy_correct::{ToyConfig, ToyDb};
use toy_mutants::{Expected, MutantSpec, catalog};

const SEEDS: u64 = 40;

fn workdir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("toy-mutants")
        .join(name)
}

fn run_config(spec: &MutantSpec) -> RunConfig {
    RunConfig {
        crash: CrashConfig {
            torn_writes: spec.needs_torn,
            ..CrashConfig::default()
        },
        cuts: CutSelection::All,
    }
}

fn expected_of(f: &Finding) -> Option<Expected> {
    match f {
        Finding::Violation(v) => Some(Expected::Violation(v.kind)),
        Finding::RecoveryFailed(_) => Some(Expected::RecoveryFailed),
        _ => None,
    }
}

/// Mutantı seed 0..SEEDS'te koşar: (yakalanan seed sayısı, bulgu türü → seed sayısı).
fn hunt(spec: &MutantSpec, cfg: &ToyConfig) -> (u64, BTreeMap<String, u64>) {
    // Testler paralel koşar: kontrol grubu ayrı dizin kullanmalı.
    let tag = if cfg.mutation.is_some() {
        "mutant"
    } else {
        "kontrol"
    };
    let dir = workdir(&format!("{}-{tag}", spec.mutation.name()));
    let rc = run_config(spec);
    let mut caught = 0;
    let mut kinds: BTreeMap<String, u64> = BTreeMap::new();
    for seed in 0..SEEDS {
        let ops = generate(seed, &WorkloadConfig::default());
        let o = run_case::<ToyDb>(ops, cfg, &dir, seed, &rc).expect("altyapı");
        if let Some(f) = o.failure {
            caught += 1;
            let key = match expected_of(&f.finding) {
                Some(e) if spec.expected.contains(&e) => format!("{e:?}"),
                Some(e) => format!("BEKLENMEYEN {e:?}: {}", f.finding),
                None => format!("BEKLENMEYEN {}", f.finding),
            };
            *kinds.entry(key).or_default() += 1;
        }
    }
    (caught, kinds)
}

/// Mutant başına en düşük yakalama oranı (yüzde). `accept-corrupt-record` ancak "ortada yırtık
/// kayıt" + ardından kalıcı bir kayıt gerektirir; daha seyrek ortaya çıkar (ölçüldü).
fn min_rate(spec: &MutantSpec) -> u64 {
    if spec.needs_torn { 20 } else { 80 }
}

/// Her mutant yakalanır; bulgu türleri beklenen sınıflardadır.
#[test]
fn every_mutant_is_caught() {
    let mut report = Vec::new();
    for spec in catalog() {
        let (caught, kinds) = hunt(&spec, &spec.config);
        report.push(format!(
            "{:<24} {caught:>3}/{SEEDS}  {kinds:?}",
            spec.mutation.name()
        ));
        let unexpected: Vec<_> = kinds
            .keys()
            .filter(|k| k.starts_with("BEKLENMEYEN"))
            .collect();
        assert!(
            unexpected.is_empty(),
            "{}: beklenmeyen bulgu türü {unexpected:?}",
            spec.mutation.name()
        );
        assert!(
            caught * 100 >= min_rate(&spec) * SEEDS,
            "{}: yalnız {caught}/{SEEDS} seed'de yakalandı ({kinds:?})",
            spec.mutation.name()
        );
    }
    eprintln!("{}", report.join("\n"));
}

/// Kontrol grubu: aynı yapılandırmalar mutasyonsuz → hiç bulgu yok. Yakalamanın kaynağı yapılandırma
/// değil mutasyondur.
#[test]
fn control_group_is_clean() {
    for spec in catalog() {
        let clean = ToyConfig {
            mutation: None,
            ..spec.config.clone()
        };
        let (caught, kinds) = hunt(&spec, &clean);
        assert_eq!(
            caught,
            0,
            "{} kontrol grubu: {kinds:?}",
            spec.mutation.name()
        );
    }
}

/// Katalog `Mutation::ALL` ile birebir ve aynı sırada; her yapılandırma kendi mutasyonunu taşır.
#[test]
fn catalog_covers_all_mutations() {
    let c = catalog();
    let ms: Vec<_> = c.iter().map(|s| s.mutation).collect();
    assert_eq!(ms, toy_correct::Mutation::ALL.to_vec());
    for s in &c {
        assert_eq!(s.config.mutation, Some(s.mutation));
        assert!(!s.expected.is_empty());
    }
    assert!(c.len() >= 6, "CLAUDE.md: en az 6 hata sınıfı");
}
