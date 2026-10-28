//! Reading GIF / PNG / JPG files into frames. Done once per picture; the
//! frames are already small enough for the panel, so drawing stays cheap.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::imageops::FilterType;
use image::{AnimationDecoder, DynamicImage, ImageReader};
use thiserror::Error;

/// Longest side of a stored frame. Terminal panels are far smaller than
/// this, and 300 frames of it stay near 150 MB at worst.
pub const MAX_DIM: u32 = 360;
/// Frames beyond this are dropped (the animation just gets shorter).
pub const MAX_FRAMES: usize = 300;
const MAX_FILE_BYTES: u64 = 128 << 20;
/// Browsers show GIF delays below 20 ms as 100 ms; so do we.
const MIN_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_DELAY: Duration = Duration::from_millis(100);

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("cannot read {0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid image: {0}")]
    Image(#[from] image::ImageError),
    #[error("the file is too large ({0} MB)")]
    TooLarge(u64),
    #[error("the picture has no frames")]
    Empty,
}

pub struct Frame {
    pub image: DynamicImage,
    pub delay: Duration,
}

pub struct Animation {
    pub frames: Vec<Frame>,
}

pub fn decode(path: &Path) -> Result<Animation, DecodeError> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_FILE_BYTES {
        return Err(DecodeError::TooLarge(size >> 20));
    }
    let is_gif = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    let frames = if is_gif {
        decode_gif(path)?
    } else {
        let image = ImageReader::open(path)?.with_guessed_format()?.decode()?;
        vec![Frame {
            image: shrink(image),
            delay: DEFAULT_DELAY,
        }]
    };
    if frames.is_empty() {
        return Err(DecodeError::Empty);
    }
    Ok(Animation { frames })
}

fn decode_gif(path: &Path) -> Result<Vec<Frame>, DecodeError> {
    let decoder = GifDecoder::new(BufReader::new(File::open(path)?))?;
    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        if frames.len() >= MAX_FRAMES {
            tracing::info!(
                "{}: only the first {MAX_FRAMES} frames are used",
                path.display()
            );
            break;
        }
        let frame = match frame {
            Ok(f) => f,
            // A damaged tail still leaves a usable beginning.
            Err(e) if !frames.is_empty() => {
                tracing::warn!("{}: stopped at a bad frame: {e}", path.display());
                break;
            }
            Err(e) => return Err(e.into()),
        };
        let (num, den) = frame.delay().numer_denom_ms();
        let delay = normalize_delay(Duration::from_millis(u64::from(num / den.max(1))));
        let image = DynamicImage::ImageRgba8(frame.into_buffer());
        frames.push(Frame {
            image: shrink(image),
            delay,
        });
    }
    Ok(frames)
}

pub fn normalize_delay(delay: Duration) -> Duration {
    if delay < MIN_DELAY {
        DEFAULT_DELAY
    } else {
        delay
    }
}

/// Scale down (never up) so that the longest side is at most `MAX_DIM`.
fn shrink(image: DynamicImage) -> DynamicImage {
    if image.width() <= MAX_DIM && image.height() <= MAX_DIM {
        image
    } else {
        image.resize(MAX_DIM, MAX_DIM, FilterType::Triangle)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame as GifFrame, RgbaImage};

    /// A GIF with one solid-color frame per `(color, delay_ms)`.
    pub fn gif_bytes(width: u32, height: u32, frames: &[([u8; 3], u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            for (c, ms) in frames {
                let img =
                    RgbaImage::from_pixel(width, height, image::Rgba([c[0], c[1], c[2], 255]));
                let delay = Delay::from_numer_denom_ms(*ms, 1);
                enc.encode_frame(GifFrame::from_parts(img, 0, 0, delay))
                    .unwrap();
            }
        }
        out
    }

    #[test]
    fn gif_frames_keep_their_own_delays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.gif");
        std::fs::write(
            &path,
            gif_bytes(
                8,
                4,
                &[([255, 0, 0], 100), ([0, 255, 0], 300), ([0, 0, 255], 40)],
            ),
        )
        .unwrap();
        let anim = decode(&path).unwrap();
        let delays: Vec<u64> = anim
            .frames
            .iter()
            .map(|f| f.delay.as_millis() as u64)
            .collect();
        assert_eq!(delays, [100, 300, 40]);
        assert_eq!(anim.frames.len(), 3);
        assert_eq!(anim.frames[0].image.width(), 8);
    }

    #[test]
    fn too_short_delays_become_a_tenth_of_a_second() {
        assert_eq!(normalize_delay(Duration::ZERO), DEFAULT_DELAY);
        assert_eq!(normalize_delay(Duration::from_millis(10)), DEFAULT_DELAY);
        assert_eq!(normalize_delay(Duration::from_millis(20)), MIN_DELAY);
    }

    #[test]
    fn big_pictures_are_shrunk_with_their_proportions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.gif");
        std::fs::write(&path, gif_bytes(1000, 500, &[([9, 9, 9], 50)])).unwrap();
        let anim = decode(&path).unwrap();
        let img = &anim.frames[0].image;
        assert_eq!((img.width(), img.height()), (MAX_DIM, MAX_DIM / 2));
        assert_eq!(anim.frames.len(), 1);
    }

    #[test]
    fn png_is_a_single_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.png");
        RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let anim = decode(&path).unwrap();
        assert_eq!(anim.frames.len(), 1);
    }

    #[test]
    fn broken_and_missing_files_are_errors_not_panics() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.gif");
        std::fs::write(&bad, b"GIF89a-not-really").unwrap();
        assert!(decode(&bad).is_err());
        let text = dir.path().join("t.png");
        std::fs::write(&text, b"hello").unwrap();
        assert!(decode(&text).is_err());
        assert!(decode(&dir.path().join("nope.gif")).is_err());
    }

    #[test]
    fn a_gif_with_a_damaged_tail_keeps_its_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.gif");
        let mut bytes = gif_bytes(
            16,
            16,
            &[([255, 0, 0], 100), ([0, 255, 0], 100), ([0, 0, 255], 100)],
        );
        bytes.truncate(bytes.len() - 25);
        std::fs::write(&path, bytes).unwrap();
        // Either the cut falls into the last frame (two survive) or the
        // decoder is lenient; a panic or an empty result is what must not happen.
        let anim = decode(&path).unwrap();
        assert!(!anim.frames.is_empty());
    }
}
