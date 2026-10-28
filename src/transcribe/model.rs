//! Whisper model files (ggml format from whisper.cpp on Hugging Face).

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

pub const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
/// First four bytes of every ggml model file ("ggml" as a little-endian u32).
const MAGIC: &[u8; 4] = b"lmgg";

/// (name, approximate download size in MB)
const MODELS: [(&str, u64); 4] = [
    ("tiny", 75),
    ("base", 142),
    ("small", 466),
    ("medium", 1500),
];

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("unknown model \"{0}\" (use tiny, base, small or medium)")]
    Unknown(String),
    #[error("download failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server answered with status {0}")]
    Status(u16),
    #[error("file error: {0}")]
    Io(#[from] io::Error),
    #[error("the downloaded file is not a Whisper model")]
    NotAModel,
    #[error("the download was cut short ({got} of {expected} bytes)")]
    Incomplete { got: u64, expected: u64 },
}

/// Approximate download size in MB, or `None` for an unknown model name.
pub fn size_mb(name: &str) -> Option<u64> {
    MODELS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

pub fn file_name(name: &str) -> String {
    format!("ggml-{name}.bin")
}

pub fn url(base: &str, name: &str) -> String {
    format!("{base}/{}", file_name(name))
}

pub fn path(models_dir: &Path, name: &str) -> PathBuf {
    models_dir.join(file_name(name))
}

/// The model file is there (it was verified when it was downloaded).
pub fn is_installed(models_dir: &Path, name: &str) -> bool {
    path(models_dir, name).is_file()
}

/// Download `url` to `dest` (via a `.part` file, so a broken download never
/// looks like a model). `progress(done, total)` is called as data arrives;
/// `total` is 0 if the server does not say.
pub fn download(
    url: &str,
    dest: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), ModelError> {
    // No overall timeout: models are big.
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("synq/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(None::<Duration>)
        .build()?;
    let mut resp = client.get(url).send()?;
    if !resp.status().is_success() {
        return Err(ModelError::Status(resp.status().as_u16()));
    }
    let total = resp.content_length().unwrap_or(0);
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let part = dest.with_extension("bin.part");
    let result = (|| -> Result<(), ModelError> {
        let mut out = File::create(&part)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        let mut head = Vec::with_capacity(4);
        loop {
            let n = resp.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if head.len() < 4 {
                head.extend_from_slice(&buf[..n.min(4 - head.len())]);
            }
            out.write_all(&buf[..n])?;
            done += n as u64;
            progress(done, total);
        }
        out.flush()?;
        if head != MAGIC {
            return Err(ModelError::NotAModel);
        }
        if total > 0 && done != total {
            return Err(ModelError::Incomplete {
                got: done,
                expected: total,
            });
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            fs::rename(&part, dest)?;
            Ok(())
        }
        Err(e) => {
            // Our own partial file; nothing worth keeping.
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::serve_once;

    fn fake_model(len: usize) -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        v.resize(len, 7);
        v
    }

    #[test]
    fn known_models_and_their_urls() {
        assert_eq!(size_mb("base"), Some(142));
        assert_eq!(size_mb("huge"), None);
        assert_eq!(file_name("tiny"), "ggml-tiny.bin");
        assert_eq!(
            url(BASE_URL, "small"),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
        );
        assert_eq!(
            path(Path::new("data/models"), "base"),
            Path::new("data/models").join("ggml-base.bin")
        );
    }

    #[test]
    fn downloads_verifies_and_reports_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("models").join("ggml-tiny.bin");
        let body = fake_model(200_000);
        let (server, handle) = serve_once("200 OK", body.clone());
        let mut calls = Vec::new();
        download(&format!("{server}/ggml-tiny.bin"), &dest, |d, t| {
            calls.push((d, t));
        })
        .unwrap();
        let request = handle.join().unwrap();
        assert!(request.starts_with("GET /ggml-tiny.bin "));
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!dest.with_extension("bin.part").exists());
        assert!(is_installed(dest.parent().unwrap(), "tiny"));
        assert_eq!(calls.last(), Some(&(200_000, 200_000)));
        assert!(calls.windows(2).all(|w| w[0].0 < w[1].0), "monotonic");
    }

    #[test]
    fn an_html_error_page_is_not_accepted_as_a_model() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("ggml-tiny.bin");
        let (server, _h) = serve_once("200 OK", "<html>Access denied</html>");
        let err = download(&server, &dest, |_, _| {}).unwrap_err();
        assert!(matches!(err, ModelError::NotAModel), "{err}");
        assert!(!dest.exists());
        assert!(!dest.with_extension("bin.part").exists());
    }

    #[test]
    fn http_errors_leave_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("ggml-tiny.bin");
        let (server, _h) = serve_once("404 Not Found", "nope");
        assert!(matches!(
            download(&server, &dest, |_, _| {}),
            Err(ModelError::Status(404))
        ));
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    #[test]
    fn an_existing_model_is_not_replaced_by_a_failed_download() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("ggml-tiny.bin");
        std::fs::write(&dest, fake_model(100)).unwrap();
        let (server, _h) = serve_once("500 Internal Server Error", "boom");
        assert!(download(&server, &dest, |_, _| {}).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), fake_model(100));
    }
}
