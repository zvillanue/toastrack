//! USB mice and tablets driven directly through EFI_USB_IO, for firmware that never starts a pointer driver for them
//! (the console splitter only forwards pointers it was told about, so apps then see a pointer that never moves).

use alloc::{vec, vec::Vec};
use toastrack_core::hid::{parse_mouse, MouseLayout, Sample};
use uefi::{
    boot::ScopedProtocol,
    proto::{
        console::pointer::{AbsolutePointer, Pointer},
        usb::io::{ControlTransfer, UsbIo},
    },
};

/// How long one poll waits for a report. Mice send at 125-1000 Hz only while they move, so this is also the cost of an
/// idle poll; the menu loop runs at about this rate.
const POLL_MS: usize = 10;

pub struct UsbPointer {
    io: ScopedProtocol<UsbIo>,
    endpoint: u8,
    len: usize,
    pub layout: MouseLayout,
}

impl UsbPointer {
    pub fn poll(&mut self) -> Option<Sample> {
        let mut buf = [0u8; 64];
        let n = self.len.clamp(1, buf.len());
        let got = self.io.sync_interrupt_receive(self.endpoint, &mut buf[..n], POLL_MS).ok()?;
        self.layout.decode(&buf[..got.min(n)])
    }
}

/// Every HID interface that describes a pointer and has no firmware pointer driver on it.
pub fn open_all() -> Vec<UsbPointer> {
    let mut out = Vec::new();
    for h in uefi::boot::find_handles::<UsbIo>().unwrap_or_default() {
        // A firmware driver already owns this interface: polling its endpoint too would steal its reports.
        if crate::vol::open_shared::<Pointer>(h).is_ok() || crate::vol::open_shared::<AbsolutePointer>(h).is_ok() {
            continue;
        }
        let Ok(mut io) = crate::vol::open_shared::<UsbIo>(h) else { continue };
        let Ok(iface) = io.interface_descriptor() else { continue };
        if iface.interface_class != 3 {
            continue;
        }
        let mut ep = None;
        for n in 0..iface.num_endpoints {
            if let Ok(e) = io.endpoint_descriptor(n) {
                if e.attributes & 3 == 3 && e.endpoint_address & 0x80 != 0 {
                    ep = Some((e.endpoint_address & 0x0F, e.max_packet_size as usize));
                    break;
                }
            }
        }
        let Some((endpoint, len)) = ep else { continue };
        // GET_DESCRIPTOR(HID report) on this interface; the tail of the buffer stays zero.
        // Some firmware reports an error for the short reply (the descriptor is shorter than the buffer) even though the
        // data arrived, so the result is ignored and whatever was received is parsed (a zero byte ends the descriptor).
        let mut desc = vec![0u8; 256];
        let _ = io.control_transfer(0x01, 0x06, 0x2200, iface.interface_number as u16, ControlTransfer::DataIn(&mut desc), 1000);
        let Some(layout) = parse_mouse(&desc) else { continue };
        out.push(UsbPointer { io, endpoint, len, layout });
    }
    out
}
