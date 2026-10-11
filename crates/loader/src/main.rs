#![no_main]
#![no_std]

extern crate alloc;

mod boot;
mod gfx;
mod vdisk;
mod vol;

use alloc::{format, string::String, vec::Vec};
use core::time::Duration;
use gfx::{rgb, Color, Screen};
use toastrack_core::{analyze_iso, iso::Iso, SecureBootStatus};
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

struct Menu {
    scr: Screen,
    vol: Handle,
    path: Vec<String>,
    items: Vec<Item>,
    badges: Vec<Badge>,
    sel: usize,
    scroll: usize,
    msg: Option<(String, Color)>,
    sb_on: bool,
    mouse: (i32, i32),
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
        self.items = vol::list(self.vol, &self.path).unwrap_or_default();
        self.badges = alloc::vec![Badge::None; self.items.len()];
        self.sel = 0;
        self.scroll = 0;
        for i in 0..self.items.len() {
            if self.items[i].kind != Kind::Iso {
                continue;
            }
            self.msg = Some((format!("Scanning {} ...", self.items[i].name), DIM));
            self.draw();
            self.scr.present();
            self.badges[i] = match vol::open_file(self.vol, &self.path, &self.items[i].name)
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
        let crumb = format!("/{}", self.path.join("/"));
        self.scr.text(24, 72, &crumb, 2, DIM);

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
                Kind::Iso => ("ISO", ACCENT),
            };
            self.scr.text(24, y + 8, tag, 2, tagc);
            let name = if it.kind == Kind::Dir { format!("{}/", it.name) } else { it.name.clone() };
            self.scr.text(88, y + 8, &name, 2, FG);
            if it.kind == Kind::Iso {
                let (label, c) = Self::badge(&self.badges[idx]);
                let lw = Screen::text_width(label, 2) + 16;
                let bx = w - 24 - lw;
                self.scr.rect(bx, y + 4, lw, ROW - 10, c);
                self.scr.text(bx + 8, y + 8, label, 2, rgb(10, 10, 10));
                let sz = fmt_size(it.size);
                self.scr.text(bx - 24 - Screen::text_width(&sz, 2), y + 8, &sz, 2, DIM);
            }
        }
        self.scr.rect(0, h - FOOT, w, FOOT, PANEL);
        let hint = "Up/Down select   Enter open/boot   Esc back   Mouse: click to select, click again to open";
        self.scr.text(24, h - FOOT + 8, hint, 1, DIM);
        if let Some((m, c)) = self.msg.clone() {
            self.scr.text(24, h - FOOT + 28, &m, 2, c);
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
        if self.path.pop().is_some() {
            self.reload();
        }
    }

    fn activate(&mut self) {
        let Some(it) = self.items.get(self.sel).cloned() else { return };
        match it.kind {
            Kind::Up => self.up(),
            Kind::Dir => {
                self.path.push(it.name);
                self.reload();
            }
            Kind::Iso => {
                if let Badge::Status(s @ (SecureBootStatus::Unsigned | SecureBootStatus::NoUefiLoader)) = &self.badges[self.sel] {
                    if self.sb_on {
                        let why = if *s == SecureBootStatus::Unsigned { "unsigned loader" } else { "no UEFI loader" };
                        self.msg = Some((format!("Blocked by Secure Boot: {why}"), rgb(218, 54, 51)));
                        return;
                    }
                }
                self.msg = Some((format!("Booting {} ...", it.name), ACCENT));
                self.draw();
                self.scr.present();
                if let Err(e) = boot::boot_iso(self.vol, &self.path, &it.name) {
                    self.msg = Some((e, rgb(218, 54, 51)));
                }
            }
        }
    }

    fn mv(&mut self, delta: isize) {
        let max = self.items.len().saturating_sub(1) as isize;
        self.sel = (self.sel as isize + delta).clamp(0, max) as usize;
        self.clamp_scroll();
    }
}

struct Pointers {
    rel: Option<ScopedProtocol<Pointer>>,
    abs: Option<ScopedProtocol<AbsolutePointer>>,
}

fn open_pointers() -> Pointers {
    let rel = uefi::boot::get_handle_for_protocol::<Pointer>().and_then(vol::open_shared::<Pointer>).ok();
    let abs = uefi::boot::get_handle_for_protocol::<AbsolutePointer>().and_then(vol::open_shared::<AbsolutePointer>).ok();
    Pointers { rel, abs }
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
        scr,
        vol: vol::find_data_volume(),
        path: Vec::new(),
        items: Vec::new(),
        badges: Vec::new(),
        sel: 0,
        scroll: 0,
        msg: None,
        sb_on: secure_boot_enabled(),
    };
    let mut ptr = open_pointers();
    m.reload();
    let mut was_down = false;
    let mut dirty = true;
    loop {
        // Pointer input.
        let (mut down, mut moved) = (was_down, false);
        if let Some(p) = ptr.rel.as_mut() {
            if let Ok(Some(s)) = p.read_state() {
                let step = |v: i32| if v == 0 { 0 } else { (v / 2).clamp(-60, 60).max(v.signum() * (v.abs().min(1))) };
                m.mouse.0 = (m.mouse.0 + step(s.relative_movement_x)).clamp(0, m.scr.w as i32 - 1);
                m.mouse.1 = (m.mouse.1 + step(s.relative_movement_y)).clamp(0, m.scr.h as i32 - 1);
                down = s.left_button.into();
                moved = true;
            }
        }
        if let Some(p) = ptr.abs.as_mut() {
            let mode = *p.mode();
            if let Ok(Some(s)) = p.read_state() {
                let sx = (mode.absolute_max_x - mode.absolute_min_x).max(1);
                let sy = (mode.absolute_max_y - mode.absolute_min_y).max(1);
                m.mouse.0 = ((s.current_x - mode.absolute_min_x) * m.scr.w as u64 / sx) as i32;
                m.mouse.1 = ((s.current_y - mode.absolute_min_y) * m.scr.h as u64 / sy) as i32;
                down = s.active_buttons & 1 != 0;
                moved = true;
            }
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
                Key::Special(ScanCode::ESCAPE) => {
                    if m.path.is_empty() {
                        return Status::SUCCESS;
                    }
                    m.up();
                }
                Key::Printable(c) if u16::from(c) == 13 => m.activate(),
                _ => {}
            }
        }
        if dirty {
            m.draw();
            m.scr.present();
            dirty = false;
        }
        uefi::boot::stall(Duration::from_millis(8));
    }
}
