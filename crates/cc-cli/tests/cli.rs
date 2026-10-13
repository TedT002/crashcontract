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
    ] {
        let (code, out) = crashcontract(&args);
        assert_eq!(code, 2, "{args:?}: {out}");
        assert!(out.contains("kullanım"), "{out}");
    }
}
