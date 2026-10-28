//! Speech recognition in the UI: when it starts, what it shows, and the
//! question about downloading the model.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::transcribe::{self, Stage};

use super::lyrics_view::LyricsState;
use super::{App, Mode};

/// How long a track must keep playing before recognition starts by itself.
const AUTO_DELAY: Duration = Duration::from_secs(3);

pub(super) enum WhisperStatus {
    Idle,
    Downloading {
        done: u64,
        total: u64,
    },
    Working {
        track_id: i64,
        stage: Stage,
        percent: u8,
        queued: usize,
    },
}

impl App {
    fn transcribe_job(&self, id: i64) -> Option<transcribe::Job> {
        let t = &self.tracks[*self.track_index.get(&id)?];
        Some(transcribe::Job {
            track_id: id,
            rel_path: t.path.clone(),
            audio_path: self.music_dir.join(&t.path),
        })
    }

    /// Lyrics were not found anywhere: recognize them, but only if the
    /// listener stays on this track for a moment.
    pub(super) fn schedule_auto_transcription(&mut self, track_id: i64) {
        if self.whisper_enabled && !self.attempted.contains(&track_id) {
            self.auto_pending = Some((track_id, Instant::now()));
        }
    }

    /// Every frame: start due automatic jobs, keep the worker informed about
    /// the current track and open a waiting model question.
    pub(super) fn tick_whisper(&mut self) {
        self.transcribe.set_current(self.player.now_id);
        if let Some((id, since)) = self.auto_pending {
            if self.player.now_id != Some(id) {
                self.auto_pending = None; // the listener moved on
            } else if since.elapsed() >= AUTO_DELAY {
                self.auto_pending = None;
                self.attempted.insert(id);
                if let Some(job) = self.transcribe_job(id) {
                    self.transcribe.enqueue(job, false);
                }
            }
        }
        if matches!(self.mode, Mode::Normal)
            && let Some((model, size_mb)) = self.model_question.take()
        {
            self.mode = Mode::ConfirmModel { model, size_mb };
        }
    }

    /// `t`: recognize the lyrics of the playing track with Whisper, or, when
    /// Whisper is not available, look them up again.
    pub(super) fn recognize_or_retry(&mut self) {
        if !self.whisper_enabled {
            if transcribe::AVAILABLE {
                self.status = "Whisper is disabled in config.toml; searching online again".into();
            }
            self.retry_lyrics();
            return;
        }
        let Some(id) = self.player.now_id else {
            self.status = "Nothing is playing".into();
            return;
        };
        let Some(job) = self.transcribe_job(id) else {
            return;
        };
        self.auto_pending = None;
        self.attempted.insert(id);
        self.transcribe.enqueue(job, true);
        self.status = "Recognizing lyrics...".into();
    }

    pub(super) fn drain_transcribe_events(&mut self) {
        while let Ok(ev) = self.transcribe.events.try_recv() {
            match ev {
                transcribe::Event::NeedModel { model, size_mb } => {
                    self.model_question = Some((model, size_mb));
                }
                transcribe::Event::Downloading { done, total } => {
                    self.whisper = WhisperStatus::Downloading { done, total };
                }
                transcribe::Event::Progress {
                    track_id,
                    stage,
                    percent,
                    queued,
                } => {
                    self.whisper = WhisperStatus::Working {
                        track_id,
                        stage,
                        percent,
                        queued,
                    };
                }
                transcribe::Event::Done { track_id, found } => {
                    self.whisper = WhisperStatus::Idle;
                    self.status = if found {
                        "Lyrics recognized and saved".into()
                    } else {
                        "No singing found in this track".into()
                    };
                    // Show them right away if the track is still playing.
                    if self.lyrics_for == Some(track_id) {
                        self.request_lyrics(track_id, false);
                    }
                }
                transcribe::Event::Failed { track_id, message } => {
                    self.whisper = WhisperStatus::Idle;
                    self.status = format!("Whisper: {message}");
                    // Let the lyrics panel say why there is still nothing.
                    if track_id.is_some() && track_id == self.lyrics_for {
                        self.lyrics_state = LyricsState::NotFound {
                            problem: Some(message),
                        };
                    }
                }
            }
        }
    }

    pub(super) fn key_confirm_model(&mut self, key: KeyEvent) {
        let yes = matches!(key.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter);
        let no = matches!(key.code, KeyCode::Char('n' | 'N') | KeyCode::Esc);
        if yes || no {
            self.mode = Mode::Normal;
            self.transcribe.confirm_download(yes);
            self.status = if yes {
                "Downloading the Whisper model...".into()
            } else {
                "Model download declined".into()
            };
        }
    }

    /// Short status for the bottom bar.
    pub(super) fn whisper_text(&self) -> Option<String> {
        match &self.whisper {
            WhisperStatus::Idle => None,
            WhisperStatus::Downloading { done, total } => Some(if *total > 0 {
                format!("downloading model {}%", done * 100 / total)
            } else {
                format!("downloading model {} MB", done >> 20)
            }),
            WhisperStatus::Working {
                stage,
                percent,
                queued,
                ..
            } => {
                let queue = if *queued > 0 {
                    format!(" (+{queued} queued)")
                } else {
                    String::new()
                };
                Some(match stage {
                    Stage::Decoding => format!("decoding audio{queue}"),
                    Stage::Recognizing => format!("recognizing {percent}%{queue}"),
                })
            }
        }
    }

    /// What the lyrics panel says while recognition is on its way.
    pub(super) fn whisper_panel_text(&self) -> Option<String> {
        match &self.whisper {
            WhisperStatus::Working { track_id, .. } if self.lyrics_for == Some(*track_id) => {
                self.whisper_text().map(|t| format!("No lyrics yet: {t}"))
            }
            WhisperStatus::Downloading { .. } => {
                self.whisper_text().map(|t| format!("No lyrics yet: {t}"))
            }
            _ => self
                .auto_pending
                .filter(|(id, _)| self.lyrics_for == Some(*id))
                .map(|_| "No lyrics found, recognizing them in a moment...".to_owned()),
        }
    }
}

/// "142 MB" / "1.5 GB" for the download question.
pub(super) fn format_size(mb: u64) -> String {
    if mb >= 1000 {
        format!("{:.1} GB", mb as f64 / 1000.0)
    } else {
        format!("{mb} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_readable() {
        assert_eq!(format_size(75), "75 MB");
        assert_eq!(format_size(1500), "1.5 GB");
    }
}
