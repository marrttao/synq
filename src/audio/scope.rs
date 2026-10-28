//! A tap on the playing audio: the last few thousand samples, shared with
//! the interface for the sound visualizer.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

/// About 0.18 s at 44.1 kHz: plenty for one spectrum.
const KEEP: usize = 8192;
/// Samples are handed over in batches so that the audio thread takes the
/// lock rarely.
const BATCH: usize = 256;

#[derive(Default)]
struct Inner {
    samples: VecDeque<f32>,
    rate: u32,
    /// Grows with every batch, so a reader can tell whether anything new came.
    pushes: u64,
}

#[derive(Default)]
pub struct Scope {
    inner: Mutex<Inner>,
}

impl Scope {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Never waits: if the reader holds the lock the batch is dropped, which
    /// is invisible in a display and keeps the sound clean.
    fn push(&self, batch: &[f32], rate: u32) {
        let Ok(mut g) = self.inner.try_lock() else {
            return;
        };
        g.samples.extend(batch.iter().copied());
        while g.samples.len() > KEEP {
            g.samples.pop_front();
        }
        g.rate = rate;
        g.pushes += 1;
    }

    /// The latest `n` mono samples, the sample rate and the push counter;
    /// `None` until `n` samples have arrived.
    pub fn recent(&self, n: usize) -> Option<(Vec<f32>, u32, u64)> {
        let g = self.inner.lock().ok()?;
        if g.samples.len() < n || g.rate == 0 {
            return None;
        }
        let skip = g.samples.len() - n;
        Some((
            g.samples.iter().skip(skip).copied().collect(),
            g.rate,
            g.pushes,
        ))
    }

    #[cfg(test)]
    pub fn feed(&self, batch: &[f32], rate: u32) {
        self.push(batch, rate);
    }
}

/// Passes a source through unchanged and copies a mono mix of it to a scope.
pub struct Tap<S> {
    inner: S,
    scope: Arc<Scope>,
    batch: Vec<f32>,
    frame: f32,
    in_frame: u16,
}

impl<S: Source> Tap<S> {
    pub fn new(inner: S, scope: Arc<Scope>) -> Self {
        Self {
            inner,
            scope,
            batch: Vec::with_capacity(BATCH),
            frame: 0.0,
            in_frame: 0,
        }
    }
}

impl<S: Source> Iterator for Tap<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let Some(v) = self.inner.next() else {
            // The end of a track: hand over what is left.
            if !self.batch.is_empty() {
                let rate = self.inner.sample_rate().get();
                self.scope.push(&self.batch, rate);
                self.batch.clear();
            }
            return None;
        };
        let channels = self.inner.channels().get();
        self.frame += v;
        self.in_frame += 1;
        if self.in_frame >= channels {
            self.batch.push(self.frame / f32::from(channels));
            self.frame = 0.0;
            self.in_frame = 0;
            if self.batch.len() >= BATCH {
                let rate = self.inner.sample_rate().get();
                self.scope.push(&self.batch, rate);
                self.batch.clear();
            }
        }
        Some(v)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for Tap<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.batch.clear();
        self.frame = 0.0;
        self.in_frame = 0;
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::source::SineWave;

    #[test]
    fn the_scope_keeps_the_latest_samples_and_counts_pushes() {
        let scope = Scope::new();
        assert!(scope.recent(10).is_none());
        scope.push(&[1.0, 2.0, 3.0], 8000);
        assert!(scope.recent(4).is_none(), "not enough yet");
        let (s, rate, pushes) = scope.recent(2).unwrap();
        assert_eq!((s, rate, pushes), (vec![2.0, 3.0], 8000, 1));
        let big: Vec<f32> = (0..KEEP + 100).map(|i| i as f32).collect();
        scope.push(&big, 8000);
        let (s, _, pushes) = scope.recent(KEEP).unwrap();
        assert_eq!(pushes, 2);
        assert_eq!(s.len(), KEEP);
        assert_eq!(s[KEEP - 1], (KEEP + 99) as f32, "the newest sample is last");
    }

    #[test]
    fn a_tap_does_not_change_the_sound_and_feeds_the_scope() {
        let scope = Scope::new();
        let source = SineWave::new(440.0).take_duration(Duration::from_millis(200));
        let plain: Vec<Sample> = SineWave::new(440.0)
            .take_duration(Duration::from_millis(200))
            .collect();
        let tapped: Vec<Sample> = Tap::new(source, scope.clone()).collect();
        assert_eq!(plain, tapped);
        let (samples, rate, _) = scope.recent(2048).expect("samples arrived");
        assert_eq!(rate, 48_000);
        assert!(samples.iter().any(|s| s.abs() > 0.5));
    }
}
