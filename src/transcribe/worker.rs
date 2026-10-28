//! Background queue: downloads the model (after the user agreed),
//! recognizes tracks one at a time and stores the result in the lyrics cache.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use crate::config::WhisperConfig;
use crate::lyrics::Cache;
use crate::lyrics::lrc;

use super::engine::Engine;
use super::{Command, Event, Job, Stage, audio, filter, model};

/// Has the user agreed to the model download?
#[derive(Clone, Copy, PartialEq, Eq)]
enum Consent {
    Unasked,
    Approved,
    Declined,
}

struct Queued {
    job: Job,
    /// Started by the user (key `t`) rather than automatically.
    forced: bool,
}

struct Worker {
    cfg: WhisperConfig,
    models_dir: PathBuf,
    cache: Cache,
    events: Sender<Event>,
    /// Track the user is listening to now (0 = none).
    current: Arc<AtomicI64>,
    queue: VecDeque<Queued>,
    engine: Option<(String, Engine)>,
    consent: Consent,
    /// A download question is open; nothing runs until it is answered.
    waiting: bool,
}

pub fn run(
    cfg: WhisperConfig,
    models_dir: PathBuf,
    lyrics_dir: PathBuf,
    commands: Receiver<Command>,
    events: Sender<Event>,
    current: Arc<AtomicI64>,
) {
    let mut w = Worker {
        cfg,
        models_dir,
        cache: Cache::new(lyrics_dir),
        events,
        current,
        queue: VecDeque::new(),
        engine: None,
        consent: Consent::Unasked,
        waiting: false,
    };
    w.main_loop(&commands);
}

impl Worker {
    fn emit(&self, ev: Event) {
        // The UI hanging up is not an error worth reporting.
        let _ = self.events.send(ev);
    }

    fn fail(&self, track_id: Option<i64>, message: String) {
        tracing::warn!("transcription: {message}");
        self.emit(Event::Failed { track_id, message });
    }

    fn main_loop(&mut self, commands: &Receiver<Command>) {
        loop {
            if self.waiting || self.queue.is_empty() {
                match commands.recv() {
                    Ok(cmd) => self.command(cmd),
                    Err(_) => return, // UI is gone
                }
            }
            while let Ok(cmd) = commands.try_recv() {
                self.command(cmd);
            }
            if self.waiting {
                continue;
            }
            if let Some(next) = self.queue.pop_front() {
                self.process(next);
            }
        }
    }

    fn command(&mut self, cmd: Command) {
        match cmd {
            Command::Enqueue { job, forced } => {
                let id = job.track_id;
                if forced {
                    // An explicit request overrides an earlier "no".
                    if self.consent == Consent::Declined {
                        self.consent = Consent::Unasked;
                    }
                    self.queue.retain(|q| q.job.track_id != id);
                    self.queue.push_front(Queued { job, forced });
                } else if self.queue.iter().all(|q| q.job.track_id != id) {
                    // The track being listened to goes first.
                    if id == self.current.load(Ordering::Relaxed) {
                        self.queue.push_front(Queued { job, forced });
                    } else {
                        self.queue.push_back(Queued { job, forced });
                    }
                }
            }
            Command::ConfirmDownload(yes) => {
                if !self.waiting {
                    return;
                }
                self.waiting = false;
                if yes {
                    self.consent = Consent::Approved;
                } else {
                    self.consent = Consent::Declined;
                    self.queue.clear();
                    self.fail(
                        None,
                        "Model download declined; press t to recognize lyrics later".into(),
                    );
                }
            }
        }
    }

    fn process(&mut self, queued: Queued) {
        let Queued { job, forced } = queued;
        // Automatic jobs only make sense for the track still being played.
        if !forced
            && (self.consent == Consent::Declined
                || job.track_id != self.current.load(Ordering::Relaxed))
        {
            return;
        }
        let name = self.cfg.model.trim().to_lowercase();
        let Some(size_mb) = model::size_mb(&name) else {
            self.fail(
                Some(job.track_id),
                model::ModelError::Unknown(name).to_string(),
            );
            return;
        };
        let model_path = model::path(&self.models_dir, &name);
        if !model::is_installed(&self.models_dir, &name) {
            if self.consent != Consent::Approved {
                // Ask first; the job waits at the front of the queue.
                self.queue.push_front(Queued { job, forced });
                self.waiting = true;
                self.emit(Event::NeedModel {
                    model: name,
                    size_mb,
                });
                return;
            }
            if let Err(e) = self.download(&name, &model_path) {
                self.fail(Some(job.track_id), e.to_string());
                // Let the user try again later.
                self.consent = Consent::Unasked;
                return;
            }
        }
        if self.engine.as_ref().is_none_or(|(n, _)| *n != name) {
            match Engine::load(&model_path) {
                Ok(e) => self.engine = Some((name, e)),
                Err(e) => {
                    self.fail(Some(job.track_id), e.to_string());
                    return;
                }
            }
        }
        self.recognize(&job);
    }

    fn download(&self, name: &str, dest: &std::path::Path) -> Result<(), model::ModelError> {
        let url = model::url(model::BASE_URL, name);
        let mut last = u64::MAX;
        model::download(&url, dest, |done, total| {
            // One event per percent (or per MB when the size is unknown).
            let step = (done * 100).checked_div(total).unwrap_or(done >> 20);
            if step != last {
                last = step;
                self.emit(Event::Downloading { done, total });
            }
        })
    }

    fn recognize(&mut self, job: &Job) {
        let track_id = job.track_id;
        let queued = self.queue.len();
        self.emit(Event::Progress {
            track_id,
            stage: Stage::Decoding,
            percent: 0,
            queued,
        });
        let samples = match audio::load_16k_mono(&job.audio_path) {
            Ok(s) => s,
            Err(e) => {
                self.fail(Some(track_id), format!("{}: {e}", job.rel_path));
                return;
            }
        };
        let audio_ms = samples.len() as u64 * 1000 / u64::from(audio::TARGET_RATE);

        let Some((_, engine)) = &self.engine else {
            return;
        };
        // Leave half of the cores to the music and the UI.
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get() / 2);
        let events = self.events.clone();
        let mut last = -1;
        let raw = engine.transcribe(&samples, &self.cfg.language, threads, move |p| {
            if p != last {
                last = p;
                let _ = events.send(Event::Progress {
                    track_id,
                    stage: Stage::Recognizing,
                    percent: p.clamp(0, 100) as u8,
                    queued,
                });
            }
        });
        let raw = match raw {
            Ok(r) => r,
            Err(e) => {
                self.fail(Some(track_id), e.to_string());
                return;
            }
        };

        let (kept, stats) = filter::filter_segments(raw, audio_ms);
        tracing::info!(?stats, kept = kept.len(), "recognition filtered");
        let lines = filter::to_lines(&kept);
        let saved = if lines.is_empty() {
            self.cache.write_no_speech(&job.rel_path)
        } else {
            self.cache
                .write_synced(&job.rel_path, &lrc::format_lrc(&lines))
        };
        match saved {
            Ok(()) => self.emit(Event::Done {
                track_id,
                found: !lines.is_empty(),
            }),
            Err(e) => self.fail(Some(track_id), format!("cannot save the lyrics: {e}")),
        }
    }
}
