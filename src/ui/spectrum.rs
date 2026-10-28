//! The sound spectrum for the visualizer: an FFT of the latest samples,
//! folded into bands on a logarithmic frequency scale, then made pleasant:
//! the level adapts to the loudness of the music (so quiet and loud tracks
//! both fill the panel), neighbouring bands are smoothed into each other,
//! and bars rise quickly, fall slowly and leave a falling peak mark.

use crate::audio::scope::Scope;

pub const BANDS: usize = 64;
const FFT_SIZE: usize = 4096;
const LOW_HZ: f32 = 30.0;
const HIGH_HZ: f32 = 16_000.0;
/// Lift per octave: music loses about this much towards the treble.
const TILT_DB_PER_OCTAVE: f32 = 4.0;
/// How many dB below the loudest recent level still show.
const RANGE_DB: f32 = 50.0;
/// The reference level never goes below this, so silence stays flat and
/// very quiet music is not blown up into noise.
const MIN_REF_DB: f32 = -28.0;
/// How fast the reference level sinks when the music gets quieter, per frame.
const REF_FALL_DB: f32 = 0.06;
/// How far a bar moves towards a louder level in one frame.
const ATTACK: f32 = 0.65;
/// How much of a bar is left after one frame without a louder sound.
const FALL: f32 = 0.88;
/// Frames a peak mark stays before it falls, and how fast it falls.
const PEAK_HOLD: u8 = 14;
const PEAK_FALL: f32 = 0.025;

pub struct Spectrum {
    /// Bar heights, 0 to 1.
    pub bands: [f32; BANDS],
    /// Peak marks, 0 to 1.
    pub peaks: [f32; BANDS],
    hold: [u8; BANDS],
    reference_db: f32,
    last_push: u64,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self {
            bands: [0.0; BANDS],
            peaks: [0.0; BANDS],
            hold: [0; BANDS],
            reference_db: MIN_REF_DB,
            last_push: 0,
        }
    }
}

impl Spectrum {
    /// Called once per frame; `playing` false lets the bars sink.
    pub fn update(&mut self, scope: &Scope, playing: bool) {
        let fresh = playing
            .then(|| scope.recent(FFT_SIZE))
            .flatten()
            .filter(|(_, _, pushes)| *pushes != self.last_push);
        match fresh {
            Some((samples, rate, pushes)) => {
                self.last_push = pushes;
                self.take(&levels_db(&samples, rate));
            }
            None => self.sink(),
        }
        self.mark_peaks();
    }

    /// Feed one frame of levels (in dB).
    fn take(&mut self, db: &[f32; BANDS]) {
        // Mix with the neighbours first: smooth, even bars instead of spikes.
        let mut smooth = [0.0f32; BANDS];
        for (b, slot) in smooth.iter_mut().enumerate() {
            let left = db[b.saturating_sub(1)];
            let right = db[(b + 1).min(BANDS - 1)];
            *slot = 0.2 * left + 0.6 * db[b] + 0.2 * right;
        }
        // The loudest band of the moment sets the scale (slowly sinking).
        let loudest = smooth.iter().copied().fold(f32::MIN, f32::max);
        self.reference_db = loudest.max(self.reference_db - REF_FALL_DB).max(MIN_REF_DB);
        let floor = self.reference_db - RANGE_DB;
        for (bar, &db) in self.bands.iter_mut().zip(&smooth) {
            let level = ((db - floor) / RANGE_DB).clamp(0.0, 1.0);
            *bar = if level > *bar {
                *bar + (level - *bar) * ATTACK
            } else {
                *bar * FALL
            };
        }
    }

    fn sink(&mut self) {
        self.bands.iter_mut().for_each(|b| *b *= FALL);
    }

    fn mark_peaks(&mut self) {
        for b in 0..BANDS {
            if self.bands[b] >= self.peaks[b] {
                self.peaks[b] = self.bands[b];
                self.hold[b] = PEAK_HOLD;
            } else if self.hold[b] > 0 {
                self.hold[b] -= 1;
            } else {
                self.peaks[b] = (self.peaks[b] - PEAK_FALL).max(self.bands[b]);
            }
        }
    }
}

/// The level of every band in dB (about 0 for a full-scale tone, with the
/// treble lift applied), for `FFT_SIZE` mono samples.
pub fn levels_db(samples: &[f32], rate: u32) -> [f32; BANDS] {
    let mut re = vec![0.0f32; FFT_SIZE];
    let mut im = vec![0.0f32; FFT_SIZE];
    let n = samples.len().min(FFT_SIZE);
    for (i, s) in samples.iter().rev().take(n).rev().enumerate() {
        // A Hann window keeps the loud low notes from smearing everywhere.
        let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (n.max(2) - 1) as f32).cos();
        re[i] = s * w;
    }
    fft(&mut re, &mut im);
    let magnitude = |k: usize| (re[k] * re[k] + im[k] * im[k]).sqrt();

    let bin_hz = rate as f32 / FFT_SIZE as f32;
    let high = HIGH_HZ.min(rate as f32 * 0.45);
    let mut out = [0.0f32; BANDS];
    for (b, slot) in out.iter_mut().enumerate() {
        let edge = |i: usize| LOW_HZ * (high / LOW_HZ).powf(i as f32 / BANDS as f32);
        let (f0, f1) = (edge(b), edge(b + 1));
        let center = (f0 * f1).sqrt();
        let (lo, hi) = (f0 / bin_hz, f1 / bin_hz);
        let peak = if hi - lo >= 2.0 {
            // Many bins: the loudest of them.
            (lo.floor() as usize..hi.ceil() as usize)
                .map(|k| magnitude(k.clamp(1, FFT_SIZE / 2)))
                .fold(0.0f32, f32::max)
        } else {
            // Less than a bin or two (the low end): interpolate between the
            // neighbouring bins, so that the bars there are not steps.
            let pos = center / bin_hz;
            let k = (pos.floor() as usize).clamp(1, FFT_SIZE / 2 - 1);
            let t = (pos - k as f32).clamp(0.0, 1.0);
            magnitude(k) * (1.0 - t) + magnitude(k + 1) * t
        };
        // A sine of amplitude A gives a peak of about A * N / 4 after the window.
        let amplitude = peak / (FFT_SIZE as f32 / 4.0);
        let tilt = TILT_DB_PER_OCTAVE * (center / 1000.0).log2();
        *slot = 20.0 * (amplitude + 1e-7).log10() + tilt;
    }
    out
}

/// In-place radix-2 FFT; the length must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, amplitude: f32) -> Vec<f32> {
        (0..FFT_SIZE)
            .map(|i| amplitude * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn loudest(bands: &[f32]) -> usize {
        (0..bands.len())
            .max_by(|&a, &b| bands[a].total_cmp(&bands[b]))
            .unwrap_or(0)
    }

    /// The band that holds `hz`.
    fn band_of(hz: f32, rate: u32) -> usize {
        let high = HIGH_HZ.min(rate as f32 * 0.45);
        ((hz / LOW_HZ).ln() / (high / LOW_HZ).ln() * BANDS as f32) as usize
    }

    /// A few frames of a signal through the whole pipeline.
    fn settle(samples: &[f32], rate: u32) -> Spectrum {
        let mut s = Spectrum::default();
        for _ in 0..30 {
            s.take(&levels_db(samples, rate));
            s.mark_peaks();
        }
        s
    }

    #[test]
    fn a_tone_lights_the_band_of_its_frequency() {
        for hz in [100.0, 440.0, 1000.0, 5000.0, 12_000.0] {
            let db = levels_db(&sine(hz, 44_100, 0.5), 44_100);
            let got = loudest(&db);
            let want = band_of(hz, 44_100);
            let slack = if hz < 200.0 { 3 } else { 1 };
            assert!(
                got.abs_diff(want) <= slack,
                "{hz} Hz: band {got}, expected {want}"
            );
            assert!(db[want] > db[got] - 3.0, "{hz} Hz: its own band is lit");
        }
    }

    #[test]
    fn silence_is_flat_and_quiet_music_still_fills_the_panel() {
        let silent = settle(&[0.0; FFT_SIZE], 44_100);
        assert!(silent.bands.iter().all(|&b| b == 0.0));
        let band = band_of(440.0, 44_100);
        // Loud and quiet tracks both reach the top: the level adapts.
        let quiet = settle(&sine(440.0, 44_100, 0.03), 44_100);
        let loud = settle(&sine(440.0, 44_100, 0.9), 44_100);
        let top = |s: &Spectrum| s.bands.iter().copied().fold(0.0f32, f32::max);
        assert!(top(&quiet) > 0.55, "quiet tops at {}", top(&quiet));
        assert!(quiet.bands[band] > 0.4, "{}", quiet.bands[band]);
        assert!(loud.bands[band] > 0.9, "{}", loud.bands[band]);
        assert!(loud.bands.iter().all(|&b| (0.0..=1.0).contains(&b)));
        // Short input and odd rates do not break it.
        levels_db(&[0.1; 10], 8000);
        levels_db(&[], 48_000);
    }

    #[test]
    fn neighbouring_bands_move_together() {
        // A single tone lights its band and the two beside it, softly.
        let s = settle(&sine(1000.0, 44_100, 0.5), 44_100);
        let at = loudest(&s.bands);
        assert!(s.bands[at] > 0.9);
        assert!(s.bands[at - 1] > 0.1 && s.bands[at - 1] < s.bands[at]);
        assert!(s.bands[at + 1] > 0.1 && s.bands[at + 1] < s.bands[at]);
        assert!(s.bands[at + 8] < 0.2, "far bands stay low");
    }

    #[test]
    fn bars_rise_at_once_fall_slowly_and_stop_with_the_music() {
        let scope = Scope::new();
        let mut spectrum = Spectrum::default();
        scope.feed(&sine(440.0, 44_100, 0.5), 44_100);
        // The scope needs a full window before anything shows.
        for _ in 0..3 {
            scope.feed(&sine(440.0, 44_100, 0.5), 44_100);
        }
        spectrum.update(&scope, true);
        let band = band_of(440.0, 44_100);
        let peak = spectrum.bands[band];
        assert!(peak > 0.4, "{peak}");
        // Nothing new arrives (paused): the bars sink, never jump.
        spectrum.update(&scope, true);
        assert!(spectrum.bands[band] < peak && spectrum.bands[band] > peak * 0.5);
        // The peak mark stays up while the bar sinks.
        assert!(spectrum.peaks[band] >= peak - 1e-6);
        for _ in 0..80 {
            spectrum.update(&scope, false);
        }
        assert!(spectrum.bands.iter().all(|&b| b < 0.01));
        assert!(spectrum.peaks.iter().all(|&p| p < 0.2), "peaks fall too");
    }
}
