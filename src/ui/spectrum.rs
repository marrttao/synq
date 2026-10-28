//! The sound spectrum for the visualizer: an FFT of the latest samples,
//! folded into bands on a logarithmic frequency scale, with the quick rise
//! and slow fall that make bars pleasant to look at.

use crate::audio::scope::Scope;

pub const BANDS: usize = 64;
const FFT_SIZE: usize = 2048;
const LOW_HZ: f32 = 35.0;
const HIGH_HZ: f32 = 16_000.0;
/// How much of a bar is left after one frame without a louder sound.
const FALL: f32 = 0.84;
/// Quietest level shown, in dB (full scale is 0).
const FLOOR_DB: f32 = -70.0;

pub struct Spectrum {
    pub bands: [f32; BANDS],
    last_push: u64,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self {
            bands: [0.0; BANDS],
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
                let now = analyze(&samples, rate);
                for (old, new) in self.bands.iter_mut().zip(now) {
                    *old = new.max(*old * FALL);
                }
            }
            None => self.bands.iter_mut().for_each(|b| *b *= FALL),
        }
    }
}

/// `BANDS` levels (0 to 1) for `samples` (`FFT_SIZE` mono samples).
pub fn analyze(samples: &[f32], rate: u32) -> [f32; BANDS] {
    let mut re = vec![0.0f32; FFT_SIZE];
    let mut im = vec![0.0f32; FFT_SIZE];
    let n = samples.len().min(FFT_SIZE);
    for (i, s) in samples.iter().rev().take(n).rev().enumerate() {
        // A Hann window keeps the loud low notes from smearing everywhere.
        let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (n.max(2) - 1) as f32).cos();
        re[i] = s * w;
    }
    fft(&mut re, &mut im);

    let bin_hz = rate as f32 / FFT_SIZE as f32;
    let high = HIGH_HZ.min(rate as f32 * 0.45);
    let mut out = [0.0f32; BANDS];
    for (b, slot) in out.iter_mut().enumerate() {
        let edge = |i: usize| LOW_HZ * (high / LOW_HZ).powf(i as f32 / BANDS as f32);
        let (f0, f1) = (edge(b), edge(b + 1));
        let lo = ((f0 / bin_hz).floor() as usize).max(1);
        let hi = ((f1 / bin_hz).ceil() as usize).clamp(lo + 1, FFT_SIZE / 2);
        let peak = (lo..hi)
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
            .fold(0.0f32, f32::max);
        // A sine of amplitude A gives a peak of about A * N / 4 after the window.
        let amplitude = peak / (FFT_SIZE as f32 / 4.0);
        let center = (f0 * f1).sqrt();
        // Music falls off towards the treble; lift it so that all of it moves.
        let tilt = 3.0 * (center / 1000.0).log2();
        let db = 20.0 * (amplitude + 1e-7).log10() + tilt;
        *slot = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
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

    fn loudest(bands: &[f32; BANDS]) -> usize {
        (0..BANDS)
            .max_by(|&a, &b| bands[a].total_cmp(&bands[b]))
            .unwrap_or(0)
    }

    /// The band that holds `hz`.
    fn band_of(hz: f32, rate: u32) -> usize {
        let high = HIGH_HZ.min(rate as f32 * 0.45);
        ((hz / LOW_HZ).ln() / (high / LOW_HZ).ln() * BANDS as f32) as usize
    }

    #[test]
    fn a_tone_lights_the_band_of_its_frequency() {
        for hz in [100.0, 440.0, 1000.0, 5000.0, 12_000.0] {
            let bands = analyze(&sine(hz, 44_100, 0.5), 44_100);
            let got = loudest(&bands);
            let want = band_of(hz, 44_100);
            // Low notes are wider than a band (the FFT is coarse down there)
            // and the treble lift favors the upper neighbours a little.
            let slack = if hz < 200.0 { 4 } else { 1 };
            assert!(
                got.abs_diff(want) <= slack,
                "{hz} Hz: band {got}, expected {want}"
            );
            assert!(bands[got] > 0.6, "{hz} Hz: level {}", bands[got]);
            assert!(
                bands[want] > bands[got] - 0.15,
                "{hz} Hz: its own band is lit"
            );
        }
    }

    #[test]
    fn silence_is_flat_and_louder_means_higher() {
        assert!(analyze(&[0.0; FFT_SIZE], 44_100).iter().all(|&b| b == 0.0));
        let quiet = analyze(&sine(440.0, 44_100, 0.02), 44_100);
        let loud = analyze(&sine(440.0, 44_100, 0.8), 44_100);
        let band = band_of(440.0, 44_100);
        assert!(loud[band] > quiet[band] + 0.2);
        assert!(loud.iter().all(|&b| (0.0..=1.0).contains(&b)));
        // Short input and odd rates do not break it.
        analyze(&[0.1; 10], 8000);
        analyze(&[], 48_000);
    }

    #[test]
    fn bars_rise_at_once_and_fall_slowly_and_stop_with_the_music() {
        let scope = Scope::new();
        let mut spectrum = Spectrum::default();
        scope.feed(&sine(440.0, 44_100, 0.5), 44_100);
        spectrum.update(&scope, true);
        let band = band_of(440.0, 44_100);
        let peak = spectrum.bands[band];
        assert!(peak > 0.6);
        // Nothing new arrives (paused): the bars sink, never jump.
        spectrum.update(&scope, true);
        assert!(spectrum.bands[band] < peak && spectrum.bands[band] > peak * 0.5);
        for _ in 0..60 {
            spectrum.update(&scope, false);
        }
        assert!(spectrum.bands.iter().all(|&b| b < 0.01));
    }
}
