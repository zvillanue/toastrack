use std::{fs::File, io::BufReader, path::PathBuf, process::ExitCode};
use toastrack_core::{analyze_iso, iso::Iso, SecureBootStatus};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("check") || args.len() < 2 {
        eprintln!("usage: toastrack check <image.iso>...");
        return ExitCode::from(2);
    }
    let mut ok = true;
    for p in args[1..].iter().map(PathBuf::from) {
        let status = File::open(&p)
            .map_err(|e| e.to_string())
            .and_then(|f| Iso::open(BufReader::new(f)).map_err(|e| e.to_string()))
            .and_then(|mut iso| analyze_iso(&mut iso).map_err(|e| e.to_string()));
        let label = match &status {
            Ok(SecureBootStatus::Trusted { .. }) => "OK       secure-boot ready".to_string(),
            Ok(SecureBootStatus::NeedsEnrollment { signers }) => {
                format!("ENROLL   needs key enrollment ({})", signers.join("; "))
            }
            Ok(SecureBootStatus::Unsigned) => "BLOCKED  unsigned loader".to_string(),
            Ok(SecureBootStatus::NoUefiLoader) => "UNKNOWN  no BOOTX64.EFI found".to_string(),
            Err(e) => format!("ERROR    {e}"),
        };
        if let Ok(f) = File::open(&p) {
            if let Ok(mut iso) = Iso::open(BufReader::new(f)) {
                for e in toastrack_core::bootcfg::entries(&mut iso).unwrap_or_default() {
                    println!("         boot entry: {:?} kernel={} initrd={:?}", e.title, e.kernel, e.initrds);
                }
            }
        }
        ok &= matches!(status, Ok(SecureBootStatus::Trusted { .. }));
        println!("{label}  {}", p.display());
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::from(1) }
}
