//! The state and decisions behind the window, kept free of drawing code so they can be tested.

use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
};
use toastrack_installer::{
    bundle::Bundle,
    device::{self, DiskInfo},
    install::DataFs,
    ops,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Install,
    Update,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Bundle,
    Unsigned,
}

pub enum Phase {
    Idle,
    Confirm { understood: bool },
    Working,
    Done(Result<Vec<String>, String>),
}

pub struct Model {
    pub disks: Vec<DiskInfo>,
    pub selected: Option<usize>,
    pub show_internal: bool,
    pub image_path: String,
    pub image_size: String,
    pub mode: Mode,
    pub data_fs: DataFs,
    pub source: Source,
    pub source_path: String,
    pub attach_path: String,
    pub bundle: Result<Bundle, String>,
    pub phase: Phase,
    pub elevated: bool,
    rx: Option<Receiver<Result<Vec<String>, String>>>,
}

pub fn letters(l: &[char]) -> String {
    if l.is_empty() {
        String::new()
    } else {
        format!("   [{}]", l.iter().map(|c| format!("{c}:")).collect::<Vec<_>>().join(" "))
    }
}

/// Whether the program can write to disks: root on Unix, and on Windows the manifest has already asked for it.
pub fn is_elevated() -> bool {
    #[cfg(unix)]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).map(|l| l.split_whitespace().nth(2) == Some("0")))
            .unwrap_or(true)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Start this program again through polkit's `pkexec` with the display settings it needs, then quit.
#[cfg(target_os = "linux")]
pub fn relaunch_elevated() {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = std::process::Command::new("pkexec");
    cmd.arg("env");
    for k in ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"] {
        if let Ok(v) = std::env::var(k) {
            cmd.arg(format!("{k}={v}"));
        }
    }
    cmd.arg(exe);
    if cmd.spawn().is_ok() {
        std::process::exit(0);
    }
}

impl Model {
    pub fn new() -> Self {
        let mut m = Model {
            disks: Vec::new(),
            selected: None,
            show_internal: false,
            image_path: String::new(),
            image_size: "8G".into(),
            mode: Mode::Install,
            data_fs: DataFs::Exfat,
            source: Source::Bundle,
            source_path: Bundle::discover().map(|p| p.display().to_string()).unwrap_or_default(),
            attach_path: String::new(),
            bundle: Err(String::new()),
            phase: Phase::Idle,
            elevated: is_elevated(),
            rx: None,
        };
        m.refresh();
        m.reload_bundle();
        m
    }

    pub fn refresh(&mut self) {
        let was = self.selected.and_then(|i| self.disks.get(i)).map(|d| d.path.clone());
        self.disks = device::list();
        self.selected = was.and_then(|p| self.disks.iter().position(|d| d.path == p));
        // A single USB stick is the obvious choice.
        if self.selected.is_none() {
            let usb: Vec<usize> = self.visible_disks().into_iter().map(|(i, _)| i).collect();
            if usb.len() == 1 {
                self.selected = Some(usb[0]);
            }
        }
    }

    /// Disks to offer: never the system disk, and USB/removable ones unless internal disks were asked for.
    pub fn visible_disks(&self) -> Vec<(usize, &DiskInfo)> {
        self.disks.iter().enumerate().filter(|(_, d)| !d.system && (self.show_internal || d.usb || d.removable)).collect()
    }

    pub fn reload_bundle(&mut self) {
        let p = self.source_path.trim();
        self.bundle = if p.is_empty() {
            Err(match self.source {
                Source::Bundle => "no bundle found next to the program; unpack the release package completely, or open \"Boot files (advanced)\" and give the folder".to_string(),
                Source::Unsigned => "give the loader file in \"Boot files (advanced)\"".to_string(),
            })
        } else {
            match self.source {
                Source::Bundle => Bundle::load(Path::new(p)),
                Source::Unsigned => {
                    let attach = self.attach_path.trim();
                    Bundle::unsigned(Path::new(p), (!attach.is_empty()).then(|| Path::new(attach)))
                }
            }
            .map_err(|e| e.to_string())
        };
    }

    /// The disk or image file the operation would write to.
    pub fn target(&self) -> Option<PathBuf> {
        let img = self.image_path.trim();
        if !img.is_empty() {
            return Some(PathBuf::from(img));
        }
        let d = self.selected.and_then(|i| self.disks.get(i))?;
        Some(d.path.clone())
    }

    fn image_size_bytes(&self) -> Result<u64, String> {
        let s = self.image_size.trim();
        let (num, mult) = match s.chars().last() {
            Some('G' | 'g') => (&s[..s.len() - 1], 1u64 << 30),
            Some('M' | 'm') => (&s[..s.len() - 1], 1u64 << 20),
            _ => (s, 1),
        };
        num.trim().parse::<u64>().map(|n| n * mult).map_err(|_| format!("bad image size {s:?} (try 8G)"))
    }

    /// Why Write is disabled, if it is.
    pub fn why_not(&self) -> Option<String> {
        let Some(target) = self.target() else { return Some("Choose a stick first.".into()) };
        if self.bundle.is_err() {
            return Some("The boot files are not ready (see above).".into());
        }
        if !self.image_path.trim().is_empty() && !target.exists() && self.mode == Mode::Install {
            if let Err(e) = self.image_size_bytes() {
                return Some(e);
            }
        }
        None
    }

    pub fn can_write(&self) -> bool {
        self.why_not().is_none()
    }

    pub fn confirm_text(&self) -> String {
        let Some(target) = self.target() else { return String::new() };
        let disk = self.selected.and_then(|i| self.disks.get(i)).filter(|_| self.image_path.trim().is_empty());
        match (self.mode, disk) {
            (Mode::Install, Some(d)) => format!("ALL DATA on this disk will be erased:\n\n{}", d.describe()),
            (Mode::Install, None) if target.exists() => format!("ALL DATA in the image file {} will be erased.", target.display()),
            (Mode::Install, None) => format!("A new image file {} will be created.", target.display()),
            (Mode::Update, Some(d)) => format!("The boot files on this disk will be replaced; your ISOs stay:\n\n{}", d.describe()),
            (Mode::Update, None) => format!("The boot files in {} will be replaced; your ISOs stay.", target.display()),
        }
    }

    /// Run the operation on a worker thread.
    pub fn start(&mut self) {
        let (Some(target), Ok(bundle)) = (self.target(), self.bundle.clone()) else { return };
        let (mode, fs, internal) = (self.mode, self.data_fs, self.show_internal);
        let size = if !self.image_path.trim().is_empty() && !target.exists() { self.image_size_bytes().ok() } else { None };
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.phase = Phase::Working;
        thread::spawn(move || {
            let result = match mode {
                Mode::Install => ops::install_to(&target, &bundle, fs, size, internal).map(|_| ops::next_steps(&bundle)),
                Mode::Update => ops::update_at(&target, &bundle, internal).map(|_| vec!["The boot files were replaced; your ISOs are untouched.".to_string()]),
            };
            let _ = tx.send(result.map_err(|e| e.to_string()));
        });
    }

    /// Pick up the result of a finished operation.
    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok(r) = rx.try_recv() {
                self.phase = Phase::Done(r);
                self.rx = None;
                self.refresh();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn disk(path: &str, usb: bool, removable: bool, system: bool) -> DiskInfo {
        DiskInfo { path: PathBuf::from(path), size: 62_000_000_000, model: "Stick".into(), removable, usb, mounted: false, letters: vec![], system }
    }

    fn model(disks: Vec<DiskInfo>) -> Model {
        let mut m = Model::new();
        m.disks = disks;
        m.selected = None;
        m.show_internal = false;
        m.image_path.clear();
        m.source_path.clear();
        m.reload_bundle();
        m
    }

    #[test]
    fn only_usb_or_removable_disks_are_offered_and_never_the_system_disk() {
        let mut m = model(vec![disk("/dev/nvme0n1", false, false, true), disk("/dev/sda", true, false, false), disk("/dev/sdb", false, false, false), disk("/dev/sdc", false, true, false)]);
        let names = |m: &Model| m.visible_disks().iter().map(|(_, d)| d.path.display().to_string()).collect::<Vec<_>>();
        assert_eq!(names(&m), ["/dev/sda", "/dev/sdc"]);
        m.show_internal = true;
        assert_eq!(names(&m), ["/dev/sda", "/dev/sdb", "/dev/sdc"], "the system disk stays hidden");
    }

    #[test]
    fn write_needs_a_target_and_boot_files() {
        let mut m = model(vec![disk("/dev/sda", true, false, false)]);
        assert!(m.why_not().unwrap().contains("Choose a stick"));
        m.selected = Some(0);
        assert!(m.why_not().unwrap().contains("boot files"));
    }

    #[test]
    fn image_file_overrides_the_selection_and_needs_a_size_when_new() {
        let mut m = model(vec![disk("/dev/sda", true, false, false)]);
        m.selected = Some(0);
        m.image_path = "/nonexistent/new.img".into();
        assert_eq!(m.target().unwrap(), PathBuf::from("/nonexistent/new.img"));
        m.image_size = "banana".into();
        m.bundle = Ok(Bundle { shim: vec![], mok_manager: vec![], loader: vec![], cert: vec![], attach_exe: None, secure_boot: false });
        assert!(m.why_not().unwrap().contains("bad image size"));
        m.image_size = "8G".into();
        assert_eq!(m.why_not(), None);
        assert_eq!(m.image_size_bytes().unwrap(), 8 << 30);
    }

    #[test]
    fn confirmation_names_the_disk() {
        let mut m = model(vec![disk("/dev/sda", true, false, false)]);
        m.selected = Some(0);
        assert!(m.confirm_text().contains("/dev/sda"));
        assert!(m.confirm_text().contains("erased"));
        m.mode = Mode::Update;
        assert!(m.confirm_text().contains("ISOs stay"));
    }
}

#[cfg(test)]
mod end_to_end {
    use super::*;

    /// Writes a real image through the model, as the Write button does. Runs when TOASTRACK_TEST_LOADER names a built loader.
    #[test]
    fn write_stick_to_an_image_file_through_the_model() {
        let Ok(loader) = std::env::var("TOASTRACK_TEST_LOADER") else { return };
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("stick.img");
        let mut m = Model::new();
        m.source = Source::Unsigned;
        m.source_path = loader;
        m.reload_bundle();
        assert!(m.bundle.is_ok(), "{:?}", m.bundle.as_ref().err());
        m.image_path = img.display().to_string();
        m.image_size = "400M".into();
        assert!(m.can_write());
        m.start();
        for _ in 0..600 {
            m.poll();
            if !matches!(m.phase, Phase::Working) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        match &m.phase {
            Phase::Done(Ok(steps)) => assert!(steps.iter().any(|s| s.contains("Secure Boot turned off")), "{steps:?}"),
            Phase::Done(Err(e)) => panic!("{e}"),
            _ => panic!("still working"),
        }
        let problems = ops::verify_at(&img, m.bundle.as_ref().unwrap()).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
    }
}
