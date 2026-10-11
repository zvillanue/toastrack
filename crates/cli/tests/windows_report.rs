//! Runs `toastrack windows-report` on a Windows-shaped ISO made by genisoimage + wimlib (skipped without them).
use std::{fs, path::Path, process::Command};

fn have(t: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {t}")]).output().map(|o| o.status.success()).unwrap_or(false)
}
fn sh(c: &str) {
    assert!(Command::new("sh").args(["-c", c]).status().unwrap().success(), "failed: {c}");
}
fn put(p: &Path, n: usize) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, vec![7u8; n]).unwrap();
}

#[test]
fn report_describes_a_windows_shaped_iso_without_leaking_contents() {
    if !(have("genisoimage") && have("wimlib-imagex")) {
        eprintln!("SKIPPED: genisoimage / wimlib-imagex missing");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let t = d.path().join("tree");
    for (p, n) in [("bootmgr", 5000), ("boot/bcd", 3000), ("sources/setup.exe", 7000), ("efi/microsoft/boot/bcd", 3000)] {
        put(&t.join(p), n);
    }
    put(&d.path().join("img/a.bin"), 90_000);
    fs::create_dir_all(d.path().join("img/Windows/System32")).unwrap();
    fs::write(d.path().join("img/Windows/System32/winpeshl.ini"), "[LaunchApps]\r\nsetup.exe\r\n").unwrap();
    sh(&format!(
        "wimlib-imagex capture {img} {w} 'Windows Setup' --compress=LZX >/dev/null && wimlib-imagex capture {img} {i} 'Windows 11 Pro' --compress=LZX >/dev/null",
        img = d.path().join("img").display(),
        w = t.join("sources/boot.wim").display(),
        i = t.join("sources/install.wim").display(),
    ));
    let iso = d.path().join("w.iso");
    sh(&format!("genisoimage -udf -iso-level 3 -o {} {} >/dev/null 2>&1", iso.display(), t.display()));

    let out = Command::new(env!("CARGO_BIN_EXE_toastrack")).args(["windows-report"]).arg(&iso).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    for want in ["UDF top level", "sources/boot.wim", "compression LZX", "\"Windows 11 Pro\"", "sources/setup.exe", "7000 bytes", "no file contents"] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    assert!(text.contains("SHA-1 verified"), "{text}");
    assert!(text.contains("Windows/System32/winpeshl.ini"), "{text}");
    assert!(!text.contains("[LaunchApps]"), "file contents must not appear unless requested:\n{text}");

    let out = Command::new(env!("CARGO_BIN_EXE_toastrack"))
        .args(["windows-report"]).arg(&iso).args(["--show-file", "Windows/System32/winpeshl.ini"]).output().unwrap();
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(shown.contains("[LaunchApps]"), "requested file must be printed:\n{shown}");

    // Names and sizes only: no byte of any file's content may appear (the fixtures are all 0x07 bytes).
    assert!(!text.contains('\u{7}'));
}

#[test]
fn report_on_a_non_iso_fails_cleanly() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("x.iso");
    fs::write(&f, vec![0u8; 100_000]).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_toastrack")).arg("windows-report").arg(&f).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not an ISO 9660"));
}
