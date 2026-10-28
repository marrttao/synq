//! UI-side playback state: mirrors what the audio thread reports and turns
//! key presses into audio commands.

use std::time::Duration;

use crate::audio::queue::{QueueItem, Repeat};
use crate::audio::{Audio, Command, Event};
use crate::config::AudioConfig;
use crate::db::Track;

const VOLUME_STEP: f32 = 0.05;

pub struct Player {
    audio: Audio,
    /// Library id of the track being played.
    pub now_id: Option<i64>,
    pub position: Duration,
    pub paused: bool,
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: Repeat,
    seek_step: Duration,
    music_dir: std::path::PathBuf,
}

impl Player {
    pub fn new(cfg: &AudioConfig, music_dir: std::path::PathBuf) -> Self {
        let volume = cfg.volume.clamp(0.0, 1.0);
        Self {
            audio: Audio::spawn(volume, false, Repeat::Off),
            now_id: None,
            position: Duration::ZERO,
            paused: false,
            volume,
            shuffle: false,
            repeat: Repeat::Off,
            seek_step: Duration::from_secs(u64::from(cfg.seek_step_secs)),
            music_dir,
        }
    }

    /// Apply pending audio events; returns the last error message, if any.
    pub fn poll(&mut self) -> Option<String> {
        let mut error = None;
        while let Ok(ev) = self.audio.events.try_recv() {
            match ev {
                Event::Started { id } => {
                    self.now_id = Some(id);
                    self.position = Duration::ZERO;
                }
                Event::Position(p) => self.position = p,
                Event::Paused(p) => self.paused = p,
                Event::Stopped => {
                    self.now_id = None;
                    self.position = Duration::ZERO;
                    self.paused = false;
                }
                Event::Error(msg) => error = Some(msg),
            }
        }
        error
    }

    /// Play `tracks[selected]` with the whole list as the queue.
    pub fn play_from(&mut self, tracks: &[Track], selected: usize) -> Result<(), String> {
        let Some(chosen) = tracks.get(selected) else {
            return Ok(());
        };
        if chosen.missing {
            return Err(format!("{}: file is missing", chosen.title));
        }
        let playable: Vec<&Track> = tracks.iter().filter(|t| !t.missing).collect();
        let start = playable.iter().position(|t| t.id == chosen.id).unwrap_or(0);
        let items = playable
            .iter()
            .map(|t| QueueItem {
                id: t.id,
                path: self.music_dir.join(&t.path),
            })
            .collect();
        self.audio.send(Command::Play { items, start });
        Ok(())
    }

    /// Pause or resume. Returns false when nothing is loaded.
    pub fn toggle_pause(&mut self) -> bool {
        if self.now_id.is_none() {
            return false;
        }
        self.audio.send(if self.paused {
            Command::Resume
        } else {
            Command::Pause
        });
        true
    }

    pub fn next(&self) {
        self.audio.send(Command::Next);
    }

    pub fn prev(&self) {
        self.audio.send(Command::Prev);
    }

    /// Seek relative to the current position; `forward` picks the direction.
    pub fn seek(&mut self, forward: bool, duration: Duration) {
        if self.now_id.is_none() {
            return;
        }
        let target = seek_target(self.position, self.seek_step, forward, duration);
        // Show the jump at once instead of waiting for the next report.
        self.position = target;
        self.audio.send(Command::Seek(target));
    }

    pub fn change_volume(&mut self, up: bool) {
        self.volume = step_volume(self.volume, up);
        self.audio.send(Command::Volume(self.volume));
    }

    pub fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        self.audio.send(Command::SetShuffle(self.shuffle));
    }

    pub fn cycle_repeat(&mut self) {
        self.repeat = self.repeat.cycle();
        self.audio.send(Command::SetRepeat(self.repeat));
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.audio.send(Command::Stop);
    }
}

/// Where a seek lands: clamped to the track. A zero `duration` (unknown
/// length) only clamps at the start.
pub fn seek_target(pos: Duration, step: Duration, forward: bool, duration: Duration) -> Duration {
    let target = if forward {
        pos + step
    } else {
        pos.saturating_sub(step)
    };
    if duration.is_zero() {
        target
    } else {
        target.min(duration)
    }
}

/// Move volume by one step, staying within 0..=1 and on 5% increments.
pub fn step_volume(volume: f32, up: bool) -> f32 {
    let steps = (volume / VOLUME_STEP).round() as i32 + if up { 1 } else { -1 };
    (steps as f32 * VOLUME_STEP).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn seek_clamps_to_track() {
        assert_eq!(seek_target(S(10), S(5), true, S(100)), S(15));
        assert_eq!(seek_target(S(98), S(5), true, S(100)), S(100));
        assert_eq!(seek_target(S(3), S(5), false, S(100)), S(0));
        assert_eq!(seek_target(S(10), S(5), true, S(0)), S(15));
    }

    #[test]
    fn volume_stays_in_range_on_steps() {
        assert!((step_volume(0.8, true) - 0.85).abs() < 1e-6);
        assert!((step_volume(0.8, false) - 0.75).abs() < 1e-6);
        assert_eq!(step_volume(1.0, true), 1.0);
        assert_eq!(step_volume(0.0, false), 0.0);
        // Off-grid start values snap to the grid (0.83 -> 0.85) before stepping.
        assert!((step_volume(0.83, true) - 0.90).abs() < 1e-6);
    }
}
