//! Local speech recognition (Whisper) for tracks without lyrics.
//!
//! Everything heavy lives behind the `whisper` cargo feature; without it
//! [`AVAILABLE`] is false, [`spawn`] returns an inert handle and the rest of
//! the player works as usual.

#[cfg(feature = "whisper")]
mod audio;
#[cfg(feature = "whisper")]
mod engine;
#[cfg(feature = "whisper")]
mod filter;
#[cfg(feature = "whisper")]
mod model;
#[cfg(feature = "whisper")]
mod worker;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use crate::config::WhisperConfig;
use crate::paths::Paths;

/// Was this build compiled with Whisper support?
pub const AVAILABLE: bool = cfg!(feature = "whisper");

/// A track to recognize.
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct Job {
    pub track_id: i64,
    /// Path relative to music/ (the lyrics cache key).
    pub rel_path: String,
    pub audio_path: PathBuf,
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
#[derive(Debug)]
pub enum Command {
    /// `forced`: the user asked for it (overrides earlier "no" answers).
    Enqueue { job: Job, forced: bool },
    /// Answer to [`Event::NeedModel`].
    ConfirmDownload(bool),
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Decoding,
    Recognizing,
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
#[derive(Debug)]
pub enum Event {
    /// The model is missing: ask the user before downloading it.
    NeedModel { model: String, size_mb: u64 },
    /// Download progress in bytes (`total` is 0 if unknown).
    Downloading { done: u64, total: u64 },
    Progress {
        track_id: i64,
        stage: Stage,
        /// 0..=100
        percent: u8,
        /// Tracks still waiting after this one.
        queued: usize,
    },
    /// The result is in the lyrics cache (`found`: there was something).
    Done { track_id: i64, found: bool },
    Failed {
        track_id: Option<i64>,
        message: String,
    },
}

pub struct Handle {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
    current: Arc<AtomicI64>,
}

impl Handle {
    pub fn enqueue(&self, job: Job, forced: bool) {
        self.send(Command::Enqueue { job, forced });
    }

    pub fn confirm_download(&self, yes: bool) {
        self.send(Command::ConfirmDownload(yes));
    }

    /// Tell the worker which track is being listened to; queued automatic
    /// jobs for other tracks are skipped.
    pub fn set_current(&self, track_id: Option<i64>) {
        self.current.store(track_id.unwrap_or(0), Ordering::Relaxed);
    }

    fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_err() {
            tracing::error!("transcription thread is gone");
        }
    }
}

#[cfg(feature = "whisper")]
pub fn spawn(cfg: &WhisperConfig, paths: &Paths) -> Handle {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (ev_tx, ev_rx) = mpsc::channel();
    let current = Arc::new(AtomicI64::new(0));
    let spawned = std::thread::Builder::new()
        .name("transcribe".into())
        .spawn({
            let (cfg, current) = (cfg.clone(), Arc::clone(&current));
            let (models, lyrics) = (paths.models.clone(), paths.lyrics.clone());
            let ev_tx = ev_tx.clone();
            move || worker::run(cfg, models, lyrics, cmd_rx, ev_tx, current)
        });
    if let Err(e) = spawned {
        tracing::error!("cannot start transcription thread: {e}");
        let _ = ev_tx.send(Event::Failed {
            track_id: None,
            message: format!("transcription thread: {e}"),
        });
    }
    Handle {
        tx: cmd_tx,
        events: ev_rx,
        current,
    }
}

/// Without the `whisper` feature nothing listens: requests are dropped.
#[cfg(not(feature = "whisper"))]
pub fn spawn(_cfg: &WhisperConfig, _paths: &Paths) -> Handle {
    let (cmd_tx, _cmd_rx) = mpsc::channel();
    let (_ev_tx, ev_rx) = mpsc::channel();
    Handle {
        tx: cmd_tx,
        events: ev_rx,
        current: Arc::new(AtomicI64::new(0)),
    }
}

/// Manual end-to-end check with a real model; needs network and a speech
/// recording:
///
/// `SYNQ_TEST_AUDIO=speech.wav cargo test real_model -- --ignored --nocapture`
///
/// Set `SYNQ_TEST_MODELS` to a folder to keep the downloaded model between runs.
#[cfg(all(test, feature = "whisper"))]
mod real_model_test {
    use super::*;

    #[test]
    #[ignore = "downloads a model and needs SYNQ_TEST_AUDIO"]
    fn real_model_recognizes_speech() {
        // whisper.cpp logs go through `log` into tracing.
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(tracing::Level::DEBUG)
            .try_init();
        let audio_path = PathBuf::from(
            std::env::var("SYNQ_TEST_AUDIO").expect("set SYNQ_TEST_AUDIO to a speech recording"),
        );
        let tmp = tempfile::tempdir().unwrap();
        let models = std::env::var("SYNQ_TEST_MODELS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| tmp.path().join("models"));
        let model_path = model::path(&models, "tiny");
        if !model_path.is_file() {
            let url = model::url(model::BASE_URL, "tiny");
            let mut last = 0;
            model::download(&url, &model_path, |done, total| {
                if total > 0 && done * 10 / total != last {
                    last = done * 10 / total;
                    eprintln!("downloaded {}%", done * 100 / total);
                }
            })
            .unwrap();
        }

        let samples = audio::load_16k_mono(&audio_path).unwrap();
        eprintln!("audio: {:.1} s at 16 kHz", samples.len() as f32 / 16_000.0);
        let engine = engine::Engine::load(&model_path).unwrap();
        let started = std::time::Instant::now();
        let raw = engine
            .transcribe(&samples, "auto", 4, |p| eprintln!("progress {p}%"))
            .unwrap();
        eprintln!(
            "recognized in {:.1} s: {raw:#?}",
            started.elapsed().as_secs_f32()
        );

        let audio_ms = samples.len() as u64 / 16;
        let (kept, stats) = filter::filter_segments(raw, audio_ms);
        let lines = filter::to_lines(&kept);
        let lrc = crate::lyrics::lrc::format_lrc(&lines);
        eprintln!("{stats:?}\n{lrc}");
        assert!(!lines.is_empty(), "nothing recognized");
        assert!(
            crate::lyrics::lrc::parse(&lrc).is_some(),
            "output must parse as LRC"
        );
        assert!(lines.iter().any(|l| !l.words.is_empty()), "word timings");
    }
}
