//! CLI testleri (SPEC_stage4 §3): çıkış kodları ve özet çıktısı.

use std::process::Command;

fn crashcontract(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_crashcontract"))
        .args(args)
        .output()
        .expect("ikili çalıştırılamadı");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), text)
}

/// Doğru motor, küçük aralık: çıkış 0 ve özet satırları.
#[test]
fn run_clean_exits_zero() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cli-run");
    let (code, out) = crashcontract(&[
        "run",
        "--seeds",
        "0..4",
        "--cuts",
        "8",
        "--jobs",
        "2",
        "--workdir",
        dir.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("seed: 4"), "{out}");
    assert!(out.contains("bulgulu seed: 0"), "{out}");
}

/// Kullanım hataları: çıkış 2.
#[test]
fn usage_errors_exit_two() {
    for args in [
        vec![],
        vec!["uç"],
        vec!["run", "--seeds", "5"],
        vec!["run", "--engine", "yok"],
        vec!["run", "--cuts"],
        vec!["run", "--bilinmeyen", "1"],
        vec!["minimize", "--engine", "toy-correct"],
        vec!["replay"],
        vec!["replay", "yok.json"],
    ] {
        let (code, out) = crashcontract(&args);
        assert_eq!(code, 2, "{args:?}: {out}");
        assert!(out.contains("kullanım"), "{out}");
    }
}

/// `replay` depodaki bir raporu bire bir üretir → çıkış 0.
#[test]
fn replay_committed_report_exits_zero() {
    let report = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../failures/toy-mutant-skip-commit-fsync-seed0.json"
    );
    let (code, out) = crashcontract(&["replay", report]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("AYNI"), "{out}");
}

/// Bulgusuz seed için `minimize` rapor yazmaz → çıkış 0.
#[test]
fn minimize_clean_seed_exits_zero() {
    let out_dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cli-min");
    let (code, out) = crashcontract(&[
        "minimize",
        "--seed",
        "3",
        "--report-dir",
        out_dir.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("bulgu yok"), "{out}");
}
