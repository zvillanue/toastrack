//! Minimal PE/COFF reader: Authenticode hash and signer names.

use cms::{content_info::ContentInfo, signed_data::SignedData};
use der::{Decode, Encode};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum PeError {
    #[error("not a PE image")]
    NotPe,
    #[error("truncated PE image")]
    Truncated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeInfo {
    pub machine: u16,
    /// SHA-256 Authenticode digest (what dbx entries are matched against).
    pub authenticode_sha256: [u8; 32],
    /// Subject strings of every certificate in the embedded signature, if any.
    pub signer_subjects: Vec<String>,
    pub signed: bool,
}

pub const MACHINE_X64: u16 = 0x8664;

fn u16le(d: &[u8], o: usize) -> Result<u16, PeError> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or(PeError::Truncated)
}
fn u32le(d: &[u8], o: usize) -> Result<u32, PeError> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(PeError::Truncated)
}

pub fn inspect(data: &[u8]) -> Result<PeInfo, PeError> {
    if data.get(..2) != Some(b"MZ") {
        return Err(PeError::NotPe);
    }
    let pe = u32le(data, 0x3c)? as usize;
    if data.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err(PeError::NotPe);
    }
    let machine = u16le(data, pe + 4)?;
    let opt = pe + 24;
    let magic = u16le(data, opt)?;
    let (dd_off, _) = match magic {
        0x20b => (opt + 112, ()), // PE32+
        0x10b => (opt + 96, ()),  // PE32
        _ => return Err(PeError::NotPe),
    };
    let checksum_off = opt + 64;
    let cert_dir = dd_off + 4 * 8;
    let (cert_off, cert_len) = (u32le(data, cert_dir)? as usize, u32le(data, cert_dir + 4)? as usize);

    // Authenticode hash: skip checksum field, the cert directory entry, and the cert table.
    let end_of_image = if cert_off != 0 { cert_off } else { data.len() };
    if end_of_image > data.len() {
        return Err(PeError::Truncated);
    }
    let mut h = Sha256::new();
    h.update(&data[..checksum_off]);
    h.update(&data[checksum_off + 4..cert_dir]);
    h.update(&data[cert_dir + 8..end_of_image]);
    // Authenticode pads to 8 bytes only when hashing unsigned content; keep image as-is.
    let authenticode_sha256: [u8; 32] = h.finalize().into();

    let mut signer_subjects = Vec::new();
    let mut signed = false;
    if cert_off != 0 && cert_len >= 8 {
        // WIN_CERTIFICATE: u32 length, u16 revision, u16 type, then PKCS#7 blob.
        let blob_len = u32le(data, cert_off)? as usize;
        let blob = data
            .get(cert_off + 8..cert_off + blob_len.min(cert_len))
            .ok_or(PeError::Truncated)?;
        if let Ok(ci) = ContentInfo::from_der(blob) {
            if let Ok(der_sd) = ci.content.to_der() {
                if let Ok(sd) = SignedData::from_der(&der_sd) {
                    signed = true;
                    if let Some(certs) = sd.certificates {
                        for c in certs.0.iter() {
                            if let cms::cert::CertificateChoices::Certificate(c) = c {
                                signer_subjects.push(c.tbs_certificate.subject.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(PeInfo { machine, authenticode_sha256, signer_subjects, signed })
}
