use std::{fs, process::Command};

#[test]
fn bare_output_filename_supports_single_and_split_packages() {
    let dir = tempfile::tempdir().expect("fixture");
    fs::create_dir(dir.path().join("assets")).expect("source");
    let script = "let alice = 1\n";
    fs::write(dir.path().join("assets/startup.hks"), script).expect("script");
    fs::write(dir.path().join("assets/payload.bin"), vec![42; 16384]).expect("payload");
    for volume_size in ["4096", "0"] {
        let result = Command::new(env!("CARGO_BIN_EXE_hiraku-tools"))
            .current_dir(dir.path())
            .args([
                "pack",
                "assets/",
                "main.hdp",
                "--compression",
                "none",
                "--chunk-size",
                "512",
                "--volume-size",
                volume_size,
            ])
            .output()
            .expect("run pack");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let archive = hiraku_hdp::Archive::open(dir.path().join("main.hdp")).expect("archive");
        assert_eq!(
            archive.read_file("startup.hks").expect("script"),
            script.as_bytes()
        );
        assert_eq!(
            archive.read_file("payload.bin").expect("payload"),
            vec![42; 16384]
        );
    }
    assert!(
        !dir.path().join("main.hdp.1").exists(),
        "obsolete split volumes are removed"
    );
}

#[test]
fn cli_packs_and_rejects_unknown_options_without_creating_output() {
    let dir = tempfile::tempdir().expect("fixture");
    let source = dir.path().join("source");
    fs::create_dir(&source).expect("source");
    fs::write(source.join("startup.hks"), "let alice = 1").expect("script");
    let output = dir.path().join("test.hdp");
    let result = Command::new(env!("CARGO_BIN_EXE_hiraku-tools"))
        .arg("pack")
        .arg(&source)
        .arg(&output)
        .args([
            "--no-uastc",
            "--compression",
            "none",
            "--chunk-size",
            "128",
            "--volume-size",
            "4096",
        ])
        .output()
        .expect("run pack");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let archive = hiraku_hdp::Archive::open(&output).expect("archive");
    assert_eq!(
        archive.read_file("startup.hks").expect("script"),
        b"let alice = 1"
    );
    let bad = dir.path().join("bad.hdp");
    let result = Command::new(env!("CARGO_BIN_EXE_hiraku-tools"))
        .arg("pack")
        .arg(&source)
        .arg(&bad)
        .args(["--unknown", "1"])
        .output()
        .expect("invalid invocation");
    assert!(!result.status.success());
    assert!(!bad.exists());
    assert!(
        hiraku_tools::pack::directory(&source, &source.join("recursive.hdp"), Default::default())
            .is_err()
    );
}
