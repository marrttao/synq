//! Playback thread built on rodio.
//!
//! The UI sends [`Command`]s and receives [`Event`]s over channels; the
//! thread owns the output device, the current track and the play queue.

pub mod queue;

use std::fs::File;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

use queue::{Queue, QueueItem, Repeat, Rng};

/// How often position updates are sent and track ends are noticed.
const TICK: Duration = Duration::from_millis(100);
/// "Previous" restarts the current track when it has played this long.
const RESTART_AFTER: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub enum Command {
    /// Replace the queue and start playing `items[start]`.
    Play {
        items: Vec<QueueItem>,
        start: usize,
    },
    Pause,
    Resume,
    Seek(Duration),
    Next,
    Prev,
    Volume(f32),
    SetShuffle(bool),
    SetRepeat(Repeat),
    Stop,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A track (library id) started playing.
    Started {
        id: i64,
    },
    /// Playback position of the current track.
    Position(Duration),
    Paused(bool),
    /// Nothing is playing any more (queue ended or stopped).
    Stopped,
    Error(String),
}

pub struct Audio {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
}

impl Audio {
    pub fn spawn(volume: f32, shuffle: bool, repeat: Repeat) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let spawned = thread::Builder::new().name("audio".into()).spawn({
            let ev_tx = ev_tx.clone();
            move || {
                Engine {
                    events: ev_tx,
                    device: None,
                    player: None,
                    queue: None,
                    volume: volume.clamp(0.0, 1.0),
                    shuffle,
                    repeat,
                }
                .run(&cmd_rx);
            }
        });
        if let Err(e) = spawned {
            tracing::error!("cannot start audio thread: {e}");
            // A send error only means the UI is already gone.
            let _ = ev_tx.send(Event::Error(format!("audio thread: {e}")));
        }
        Self {
            tx: cmd_tx,
            events: ev_rx,
        }
    }

    pub fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_err() {
            tracing::error!("audio thread is gone");
        }
    }
}

/// Why a track could not be started.
enum OpenError {
    /// This file is unreadable or undecodable; others may still work.
    Track(String),
    /// The output device is unavailable; nothing can play.
    Device(String),
}

struct Engine {
    events: Sender<Event>,
    device: Option<MixerDeviceSink>,
    player: Option<Player>,
    queue: Option<Queue>,
    volume: f32,
    shuffle: bool,
    repeat: Repeat,
}

impl Engine {
    fn emit(&self, ev: Event) {
        // The UI hanging up is not an error worth reporting.
        let _ = self.events.send(ev);
    }

    fn fail(&self, msg: String) {
        tracing::error!("{msg}");
        self.emit(Event::Error(msg));
    }

    fn run(mut self, commands: &Receiver<Command>) {
        loop {
            match commands.recv_timeout(TICK) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.tick();
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Play { items, start } => {
                if items.is_empty() {
                    return;
                }
                self.queue = Some(Queue::new(
                    items,
                    start,
                    self.shuffle,
                    self.repeat,
                    Rng::from_clock(),
                ));
                self.start_current();
            }
            Command::Pause => {
                if let Some(p) = &self.player {
                    p.pause();
                    self.emit(Event::Paused(true));
                }
            }
            Command::Resume => {
                if let Some(p) = &self.player {
                    p.play();
                    self.emit(Event::Paused(false));
                }
            }
            Command::Seek(pos) => {
                if let Some(p) = &self.player {
                    match p.try_seek(pos) {
                        Ok(()) => self.emit(Event::Position(p.get_pos())),
                        Err(e) => self.fail(format!("seek failed: {e}")),
                    }
                }
            }
            Command::Next => {
                if let Some(q) = &mut self.queue {
                    q.next(false);
                    self.start_current();
                }
            }
            Command::Prev => self.prev(),
            Command::Volume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                if let Some(p) = &self.player {
                    p.set_volume(self.volume);
                }
            }
            Command::SetShuffle(on) => {
                self.shuffle = on;
                if let Some(q) = &mut self.queue {
                    q.set_shuffle(on);
                }
            }
            Command::SetRepeat(r) => {
                self.repeat = r;
                if let Some(q) = &mut self.queue {
                    q.set_repeat(r);
                }
            }
            Command::Stop => self.stop(),
        }
    }

    fn prev(&mut self) {
        let played = self.player.as_ref().map(Player::get_pos);
        if played.is_some_and(|p| p > RESTART_AFTER) {
            if let Some(p) = &self.player {
                match p.try_seek(Duration::ZERO) {
                    Ok(()) => self.emit(Event::Position(Duration::ZERO)),
                    Err(e) => self.fail(format!("seek failed: {e}")),
                }
            }
        } else if let Some(q) = &mut self.queue {
            q.prev();
            self.start_current();
        }
    }

    fn stop(&mut self) {
        self.player = None;
        self.queue = None;
        self.emit(Event::Stopped);
    }

    /// Noticed every tick: report position, advance when a track ends.
    fn tick(&mut self) {
        let Some(player) = &self.player else { return };
        if player.empty() && !player.is_paused() {
            let next = self.queue.as_mut().and_then(|q| q.next(true)).is_some();
            if next {
                self.start_current();
            } else {
                self.stop();
            }
        } else {
            self.emit(Event::Position(player.get_pos()));
        }
    }

    fn ensure_device(&mut self) -> Result<&MixerDeviceSink, String> {
        if self.device.is_none() {
            let mut sink = DeviceSinkBuilder::open_default_sink()
                .map_err(|e| format!("cannot open audio output: {e}"))?;
            // The default message goes to stderr, which would garble the UI.
            sink.log_on_drop(false);
            self.device = Some(sink);
        }
        self.device
            .as_ref()
            .ok_or_else(|| "no audio output".to_owned())
    }

    /// Start the queue's current track; unplayable files are skipped.
    fn start_current(&mut self) {
        let mut tries = 0usize;
        loop {
            let Some(item) = self.queue.as_ref().and_then(|q| q.current()).cloned() else {
                self.stop();
                return;
            };
            match self.open(&item) {
                Ok(()) => return,
                Err(OpenError::Device(msg)) => {
                    // Every other track would fail the same way.
                    self.fail(msg);
                    self.stop();
                    return;
                }
                Err(OpenError::Track(msg)) => {
                    self.fail(msg);
                    tries += 1;
                    // Skip forward; with repeat-one "auto" would stay put.
                    let auto = self.repeat != Repeat::One;
                    let next = self.queue.as_mut().and_then(|q| q.next(auto)).is_some();
                    if !next || tries >= self.queue_len() {
                        self.stop();
                        return;
                    }
                }
            }
        }
    }

    fn queue_len(&self) -> usize {
        self.queue.as_ref().map_or(0, Queue::len)
    }

    fn open(&mut self, item: &QueueItem) -> Result<(), OpenError> {
        let name = item
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let track_err = |e: &dyn std::fmt::Display| OpenError::Track(format!("{name}: {e}"));
        let file = File::open(&item.path).map_err(|e| track_err(&e))?;
        let decoder = Decoder::try_from(file).map_err(|e| track_err(&e))?;
        let volume = self.volume;
        let mixer = self
            .ensure_device()
            .map_err(OpenError::Device)?
            .mixer()
            .clone();
        let player = Player::connect_new(&mixer);
        player.set_volume(volume);
        player.append(decoder);
        // Replacing the old player stops it.
        self.player = Some(player);
        self.emit(Event::Started { id: item.id });
        self.emit(Event::Paused(false));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::wav_bytes;

    fn next_event(audio: &Audio) -> Option<Event> {
        audio.events.recv_timeout(Duration::from_secs(5)).ok()
    }

    /// Needs a sound device; on machines without one the engine must report
    /// an error instead of crashing, which is what is checked then.
    #[test]
    fn plays_a_file_to_the_end_and_reports_events() {
        let tmp = tempfile::tempdir().unwrap();
        let mut items = Vec::new();
        for id in 0..2 {
            let path = tmp.path().join(format!("{id}.wav"));
            std::fs::write(&path, wav_bytes(400)).unwrap();
            items.push(QueueItem { id, path });
        }
        let audio = Audio::spawn(0.0, false, Repeat::Off);
        audio.send(Command::Play { items, start: 0 });

        let mut started = Vec::new();
        loop {
            match next_event(&audio) {
                Some(Event::Started { id }) => started.push(id),
                Some(Event::Stopped) => break,
                Some(Event::Error(msg)) => {
                    assert!(msg.contains("audio output"), "unexpected error: {msg}");
                    eprintln!("skipped: no audio device ({msg})");
                    return;
                }
                Some(_) => {}
                None => panic!("timed out, started so far: {started:?}"),
            }
        }
        assert_eq!(started, [0, 1], "both tracks play in order, then stop");
    }

    #[test]
    fn unplayable_files_are_reported_and_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("bad.wav");
        std::fs::write(&bad, b"definitely not audio").unwrap();
        let missing = tmp.path().join("missing.wav");
        let items = vec![
            QueueItem { id: 1, path: bad },
            QueueItem {
                id: 2,
                path: missing,
            },
        ];
        let audio = Audio::spawn(0.0, false, Repeat::Off);
        audio.send(Command::Play { items, start: 0 });

        let mut errors = 0;
        loop {
            match next_event(&audio) {
                Some(Event::Error(_)) => errors += 1,
                Some(Event::Stopped) => break,
                Some(Event::Started { id }) => panic!("track {id} must not start"),
                Some(_) => {}
                None => panic!("timed out"),
            }
        }
        assert_eq!(errors, 2);
    }
}
