//! Background rendering of picture frames. Scaling a frame to the panel
//! costs milliseconds (tens of them for pixel pictures), which would freeze
//! the interface if done between two screen updates; so worker threads do
//! it, and the interface shows the newest frame that is ready.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use image::DynamicImage;
use ratatui::layout::Size;
use ratatui::text::Line;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, ResizeEncodeRender};

use super::blocks;
use super::visual_view::{VisualFit, ascii_lines, scaled};

/// What a frame is rendered into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Blocks,
    Ascii,
    /// Real pixels: the size of the panel in pixels.
    Pixels {
        width: u32,
        height: u32,
    },
}

/// Everything that decides how a frame looks; frames made for another key
/// are of no use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub cols: u16,
    pub rows: u16,
    pub fit: VisualFit,
    pub crop: u8,
    pub kind: Kind,
}

pub enum Output {
    Blocks(blocks::Grid),
    Ascii(Vec<Line<'static>>),
    /// Scaled and already encoded for the terminal (sixel encoding is the
    /// slowest part, and must not happen while the screen is drawn).
    Pixels(StatefulProtocol),
}

pub struct Job {
    /// Jobs older than the newest one are skipped.
    pub id: u64,
    pub index: usize,
    pub images: Arc<Vec<DynamicImage>>,
    pub key: Key,
    pub background: [u8; 3],
}

pub struct Done {
    pub id: u64,
    pub index: usize,
    pub output: Output,
}

pub struct Renderer {
    jobs: Sender<Job>,
    pub done: Receiver<Done>,
    newest: Arc<AtomicU64>,
    next_id: AtomicU64,
}

/// Two workers: the next frames are made side by side.
const WORKERS: usize = 2;

impl Renderer {
    pub fn spawn(picker: &Picker) -> Self {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done) = mpsc::channel::<Done>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let newest = Arc::new(AtomicU64::new(0));
        for n in 0..WORKERS {
            let (job_rx, done_tx, newest) = (job_rx.clone(), done_tx.clone(), newest.clone());
            let picker = picker.clone();
            let spawned = thread::Builder::new()
                .name(format!("render-{n}"))
                .spawn(move || work(&job_rx, &done_tx, &newest, &picker));
            if let Err(e) = spawned {
                tracing::error!("cannot start a render thread: {e}");
            }
        }
        Self {
            jobs,
            done,
            newest,
            next_id: AtomicU64::new(1),
        }
    }

    /// A new generation of frames: everything made for older ones is now stale.
    pub fn new_generation(&self) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.newest.store(id, Ordering::Relaxed);
        id
    }

    pub fn request(&self, job: Job) {
        if self.jobs.send(job).is_err() {
            tracing::error!("render threads are gone");
        }
    }
}

fn work(jobs: &Mutex<Receiver<Job>>, done: &Sender<Done>, newest: &AtomicU64, picker: &Picker) {
    loop {
        // The lock is held only while waiting for a job, not while working.
        let job = match jobs.lock() {
            Ok(rx) => rx.recv(),
            Err(_) => return,
        };
        let Ok(job) = job else { return };
        if job.id < newest.load(Ordering::Relaxed) {
            continue; // for a size that is gone
        }
        let Some(image) = job.images.get(job.index) else {
            continue;
        };
        let k = job.key;
        let output = match k.kind {
            Kind::Blocks => {
                let img = scaled(
                    image,
                    u32::from(k.cols) * 2,
                    u32::from(k.rows) * 2,
                    k.fit,
                    k.crop,
                )
                .to_rgba8();
                Output::Blocks(blocks::quadrants(&img, k.cols, k.rows, job.background))
            }
            Kind::Ascii => Output::Ascii(ascii_lines(image, k.cols, k.rows, k.fit, k.crop)),
            Kind::Pixels { width, height } => {
                let img = scaled(image, width, height, k.fit, k.crop);
                let mut proto = picker.new_resize_protocol(img);
                proto.resize_encode(&Resize::Fit(None), Size::new(k.cols, k.rows));
                Output::Pixels(proto)
            }
        };
        let sent = done.send(Done {
            id: job.id,
            index: job.index,
            output,
        });
        if sent.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn images() -> Arc<Vec<DynamicImage>> {
        Arc::new(vec![DynamicImage::ImageRgba8(
            image::RgbaImage::from_pixel(16, 16, image::Rgba([200, 10, 10, 255])),
        )])
    }

    fn job(r: &Renderer, id: u64, kind: Kind) -> Job {
        let _ = r;
        Job {
            id,
            index: 0,
            images: images(),
            key: Key {
                cols: 10,
                rows: 5,
                fit: VisualFit::Fill,
                crop: 0,
                kind,
            },
            background: [0, 0, 0],
        }
    }

    fn wait(r: &Renderer) -> Option<Done> {
        r.done.recv_timeout(Duration::from_secs(5)).ok()
    }

    #[test]
    fn every_kind_is_rendered_in_the_background() {
        let r = Renderer::spawn(&Picker::halfblocks());
        let id = r.new_generation();
        for kind in [
            Kind::Blocks,
            Kind::Ascii,
            Kind::Pixels {
                width: 100,
                height: 50,
            },
        ] {
            r.request(job(&r, id, kind));
        }
        let mut seen = [false; 3];
        for _ in 0..3 {
            let done = wait(&r).expect("a frame arrives");
            assert_eq!((done.id, done.index), (id, 0));
            match done.output {
                Output::Blocks(g) => {
                    assert_eq!((g.cols, g.rows), (10, 5));
                    seen[0] = true;
                }
                Output::Ascii(lines) => {
                    assert_eq!(lines.len(), 5);
                    seen[1] = true;
                }
                Output::Pixels(_) => seen[2] = true,
            }
        }
        assert_eq!(seen, [true; 3]);
    }

    #[test]
    fn jobs_of_an_older_generation_are_skipped() {
        let r = Renderer::spawn(&Picker::halfblocks());
        let old = r.new_generation();
        let new = r.new_generation();
        assert!(new > old);
        r.request(job(&r, old, Kind::Blocks));
        r.request(job(&r, new, Kind::Blocks));
        let done = wait(&r).expect("the new one is made");
        assert_eq!(done.id, new);
        // Nothing else comes: the old job was dropped.
        let start = Instant::now();
        assert!(r.done.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(start.elapsed() >= Duration::from_millis(250));
    }
}
