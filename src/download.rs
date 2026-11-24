//! Pinned downloads: fetch a file with curl from an https address and refuse it unless its SHA-256 is the pinned one.
//! A file that does not match is deleted and never used. (The same approach Padyx uses for its drivers.)

#![cfg_attr(not(windows), allow(dead_code))]

use crate::selfupdate::sha256_file;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadError {
    /// curl is missing or the download failed.
    Failed(String),
    /// The file is not the pinned one; it has been deleted.
    HashMismatch { expected: String, got: String },
    /// The address is not https (only tests may use http).
    NotHttps,
    /// The pin is empty: nothing is downloaded or run without a checksum.
    NotPinned,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Failed(e) => write!(f, "download failed: {e}"),
            DownloadError::HashMismatch { expected, got } => write!(f, "the file does not match the pinned checksum (expected {expected}, got {got}); it was deleted and not used"),
            DownloadError::NotHttps => write!(f, "the address is not https"),
            DownloadError::NotPinned => write!(f, "no pinned checksum for this file"),
        }
    }
}

/// Checks a file against a pinned hash, deleting it when it does not match.
pub fn verify(file: &Path, sha256: &str) -> Result<(), DownloadError> {
    if sha256.len() != 64 {
        let _ = std::fs::remove_file(file);
        return Err(DownloadError::NotPinned);
    }
    let got = sha256_file(file).map_err(|e| DownloadError::Failed(e.to_string()))?;
    if got.eq_ignore_ascii_case(sha256) {
        Ok(())
    } else {
        let _ = std::fs::remove_file(file);
        Err(DownloadError::HashMismatch { expected: sha256.to_lowercase(), got })
    }
}

/// Downloads `url` to `dest` and checks it. `allow_http` is for the tests with a local server only.
pub fn download_verified(url: &str, dest: &Path, sha256: &str, max_bytes: u64, allow_http: bool) -> Result<(), DownloadError> {
    if sha256.len() != 64 {
        return Err(DownloadError::NotPinned);
    }
    if !allow_http && !url.starts_with("https://") {
        return Err(DownloadError::NotHttps);
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| DownloadError::Failed(e.to_string()))?;
    }
    let _ = std::fs::remove_file(dest);
    let mut cmd = std::process::Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    cmd.args(["--fail", "--silent", "--show-error", "--location", "--max-redirs", "5", "--max-time", "600", "--max-filesize", &max_bytes.to_string()]);
    if allow_http {
        cmd.args(["--noproxy", "*"]);
    } else {
        cmd.args(["--proto", "=https", "--proto-redir", "=https"]);
    }
    cmd.arg("--output").arg(dest).arg(url);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.stdin(std::process::Stdio::null()).output().map_err(|e| DownloadError::Failed(format!("curl: {e}")))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(dest);
        return Err(DownloadError::Failed(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    verify(dest, sha256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("thermora-dl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A one-shot local http server that answers a single request with `body`.
    fn serve(body: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}/file.bin")
    }

    #[test]
    fn a_matching_file_is_accepted() {
        let body = b"signed installer bytes".to_vec();
        let want = crate::selfupdate::sha256_hex(&body);
        let dir = tmp("ok");
        let dest = dir.join("a.bin");
        download_verified(&serve(body.clone()), &dest, &want, 1 << 20, true).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_wrong_hash_is_refused_and_deleted() {
        let dir = tmp("bad");
        let dest = dir.join("a.bin");
        let wrong = "0".repeat(64);
        let e = download_verified(&serve(b"tampered".to_vec()), &dest, &wrong, 1 << 20, true).unwrap_err();
        assert!(matches!(e, DownloadError::HashMismatch { .. }));
        assert!(!dest.exists(), "the file must be gone");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nothing_downloads_without_a_pin_or_over_http() {
        let dir = tmp("pin");
        let dest = dir.join("a.bin");
        assert_eq!(download_verified("https://example.invalid/x", &dest, "", 10, false), Err(DownloadError::NotPinned));
        assert_eq!(download_verified("http://example.invalid/x", &dest, &"a".repeat(64), 10, false), Err(DownloadError::NotHttps));
        let _ = std::fs::remove_dir_all(dir);
    }
}
