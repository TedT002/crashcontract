//! Rapor + küçültme + replay testleri: her mutant için küçültülmüş rapor aynı bulgu
//! sınıfını korur, JSON gidiş-dönüş eşittir ve replay sonucu bire bir aynıdır.

use std::path::PathBuf;

use cc_adapter::runner::{CutSelection, RunConfig};
use cc_cli::{EngineSpec, Report, read_report, replay, report_for_seed, write_report};
use cc_minimize::same_class;
use cc_trace::CrashConfig;
use cc_workload::WorkloadConfig;
use toy_mutants::catalog;

fn dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("cc-cli-report")
        .join(name)
}

/// Mutantın ilk bulgulu seed'i için küçültülmüş rapor (seed 0..10 içinde bulunmalı).
/// `tag`: testler paralel koşar, her test kendi çalışma dizinini kullanmalı.
fn first_report(spec: &toy_mutants::MutantSpec, minimize: bool, tag: &str) -> Report {
    let engine = EngineSpec::Toy(spec.config.clone());
    let rc = RunConfig {
        crash: CrashConfig {
            torn_writes: spec.needs_torn,
            ..CrashConfig::default()
        },
        cuts: CutSelection::All,
        skip_open_phase: false,
    };
    let wd = dir(&format!("{tag}-{}-{minimize}", spec.mutation.name()));
    (0..10)
        .find_map(|seed| {
            report_for_seed(
                &engine,
                seed,
                &WorkloadConfig::default(),
                &rc,
                &wd,
                minimize,
            )
            .unwrap()
        })
        .unwrap_or_else(|| panic!("{}: 10 seed'de bulgu yok", spec.mutation.name()))
}

/// Her mutant: küçültme aynı sınıfı korur ve diziyi kısaltır; JSON gidiş-dönüş eşit; replay aynı.
#[test]
fn minimized_reports_replay_identically() {
    let mut lines = Vec::new();
    for spec in catalog() {
        let name = spec.mutation.name();
        let raw = first_report(&spec, false, "ana");
        let min = first_report(&spec, true, "ana");
        let stats = min.minimization.clone().expect("küçültme istatistiği");
        assert_eq!(raw.seed, min.seed);
        assert!(
            same_class(&raw.failure.finding, &min.failure.finding),
            "{name}: sınıf değişti: {} → {}",
            raw.finding_text,
            min.finding_text
        );
        assert_eq!(stats.original_ops, raw.ops.len());
        assert_eq!(stats.minimized_ops, min.ops.len());
        assert!(min.ops.len() <= raw.ops.len());

        // JSON gidiş-dönüş.
        let path = dir("json").join(format!("{name}.json"));
        write_report(&path, &min).unwrap();
        let back = read_report(&path).unwrap();
        assert_eq!(back, min, "{name}: JSON gidiş-dönüş farklı");

        // Replay: okunan rapordan bire bir aynı sonuç (iki kez — determinizm).
        for i in 0..2 {
            let r = replay(&back, &dir(&format!("replay-{name}-{i}"))).unwrap();
            assert!(
                r.identical,
                "{name}: replay farklı: {:?}",
                r.failure.map(|f| f.finding)
            );
        }
        lines.push(format!(
            "{name:<24} seed {:>2}  işlem {:>3} → {:<2} anahtar {} → {} değer baytı {:>4} → {:<3} test {:>4}  {}",
            min.seed,
            stats.original_ops,
            stats.minimized_ops,
            stats.original_keys,
            stats.minimized_keys,
            stats.original_value_bytes,
            stats.minimized_value_bytes,
            stats.tests_run,
            min.finding_text
        ));
    }
    eprintln!("{}", lines.join("\n"));
}

/// Raporda değişiklik → replay "farklı" der; şema sürümü uyuşmazlığı reddedilir.
#[test]
fn tampered_report_is_detected() {
    let spec = &catalog()[0];
    let mut rep = first_report(spec, true, "kurcala");
    let wd = dir("tamper");
    // Kurtarılan durumu değiştir: aynı koşu artık rapora eşit değil.
    rep.failure.recovered = Some(Default::default());
    rep.failure.cut = rep.failure.cut.map(|c| c + 1);
    assert!(!replay(&rep, &wd).unwrap().identical);
    rep.schema_version = 99;
    assert!(replay(&rep, &wd).is_err());
}

/// Depodaki `failures/*.json` raporlarının HER biri bire bir yeniden üretilir (CI'da koşar: araç
/// değişince eski raporlar sessizce geçersizleşmesin — değişiklik şema sürümü artırmayı gerektirir).
#[test]
fn committed_reports_replay_identically() {
    let failures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../failures");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&failures)
        .expect("failures/ dizini")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    assert!(
        paths.len() >= 7,
        "her mutant için bir rapor bekleniyordu: {paths:?}"
    );
    for p in paths {
        let report = read_report(&p).unwrap();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let r = replay(&report, &dir(&format!("committed-{name}"))).unwrap();
        assert!(
            r.identical,
            "{name}: replay farklı: {:?}",
            r.failure.map(|f| f.finding)
        );
    }
}
