#![no_main]
#![no_std]

extern crate alloc;

use uefi::prelude::*;

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    uefi::println!("Toastrack loader (skeleton)");
    boot::stall(core::time::Duration::from_secs(3));
    Status::SUCCESS
}
