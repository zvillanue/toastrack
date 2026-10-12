//! `toastrack-attach.exe`: runs inside Windows PE / Windows Setup (started by `autounattend.xml`).
//!
//! Reads the UEFI variable the Toastrack menu left behind, finds the stick's data volume by its serial number,
//! and attaches the chosen ISO as a virtual DVD, so Setup finds `sources\install.wim` on it.
//! It never fails Setup: every problem is logged (to X:\ and to <stick>\toastrack\attach.log) and the exit code is 0.
//! See `toastrack_core::winreq` for the hand-off format.
//!
//! Built without the Rust standard library on purpose: it then imports only kernel32, advapi32, virtdisk and msvcrt,
//! all of which a minimal Windows PE has. (`std` would add userenv/ws2_32/bcryptprimitives.)
#![cfg_attr(windows, no_std)]
#![cfg_attr(windows, no_main)]

#[cfg(windows)]
extern crate alloc;

#[cfg(windows)]
mod imp {
    use alloc::{format, string::String, vec, vec::Vec};
    use core::{alloc::{GlobalAlloc, Layout}, ffi::c_void, ptr::{null, null_mut}};
    use toastrack_core::winreq::{pick_drive, Request, VAR_GUID, VAR_NAME};
    use windows_sys::Win32::{
        Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE, LUID},
        Security::{AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, SE_SYSTEM_ENVIRONMENT_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES},
        Storage::{
            FileSystem::{
                CreateDirectoryW, CreateFileW, GetFileAttributesExW, GetFileAttributesW, GetLogicalDrives, GetVolumeInformationW, ReadFile, WriteFile,
                FILE_APPEND_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileExInfoStandard, OPEN_ALWAYS, WIN32_FILE_ATTRIBUTE_DATA, FILE_ATTRIBUTE_NORMAL,
            },
            Vhd::{
                AttachVirtualDisk, OpenVirtualDisk, ATTACH_VIRTUAL_DISK_FLAG_PERMANENT_LIFETIME, ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY,
                OPEN_VIRTUAL_DISK_FLAG_NONE, VIRTUAL_DISK_ACCESS_ATTACH_RO, VIRTUAL_STORAGE_TYPE, VIRTUAL_STORAGE_TYPE_DEVICE_ISO,
                VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
            },
        },
        System::{
            Memory::{GetProcessHeap, HeapAlloc, HeapFree},
            Environment::GetEnvironmentVariableW,
            Threading::{GetCurrentProcess, OpenProcessToken, Sleep},
            WindowsProgramming::GetFirmwareEnvironmentVariableW,
        },
    };

    struct HeapAllocator;

    // SAFETY: HeapAlloc/HeapFree on the process heap are thread-safe; alignment above 16 is not requested by this program.
    unsafe impl GlobalAlloc for HeapAllocator {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            unsafe { HeapAlloc(GetProcessHeap(), 0, l.size()) as *mut u8 }
        }
        unsafe fn dealloc(&self, p: *mut u8, _: Layout) {
            unsafe { HeapFree(GetProcessHeap(), 0, p as *const c_void) };
        }
    }

    #[global_allocator]
    static ALLOC: HeapAllocator = HeapAllocator;

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! {
        loop {
            // SAFETY: Sleep has no preconditions. (We never panic on purpose; hanging beats crashing Setup.)
            unsafe { Sleep(1000) };
        }
    }

    /// The precompiled core library names an unwinding personality routine even though this program is built with
    /// panic=abort and never unwinds; the linker needs a definition.
    #[no_mangle]
    pub extern "C" fn rust_eh_personality() {}

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(core::iter::once(0)).collect()
    }

    fn exists(path: &str) -> bool {
        // SAFETY: valid NUL-terminated wide string.
        unsafe { GetFileAttributesW(wide(path).as_ptr()) != u32::MAX }
    }

    fn size_of(path: &str) -> Option<u64> {
        let mut d: WIN32_FILE_ATTRIBUTE_DATA = unsafe { core::mem::zeroed() };
        // SAFETY: valid path and out pointer.
        let ok = unsafe { GetFileAttributesExW(wide(path).as_ptr(), GetFileExInfoStandard, &mut d as *mut _ as *mut c_void) };
        (ok != 0).then(|| (d.nFileSizeHigh as u64) << 32 | d.nFileSizeLow as u64)
    }

    fn append(path: &str, text: &str) {
        // SAFETY: valid path; the handle is closed by process exit (best-effort logging).
        unsafe {
            let h = CreateFileW(wide(path).as_ptr(), FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, null_mut());
            if h == INVALID_HANDLE_VALUE {
                return;
            }
            let mut written = 0u32;
            WriteFile(h, text.as_ptr(), text.len() as u32, &mut written, null_mut());
            windows_sys::Win32::Foundation::CloseHandle(h);
        }
    }

    struct Log(Vec<String>);

    impl Log {
        fn add(&mut self, s: impl Into<String>) {
            let s = s.into();
            append(r"X:\toastrack-attach.log", &format!("{s}\r\n"));
            self.0.push(s);
        }
        /// Copy the whole log to the stick, where it can be read after the test.
        fn to_stick(&self, drive: char) {
            let dir = format!(r"{drive}:\toastrack");
            // SAFETY: valid path; failure (already exists) is fine.
            unsafe { CreateDirectoryW(wide(&dir).as_ptr(), null()) };
            let mut text = String::from("---- run ----\r\n");
            for l in &self.0 {
                text.push_str(l);
                text.push_str("\r\n");
            }
            append(&format!(r"{dir}\attach.log"), &text);
        }
    }

    fn enable_firmware_privilege() -> Result<(), u32> {
        // SAFETY: plain Win32 calls on locals.
        unsafe {
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token) == 0 {
                return Err(GetLastError());
            }
            let mut luid = LUID { LowPart: 0, HighPart: 0 };
            if LookupPrivilegeValueW(null(), SE_SYSTEM_ENVIRONMENT_NAME, &mut luid) == 0 {
                return Err(GetLastError());
            }
            let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
            if AdjustTokenPrivileges(token, 0, &tp, 0, null_mut(), null_mut()) == 0 {
                return Err(GetLastError());
            }
        }
        Ok(())
    }

    /// Test hook: if the environment variable TOASTRACK_TEST_REQUEST names a file, read the request from it instead of
    /// the firmware variable (lets the rest of the program be exercised without UEFI). Never set in Windows PE.
    fn read_test_request() -> Option<Result<Request, String>> {
        let mut path = vec![0u16; 520];
        // SAFETY: valid buffer and NUL-terminated name.
        let n = unsafe { GetEnvironmentVariableW(wide("TOASTRACK_TEST_REQUEST").as_ptr(), path.as_mut_ptr(), path.len() as u32) };
        if n == 0 || n as usize >= path.len() {
            return None;
        }
        let path = String::from_utf16_lossy(&path[..n as usize]);
        // SAFETY: valid path; plain file read of a small file.
        let data = unsafe {
            let h = CreateFileW(wide(&path).as_ptr(), 0x8000_0000, FILE_SHARE_READ, null(), 3, FILE_ATTRIBUTE_NORMAL, null_mut());
            if h == INVALID_HANDLE_VALUE {
                return Some(Err(format!("cannot open the test request file {path}")));
            }
            let mut buf = vec![0u8; 4096];
            let mut got = 0u32;
            ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut got, null_mut());
            windows_sys::Win32::Foundation::CloseHandle(h);
            buf.truncate(got as usize);
            buf
        };
        Some(Request::decode(&data).map_err(|e| format!("test request is malformed: {e:?}")))
    }

    fn read_request() -> Result<Request, String> {
        if let Some(r) = read_test_request() {
            return r;
        }
        let name = wide(VAR_NAME);
        let guid = wide(&format!("{{{VAR_GUID}}}"));
        let mut buf = vec![0u8; 4096];
        // SAFETY: buffer and name pointers are valid for the call.
        let n = unsafe { GetFirmwareEnvironmentVariableW(name.as_ptr(), guid.as_ptr(), buf.as_mut_ptr().cast(), buf.len() as u32) };
        if n == 0 {
            // SAFETY: trivially safe.
            return Err(format!("variable {VAR_NAME} not readable (Win32 error {})", unsafe { GetLastError() }));
        }
        Request::decode(&buf[..n as usize]).map_err(|e| format!("variable {VAR_NAME} is malformed: {e:?}"))
    }

    fn volumes() -> Vec<(char, u32)> {
        // SAFETY: plain Win32 calls with valid pointers.
        let mask = unsafe { GetLogicalDrives() };
        let mut v = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root = wide(&format!(r"{letter}:\"));
            let mut serial = 0u32;
            let ok = unsafe { GetVolumeInformationW(root.as_ptr(), null_mut(), 0, &mut serial, null_mut(), null_mut(), null_mut(), 0) };
            if ok != 0 {
                v.push((letter, serial));
            }
        }
        v
    }

    fn attach(path: &str) -> Result<(), u32> {
        let ty = VIRTUAL_STORAGE_TYPE { DeviceId: VIRTUAL_STORAGE_TYPE_DEVICE_ISO, VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT };
        let p = wide(path);
        let mut h: HANDLE = null_mut();
        // SAFETY: valid pointers; the parameter structs are optional (null).
        unsafe {
            let e = OpenVirtualDisk(&ty, p.as_ptr(), VIRTUAL_DISK_ACCESS_ATTACH_RO, OPEN_VIRTUAL_DISK_FLAG_NONE, null(), &mut h);
            if e != 0 {
                return Err(e);
            }
            // PERMANENT_LIFETIME keeps the disk attached after this process (and the handle) are gone.
            let e = AttachVirtualDisk(h, null_mut(), ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY | ATTACH_VIRTUAL_DISK_FLAG_PERMANENT_LIFETIME, 0, null(), null());
            if e != 0 {
                return Err(e);
            }
        }
        Ok(())
    }

    fn sleep_s(s: u32) {
        // SAFETY: Sleep has no preconditions.
        unsafe { Sleep(s * 1000) };
    }

    pub fn run() {
        let mut log = Log(Vec::new());
        log.add("toastrack-attach started");
        match enable_firmware_privilege() {
            Ok(()) => log.add("firmware-variable privilege enabled"),
            Err(e) => log.add(format!("could not enable the firmware-variable privilege (Win32 error {e}); trying anyway")),
        }
        let req = match read_request() {
            Ok(r) => r,
            Err(e) => {
                // Normal when the stick is used without the Toastrack menu: do nothing.
                log.add(format!("nothing to do: {e}"));
                return;
            }
        };
        log.add(format!("request: volume serial {:08X}, ISO {}", req.serial, req.path));

        // USB volumes can take a while to appear in WinPE: wait up to a minute.
        let mut drive = None;
        for attempt in 0..60 {
            let v = volumes();
            if let Some(d) = pick_drive(&v, req.serial) {
                drive = Some(d);
                break;
            }
            if attempt == 0 {
                let seen: Vec<String> = v.iter().map(|(d, s)| format!("{d}:={s:08X}")).collect();
                log.add(format!("volumes seen: {}", seen.join(" ")));
            }
            sleep_s(1);
        }
        let Some(drive) = drive else {
            log.add("the stick's data volume did not appear (no drive letter with that serial number)");
            return;
        };
        log.add(format!("data volume is {drive}:"));
        let iso = req.windows_path(drive);
        match size_of(&iso) {
            Some(n) => log.add(format!("ISO found: {iso} ({n} bytes)")),
            None => {
                log.add(format!("ISO not found at {iso}"));
                log.to_stick(drive);
                return;
            }
        }
        let before: Vec<char> = volumes().into_iter().map(|(d, _)| d).collect();
        match attach(&iso) {
            Err(e) => log.add(format!("attaching the ISO failed (Win32 error {e})")),
            Ok(()) => {
                log.add("ISO attached; waiting for its drive letter");
                for _ in 0..30 {
                    let new: Vec<char> = volumes().into_iter().map(|(d, _)| d).filter(|d| !before.contains(d)).collect();
                    if let Some(d) = new.iter().find(|d| exists(&format!(r"{d}:\sources"))) {
                        log.add(format!("install media is now {d}:"));
                        break;
                    }
                    sleep_s(1);
                }
            }
        }
        log.to_stick(drive);
    }
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn main(_argc: i32, _argv: *const *const u8) -> i32 {
    imp::run();
    0
}

#[cfg(not(windows))]
fn main() {
    eprintln!("toastrack-attach runs inside Windows Setup (WinPE); it has nothing to do on this system.");
}
