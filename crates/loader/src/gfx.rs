//! Software-rendered UI on top of the Graphics Output Protocol.

use alloc::vec::Vec;
use font8x8::legacy::BASIC_LEGACY;
use uefi::{
    boot::{self, ScopedProtocol},
    proto::console::gop::{BltOp, BltPixel, BltRegion, GraphicsOutput},
};

pub type Color = BltPixel;
pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    BltPixel::new(r, g, b)
}

pub struct Screen {
    gop: ScopedProtocol<GraphicsOutput>,
    pub w: usize,
    pub h: usize,
    buf: Vec<Color>,
}

const CURSOR: [&str; 16] = [
    "X           ", "XX          ", "XOX         ", "XOOX        ", "XOOOX       ", "XOOOOX      ", "XOOOOOX     ",
    "XOOOOOOX    ", "XOOOOXXXX   ", "XOOXOX      ", "XOX XOX     ", "XX  XOX     ", "X    XOX    ", "     XOX    ",
    "      XX    ", "            ",
];

impl Screen {
    pub fn new() -> uefi::Result<Self> {
        let h = boot::get_handle_for_protocol::<GraphicsOutput>()?;
        // SAFETY: shared access only; console drivers keep their own use of the GOP.
        let mut gop = unsafe {
            boot::open_protocol::<GraphicsOutput>(
                boot::OpenProtocolParams { handle: h, agent: boot::image_handle(), controller: None },
                boot::OpenProtocolAttributes::GetProtocol,
            )?
        };
        // Largest mode not exceeding 1280x1024 (keeps software blits quick).
        let best = gop
            .modes()
            .filter(|m| {
                let (w, h) = m.info().resolution();
                w <= 1280 && h <= 1024
            })
            .max_by_key(|m| m.info().resolution().0 * m.info().resolution().1);
        if let Some(m) = best {
            let _ = gop.set_mode(&m);
        }
        let (w, h) = gop.current_mode_info().resolution();
        Ok(Self { gop, w, h, buf: alloc::vec![rgb(0, 0, 0); w * h] })
    }

    pub fn clear(&mut self, c: Color) {
        self.buf.fill(c);
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color) {
        let (x0, y0) = (x.max(0) as usize, y.max(0) as usize);
        let (x1, y1) = (((x + w).max(0) as usize).min(self.w), ((y + h).max(0) as usize).min(self.h));
        for yy in y0..y1 {
            self.buf[yy * self.w + x0..yy * self.w + x1.max(x0)].fill(c);
        }
    }

    pub fn text_width(s: &str, scale: usize) -> i32 {
        (s.chars().count() * 8 * scale) as i32
    }

    pub fn text(&mut self, x: i32, y: i32, s: &str, scale: usize, c: Color) {
        let mut cx = x;
        for ch in s.chars() {
            let glyph = BASIC_LEGACY.get(if ch.is_ascii() { ch as usize } else { b'?' as usize }).copied().unwrap_or([0; 8]);
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits >> col & 1 == 1 {
                        self.rect(cx + (col * scale) as i32, y + (row * scale) as i32, scale as i32, scale as i32, c);
                    }
                }
            }
            cx += (8 * scale) as i32;
        }
    }

    pub fn cursor(&mut self, x: i32, y: i32) {
        for (r, row) in CURSOR.iter().enumerate() {
            for (cc, ch) in row.bytes().enumerate() {
                let (px, py) = (x + cc as i32, y + r as i32);
                if px < 0 || py < 0 || px as usize >= self.w || py as usize >= self.h {
                    continue;
                }
                match ch {
                    b'X' => self.buf[py as usize * self.w + px as usize] = rgb(0, 0, 0),
                    b'O' => self.buf[py as usize * self.w + px as usize] = rgb(255, 255, 255),
                    _ => {}
                }
            }
        }
    }

    pub fn present(&mut self) {
        let _ = self.gop.blt(BltOp::BufferToVideo {
            buffer: &self.buf,
            src: BltRegion::Full,
            dest: (0, 0),
            dims: (self.w, self.h),
        });
    }
}
