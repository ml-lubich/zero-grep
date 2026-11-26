use std::{fs, process::Command};

fn temp() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("zg-cli-{}", std::process::id()));
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn cli_reports_matches_misses_and_bad_patterns() {
    let root = temp();
    let file = root.join("sample.txt");
    fs::write(&file, "Alpha\nBeta 42\n").unwrap();
    let exe = env!("CARGO_BIN_EXE_zg");

    let hit = Command::new(exe)
        .args(["-i", "beta \\d+", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(hit.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(hit.stdout).unwrap(),
        format!("{}:2:Beta 42\n", file.display())
    );
    let anchored = Command::new(exe)
        .args(["^Beta", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(anchored.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(anchored.stdout).unwrap(),
        format!("{}:2:Beta 42\n", file.display())
    );
    let indexed = Command::new(exe)
        .args(["(?:Alpha|Beta)", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(indexed.status.code(), Some(0));
    assert!(
        String::from_utf8(indexed.stdout)
            .unwrap()
            .contains("Beta 42")
    );
    let insensitive = Command::new(exe)
        .args(["-i", "alpha", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(insensitive.status.code(), Some(0));
    let quiet = Command::new(exe)
        .args(["-q", "Alpha", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(quiet.status.code(), Some(0));
    assert!(quiet.stdout.is_empty());
    let files = Command::new(exe)
        .args(["-l", "Beta", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(files.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(files.stdout).unwrap(),
        format!("{}\n", file.display())
    );

    let miss = Command::new(exe)
        .args(["nothing", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(miss.status.code(), Some(1));
    assert!(miss.stdout.is_empty());

    let bad = Command::new(exe)
        .args(["(", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(
        String::from_utf8(bad.stderr)
            .unwrap()
            .contains("regex parse error")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_help_works() {
    let out = Command::new(env!("CARGO_BIN_EXE_zg"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("Fast, portable code search")
    );
}
