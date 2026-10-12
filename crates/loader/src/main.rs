#![no_main]
#![no_std]

extern crate alloc;

mod boot;
mod gfx;
mod hid;
mod linux;
mod vdisk;
mod vol;

use alloc::{format, rc::Rc, string::String, vec::Vec};
use core::time::Duration;
use gfx::{rgb, Color, Screen};
use toastrack_core::{analyze_iso, bootcfg::{self, BootEntry}, checksum, io::ReadAt, iso::Iso, SecureBootStatus};
use uefi::{
    prelude::*,
    proto::console::{
        pointer::{AbsolutePointer, Pointer},
        text::{Key, ScanCode},
    },
    runtime::VariableVendor,
    boot::ScopedProtocol,
};
use vol::{Item, Kind};

const BG: Color = rgb(18, 20, 26);
const PANEL: Color = rgb(28, 31, 40);
const SEL: Color = rgb(44, 62, 104);
const HOVER: Color = rgb(36, 40, 52);
const FG: Color = rgb(225, 228, 236);
const DIM: Color = rgb(130, 136, 152);
const ACCENT: Color = rgb(235, 170, 60);

const TOP: i32 = 96;
const ROW: i32 = 32;
const FOOT: i32 = 56;

#[derive(Clone)]
enum Badge {
    None,
    Status(SecureBootStatus),
    Unreadable,
}

/// Result of a checksum run on one ISO (from the `C` key).
#[derive(Clone, Copy, PartialEq)]
enum Sum {
    Untested,
    Good,
    Bad,
}

struct Menu {
    scr: Screen,
    vol: Rc<vol::DataVolume>,
    path: Vec<String>,
    items: Vec<Item>,
    badges: Vec<Badge>,
    sums: Vec<Sum>,
    /// Set after the "damaged" warning, so that a second Enter boots anyway.
    warned_bad: bool,
    sel: usize,
    scroll: usize,
    msg: Option<(String, Color)>,
    sb_on: bool,
    mouse: (i32, i32),
    /// Set while the boot entries of one ISO are shown instead of a folder.
    open_iso: Option<OpenIso>,
}

struct OpenIso {
    name: String,
    windows: bool,
    entries: Vec<BootEntry>,
    saved: (Vec<Item>, Vec<Badge>, usize, usize),
}

fn secure_boot_enabled() -> bool {
    let mut b = [0u8; 1];
    matches!(
        uefi::runtime::get_variable(cstr16!("SecureBoot"), &VariableVendor::GLOBAL_VARIABLE, &mut b),
        Ok((d, _)) if d.first() == Some(&1)
    )
}

fn fmt_size(s: u64) -> String {
    const G: u64 = 1 << 30;
    if s >= G {
        format!("{}.{} GB", s / G, s % G * 10 / G)
    } else {
        format!("{} MB", s >> 20)
    }
}

impl Menu {
    fn visible(&self) -> usize {
        ((self.scr.h as i32 - TOP - FOOT) / ROW).max(1) as usize
    }

    fn reload(&mut self) {
        self.items = self.vol.list(&self.path).unwrap_or_else(|e| {
            self.msg = Some((format!("Cannot read this folder: {e}"), rgb(218, 54, 51)));
            Vec::new()
        });
        self.badges = alloc::vec![Badge::None; self.items.len()];
        self.sums = alloc::vec![Sum::Untested; self.items.len()];
        self.sel = 0;
        self.scroll = 0;
        for i in 0..self.items.len() {
            if self.items[i].kind != Kind::Iso {
                continue;
            }
            self.msg = Some((format!("Scanning {} ...", self.items[i].name), DIM));
            self.draw();
            self.scr.present();
            self.badges[i] = match self.vol.open_file(&self.path, &self.items[i].name)
                .ok()
                .and_then(|f| Iso::open(f).ok())
                .map(|mut iso| analyze_iso(&mut iso))
            {
                Some(Ok(s)) => Badge::Status(s),
                _ => Badge::Unreadable,
            };
        }
        self.msg = None;
    }

    fn clamp_scroll(&mut self) {
        let v = self.visible();
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + v {
            self.scroll = self.sel + 1 - v;
        }
    }

    fn badge(b: &Badge) -> (&'static str, Color) {
        match b {
            Badge::None => ("", DIM),
            Badge::Unreadable => ("UNREADABLE", rgb(120, 120, 130)),
            Badge::Status(SecureBootStatus::Trusted { .. }) => ("SECURE BOOT OK", rgb(46, 160, 67)),
            Badge::Status(SecureBootStatus::NeedsEnrollment { .. }) => ("NEEDS KEY ENROLL", rgb(210, 153, 34)),
            Badge::Status(SecureBootStatus::Unsigned) => ("UNSIGNED", rgb(218, 54, 51)),
            Badge::Status(SecureBootStatus::NoUefiLoader) => ("NO UEFI LOADER", rgb(120, 120, 130)),
        }
    }

    fn draw(&mut self) {
        let (w, h) = (self.scr.w as i32, self.scr.h as i32);
        self.scr.clear(BG);
        self.scr.rect(0, 0, w, 64, PANEL);
        self.scr.text(24, 18, "TOASTRACK", 3, ACCENT);
        let (sb, sbc) = if self.sb_on { ("Secure Boot: ON", rgb(46, 160, 67)) } else { ("Secure Boot: OFF", DIM) };
        self.scr.text(w - Screen::text_width(sb, 2) - 24, 24, sb, 2, sbc);
        let mut crumb = format!("/{}", self.path.join("/"));
        if let Some(o) = &self.open_iso {
            crumb = format!("{}/{}", crumb.trim_end_matches('/'), o.name);
        }
        self.scr.text(24, 72, &crumb, 2, DIM);
        let fsname = self.vol.kind();
        self.scr.text(w - Screen::text_width(fsname, 2) - 24, 72, fsname, 2, DIM);

        let vis = self.visible();
        let hover = self.row_at(self.mouse);
        for (n, idx) in (self.scroll..self.items.len().min(self.scroll + vis)).enumerate() {
            let y = TOP + n as i32 * ROW;
            let bg = if idx == self.sel { SEL } else if hover == Some(idx) { HOVER } else { BG };
            self.scr.rect(12, y, w - 24, ROW - 2, bg);
            let it = &self.items[idx];
            let (tag, tagc) = match it.kind {
                Kind::Up => ("<-", DIM),
                Kind::Dir => ("DIR", rgb(90, 160, 230)),
                Kind::Iso if matches!(&self.badges[idx], Badge::Status(st) if st.is_windows()) => ("WIN", rgb(0, 164, 239)),
                Kind::Iso => ("ISO", ACCENT),
                Kind::Entry => ("BOOT", rgb(46, 160, 67)),
                Kind::Chain => ("EFI", rgb(160, 120, 220)),
            };
            self.scr.text(24, y + 8, tag, 2, tagc);
            let name = if it.kind == Kind::Dir { format!("{}/", it.name) } else { it.name.clone() };
            self.scr.text(104, y + 8, &name, 2, FG);
            if it.kind == Kind::Iso {
                let (label, c) = Self::badge(&self.badges[idx]);
                let lw = Screen::text_width(label, 2) + 16;
                let bx = w - 24 - lw;
                self.scr.rect(bx, y + 4, lw, ROW - 10, c);
                self.scr.text(bx + 8, y + 8, label, 2, rgb(10, 10, 10));
                let mut right = bx;
                let sum = self.sums.get(idx).copied().unwrap_or(Sum::Untested);
                if sum != Sum::Untested {
                    let (l, c) = if sum == Sum::Good { ("SUM OK", rgb(46, 160, 67)) } else { ("DAMAGED", rgb(218, 54, 51)) };
                    let sw = Screen::text_width(l, 2) + 16;
                    right = bx - 12 - sw;
                    self.scr.rect(right, y + 4, sw, ROW - 10, c);
                    self.scr.text(right + 8, y + 8, l, 2, rgb(10, 10, 10));
                }
                let sz = fmt_size(it.size);
                self.scr.text(right - 24 - Screen::text_width(&sz, 2), y + 8, &sz, 2, DIM);
            }
        }
        self.scr.rect(0, h - FOOT, w, FOOT, PANEL);
        let hint = "Up/Down select   Enter open/boot   C check ISO checksum   Esc back   Mouse: click to select, click again to open";
        self.scr.text(24, h - FOOT + 8, hint, 1, DIM);
        if let Some((m, c)) = self.msg.clone() {
            let cols = ((w - 48) / 8) as usize; // scale-1 glyphs, wrapped
            for (i, line) in m.as_bytes().chunks(cols.max(1)).take(3).enumerate() {
                self.scr.text(24, h - FOOT + 24 + i as i32 * 10, core::str::from_utf8(line).unwrap_or("?"), 1, c);
            }
        }
        self.scr.cursor(self.mouse.0, self.mouse.1);
    }

    fn row_at(&self, (x, y): (i32, i32)) -> Option<usize> {
        if y < TOP || x < 0 {
            return None;
        }
        let idx = self.scroll + ((y - TOP) / ROW) as usize;
        (idx < self.items.len() && idx < self.scroll + self.visible()).then_some(idx)
    }

    fn up(&mut self) {
        if let Some(o) = self.open_iso.take() {
            (self.items, self.badges, self.sel, self.scroll) = o.saved;
        } else if self.path.pop().is_some() {
            self.reload();
        }
    }

    fn open_iso_entries(&mut self, name: String) {
        let entries = self.vol.open_file(&self.path, &name)
            .ok()
            .and_then(|f| Iso::open(f).ok())
            .and_then(|mut iso| bootcfg::entries(&mut iso).ok())
            .unwrap_or_default();
        let mut items = alloc::vec![Item { name: "..".into(), kind: Kind::Up, size: 0 }];
        items.extend(entries.iter().map(|e| Item { name: e.title.clone(), kind: Kind::Entry, size: 0 }));
        items.push(Item { name: "Boot the ISO's own bootloader (chainload)".into(), kind: Kind::Chain, size: 0 });
        let n = items.len();
        // `force-windows-handoff` is a test switch: treat every ISO as Windows to exercise the hand-off with Linux media.
        let is_windows = cfg!(feature = "force-windows-handoff") || matches!(&self.badges[self.sel], Badge::Status(st) if st.is_windows());
        let saved = (
            core::mem::replace(&mut self.items, items),
            core::mem::replace(&mut self.badges, alloc::vec![Badge::None; n]),
            self.sel,
            self.scroll,
        );
        self.sel = if entries.is_empty() { n - 1 } else { 1 };
        self.scroll = 0;
        if is_windows {
            self.msg = Some((
                String::from(
                    "Windows ISO: choose the chainload entry. Windows Setup needs the stick's autounattend.xml + toastrack\\attach.exe \
                     to find its install files (experimental, see docs/WINDOWS-TESTING.md).",
                ),
                rgb(210, 153, 34),
            ));
        }
        self.open_iso = Some(OpenIso { name, windows: is_windows, entries, saved });
    }

    fn start(&mut self, label: &str, f: impl FnOnce(&Self) -> Result<(), String>) {
        self.msg = Some((format!("Booting {label} ..."), ACCENT));
        self.draw();
        self.scr.present();
        if let Err(e) = f(self) {
            self.msg = Some((e, rgb(218, 54, 51)));
        }
    }

    fn activate(&mut self) {
        let Some(it) = self.items.get(self.sel).cloned() else { return };
        match it.kind {
            Kind::Up => self.up(),
            Kind::Entry => {
                let o = self.open_iso.as_ref().unwrap();
                let (name, entry) = (o.name.clone(), o.entries[self.sel - 1].clone());
                let plan = match boot::plan_entry(&self.vol, &self.path, &name, &entry) {
                    Ok(p) => p,
                    Err(e) => {
                        self.msg = Some((e, rgb(218, 54, 51)));
                        return;
                    }
                };
                let mut label = format!("{} [{}]", entry.title, plan.family.name());
                if !plan.notes.is_empty() {
                    label.push_str(" - ");
                    label.push_str(&plan.notes.join("; "));
                }
                let cmdline = plan.cmdline;
                if self.sb_on {
                    // Firmware + shim will not start a kernel for us; the ISO's signed GRUB must.
                    self.start(&label, |m| boot::boot_iso(&m.vol, &m.path, &name, Some((&entry, &cmdline))));
                } else {
                    self.start(&label, |m| linux::boot_entry(&m.vol, &m.path, &name, &entry, &cmdline));
                }
            }
            Kind::Chain => {
                let (name, windows) = {
                    let o = self.open_iso.as_ref().unwrap();
                    (o.name.clone(), o.windows)
                };
                let mut label = name.clone();
                if windows {
                    // Tell the helper that Setup starts which ISO to attach as the install DVD.
                    match boot::publish_windows_request(&self.vol, &self.path, &name) {
                        Ok(()) => label.push_str(" [hand-off for Windows Setup stored]"),
                        Err(e) => label.push_str(&format!(" [WARNING: Windows Setup will not find its install files: {e}]")),
                    }
                }
                self.start(&label, |m| boot::boot_iso(&m.vol, &m.path, &name, None));
            }
            Kind::Dir => {
                self.path.push(it.name);
                self.reload();
            }
            Kind::Iso => {
                if self.sums.get(self.sel) == Some(&Sum::Bad) && !self.warned_bad {
                    self.warned_bad = true;
                    self.msg = Some(("This ISO failed its checksum and probably will not boot. Press Enter again to try anyway.".into(), rgb(218, 54, 51)));
                    return;
                }
                if let Badge::Status(s @ (SecureBootStatus::Unsigned | SecureBootStatus::NoUefiLoader)) = &self.badges[self.sel] {
                    if self.sb_on {
                        let why = if *s == SecureBootStatus::Unsigned { "unsigned loader" } else { "no UEFI loader" };
                        self.msg = Some((format!("Blocked by Secure Boot: {why}"), rgb(218, 54, 51)));
                        return;
                    }
                }
                self.open_iso_entries(it.name);
            }
        }
    }

    /// The SHA-256 published for `name` in a checksum file next to it (`<name>.sha256`, `SHA256SUMS`, ...).
    /// `Err` says what was found instead, for the message shown to the user.
    fn published_sum(&self, name: &str) -> Result<String, String> {
        let mut seen: Option<(String, Vec<String>)> = None;
        for cand in checksum::sidecar_names(name) {
            let Ok(mut f) = self.vol.open_file(&self.path, &cand) else { continue };
            let size = f.size();
            if size == 0 || size > 512 * 1024 {
                seen.get_or_insert((cand, Vec::new()));
                continue;
            }
            let mut buf = alloc::vec![0u8; size as usize];
            if f.read_at(0, &mut buf).is_err() {
                continue;
            }
            let text = String::from_utf8_lossy(&buf);
            if let Some(h) = checksum::expected_in(&text, name) {
                return Ok(h);
            }
            seen.get_or_insert((cand, checksum::listed_names(&text)));
        }
        Err(match seen {
            None => format!("no checksum file found: put {name}.sha256 or SHA256SUMS next to the ISO to compare"),
            Some((file, names)) if names.is_empty() => format!("{file} found but it holds no checksum I can read"),
            Some((file, names)) => {
                let shown: Vec<&str> = names.iter().take(4).map(|s| s.as_str()).collect();
                format!("{file} has no entry for {name}; it lists: {}", shown.join(", "))
            }
        })
    }

    /// Hash the selected ISO (Esc cancels) and compare it with the checksum published next to it.
    fn verify(&mut self) {
        let Some(it) = self.items.get(self.sel).cloned() else { return };
        if it.kind != Kind::Iso || self.open_iso.is_some() {
            return;
        }
        let Ok(mut f) = self.vol.open_file(&self.path, &it.name) else {
            self.msg = Some((format!("Cannot open {}", it.name), rgb(218, 54, 51)));
            return;
        };
        let size = f.size();
        let expected = self.published_sum(&it.name);
        let mut last_pct = u64::MAX;
        let name = it.name.clone();
        let result = checksum::sha256_of(&mut f, size, |done| {
            let pct = done * 100 / size.max(1);
            if pct != last_pct {
                last_pct = pct;
                self.msg = Some((format!("Checking {name}: {pct}% ({} of {})   Esc cancels", fmt_size(done), fmt_size(size)), ACCENT));
                self.draw();
                self.scr.present();
                if matches!(uefi::system::with_stdin(|i| i.read_key()), Ok(Some(Key::Special(ScanCode::ESCAPE)))) {
                    return false;
                }
            }
            true
        });
        match result {
            Ok(Some(d)) => {
                let got = checksum::to_hex(&d);
                match expected {
                    Ok(want) if want == got => {
                        self.sums[self.sel] = Sum::Good;
                        self.msg = Some((format!("Checksum OK: this copy matches the published SHA-256 ({})", &got[..16]), rgb(46, 160, 67)));
                    }
                    Ok(want) => {
                        self.sums[self.sel] = Sum::Bad;
                        self.warned_bad = false;
                        self.msg = Some((format!("DAMAGED COPY - expected {want} but got {got}. Copy the ISO to the stick again."), rgb(218, 54, 51)));
                    }
                    Err(why) => {
                        self.msg = Some((format!("SHA-256 {got} ({why})"), DIM));
                    }
                }
            }
            Ok(None) => self.msg = Some(("Checksum check cancelled".into(), DIM)),
            Err(_) => {
                self.sums[self.sel] = Sum::Bad;
                self.msg = Some(("Read error while checking: the ISO or the stick is damaged".into(), rgb(218, 54, 51)));
            }
        }
    }

    fn mv(&mut self, delta: isize) {
        let max = self.items.len().saturating_sub(1) as isize;
        self.sel = (self.sel as isize + delta).clamp(0, max) as usize;
        self.warned_bad = false;
        self.clamp_scroll();
    }
}

/// Every pointing device the firmware offers. The console splitter's combined pointer does not forward events on every
/// firmware (VirtualBox, some laptops), so each device handle is opened and polled itself.
struct Pointers {
    rel: Vec<ScopedProtocol<Pointer>>,
    abs: Vec<ScopedProtocol<AbsolutePointer>>,
    /// USB mice read by us because the firmware has no driver for them.
    usb: Vec<hid::UsbPointer>,
}

fn open_pointers() -> Pointers {
    fn all<P: uefi::proto::ProtocolPointer + ?Sized + uefi::Identify>() -> Vec<ScopedProtocol<P>> {
        uefi::boot::find_handles::<P>()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|h| vol::open_shared::<P>(h).ok())
            .collect()
    }
    // Firmware often connects only the console devices (keyboards) at boot, so a mouse has no pointer protocol until its
    // driver is connected. Do what `connect -r` in the UEFI shell does (rEFInd does the same). Already-connected
    // controllers answer NOT_FOUND, which is fine.
    if let Ok(handles) = uefi::boot::locate_handle_buffer(uefi::boot::SearchType::AllHandles) {
        for &h in handles.iter() {
            let _ = uefi::boot::connect_controller(h, &[], None, true);
        }
    }
    Pointers { rel: all::<Pointer>(), abs: all::<AbsolutePointer>(), usb: hid::open_all() }
}

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    let Ok(scr) = Screen::new() else {
        uefi::println!("Toastrack: no graphics output available");
        uefi::boot::stall(Duration::from_secs(5));
        return Status::UNSUPPORTED;
    };
    let mut m = Menu {
        mouse: (scr.w as i32 / 2, scr.h as i32 / 2),
        open_iso: None,
        scr,
        vol: Rc::new(vol::find_data_volume()),
        path: Vec::new(),
        items: Vec::new(),
        badges: Vec::new(),
        sums: Vec::new(),
        warned_bad: false,
        sel: 0,
        scroll: 0,
        msg: None,
        sb_on: secure_boot_enabled(),
    };
    let mut ptr = open_pointers();
    m.reload();
    let mut was_down = false;
    let mut dirty = true;
    // F2 shows what the firmware offers for pointing devices (for reporting problems on odd firmware).
    let mut show_ptr = false;
    let (mut rel_events, mut abs_events, mut usb_events) = (0u32, 0u32, 0u32);
    let mut last_raw = String::new();
    loop {
        // Pointer input.
        let (mut down, mut moved) = (was_down, false);
        let mut button = None;
        for p in ptr.rel.iter_mut() {
            if let Ok(Some(s)) = p.read_state() {
                let step = |v: i32| if v == 0 { 0 } else { (v / 2).clamp(-60, 60).max(v.signum() * (v.abs().min(1))) };
                m.mouse.0 = (m.mouse.0 + step(s.relative_movement_x)).clamp(0, m.scr.w as i32 - 1);
                m.mouse.1 = (m.mouse.1 + step(s.relative_movement_y)).clamp(0, m.scr.h as i32 - 1);
                button = Some(button.unwrap_or(false) | bool::from(s.left_button));
                moved = true;
                rel_events += 1;
                last_raw = format!("rel dx={} dy={} left={}", s.relative_movement_x, s.relative_movement_y, bool::from(s.left_button));
            }
        }
        for p in ptr.abs.iter_mut() {
            let mode = *p.mode();
            if let Ok(Some(s)) = p.read_state() {
                let sx = (mode.absolute_max_x - mode.absolute_min_x).max(1);
                let sy = (mode.absolute_max_y - mode.absolute_min_y).max(1);
                m.mouse.0 = ((s.current_x.saturating_sub(mode.absolute_min_x)) * m.scr.w as u64 / sx).min(m.scr.w as u64 - 1) as i32;
                m.mouse.1 = ((s.current_y.saturating_sub(mode.absolute_min_y)) * m.scr.h as u64 / sy).min(m.scr.h as u64 - 1) as i32;
                button = Some(button.unwrap_or(false) | (s.active_buttons & 1 != 0));
                moved = true;
                abs_events += 1;
                last_raw = format!("abs x={} y={} buttons={:#x}", s.current_x, s.current_y, s.active_buttons);
            }
        }
        for p in ptr.usb.iter_mut() {
            if let Some(smp) = p.poll() {
                if p.layout.absolute {
                    let (lx, ly) = (p.layout.x, p.layout.y);
                    m.mouse.0 = ((smp.x - lx.min).max(0) as i64 * m.scr.w as i64 / (lx.max - lx.min).max(1) as i64).min(m.scr.w as i64 - 1) as i32;
                    m.mouse.1 = ((smp.y - ly.min).max(0) as i64 * m.scr.h as i64 / (ly.max - ly.min).max(1) as i64).min(m.scr.h as i64 - 1) as i32;
                } else {
                    m.mouse.0 = (m.mouse.0 + smp.x).clamp(0, m.scr.w as i32 - 1);
                    m.mouse.1 = (m.mouse.1 + smp.y).clamp(0, m.scr.h as i32 - 1);
                }
                button = Some(button.unwrap_or(false) | smp.left);
                moved = true;
                usb_events += 1;
                last_raw = format!("usb {} x={} y={} left={}", if p.layout.absolute { "tablet" } else { "mouse" }, smp.x, smp.y, smp.left);
            }
        }
        if let Some(b) = button {
            down = b;
        }
        if show_ptr {
            let ranges: Vec<String> = ptr.abs.iter().map(|p| { let md = p.mode(); format!("{}..{} x {}..{}", md.absolute_min_x, md.absolute_max_x, md.absolute_min_y, md.absolute_max_y) }).collect();
            m.msg = Some((format!("Pointers: {} relative, {} absolute [{}], {} USB read directly; events rel={} abs={} usb={}; last: {}", ptr.rel.len(), ptr.abs.len(), ranges.join(", "), ptr.usb.len(), rel_events, abs_events, usb_events, last_raw), DIM));
            dirty = true;
        }
        if moved {
            dirty = true;
        }
        if down && !was_down {
            if let Some(idx) = m.row_at(m.mouse) {
                if idx == m.sel {
                    m.activate();
                } else {
                    m.sel = idx;
                }
                dirty = true;
            }
        }
        was_down = down;

        // Keyboard input.
        if let Ok(Some(k)) = uefi::system::with_stdin(|i| i.read_key()) {
            dirty = true;
            m.msg = None;
            match k {
                Key::Special(ScanCode::UP) => m.mv(-1),
                Key::Special(ScanCode::DOWN) => m.mv(1),
                Key::Special(ScanCode::PAGE_UP) => m.mv(-(m.visible() as isize)),
                Key::Special(ScanCode::PAGE_DOWN) => m.mv(m.visible() as isize),
                Key::Special(ScanCode::HOME) => m.mv(-(m.items.len() as isize)),
                Key::Special(ScanCode::END) => m.mv(m.items.len() as isize),
                Key::Special(ScanCode::FUNCTION_2) => show_ptr = !show_ptr,
                Key::Special(ScanCode::ESCAPE) => {
                    if m.path.is_empty() && m.open_iso.is_none() {
                        return Status::SUCCESS;
                    }
                    m.up();
                }
                Key::Printable(c) if u16::from(c) == 13 => m.activate(),
                Key::Printable(c) if matches!(u16::from(c), 0x43 | 0x63) => m.verify(),
                _ => {}
            }
        }
        if dirty {
            m.draw();
            m.scr.present();
            dirty = false;
        }
        if ptr.usb.is_empty() {
            uefi::boot::stall(Duration::from_millis(8));
        }
    }
}

/// SBAT metadata (https://github.com/rhboot/shim/blob/main/SBAT.md). Shim refuses second-stage
/// loaders without it. Bump the generation number only when a security fix needs older builds revoked.
#[used]
#[link_section = ".sbat"]
static SBAT: [u8; SBAT_TEXT.len()] = {
    let (src, mut out, mut i) = (SBAT_TEXT.as_bytes(), [0u8; SBAT_TEXT.len()], 0);
    while i < out.len() {
        out[i] = src[i];
        i += 1;
    }
    out
};
const SBAT_TEXT: &str = "sbat,1,SBAT Version,sbat,1,https://github.com/rhboot/shim/blob/main/SBAT.md\n\
toastrack,1,Toastrack,toastrack,0.1.0,https://github.com/zvillanue/toastrack\n";

/// Some toolchains compile UCS-2 length loops (in the `uefi` crate's string handling) into a call to `wcslen`, which
/// this freestanding target does not provide. The volatile read keeps LLVM from turning this loop back into a call to itself.
#[no_mangle]
pub unsafe extern "C" fn wcslen(s: *const u16) -> usize {
    let mut n = 0;
    while core::ptr::read_volatile(s.add(n)) != 0 {
        n += 1;
    }
    n
}
