//! Play queue: order, shuffle and repeat. Pure logic, no audio.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

/// What the queue needs to know about a track.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueItem {
    /// Library track id.
    pub id: i64,
    pub path: PathBuf,
}

/// Tiny xorshift generator: shuffling needs no cryptographic quality, and
/// this avoids a dependency.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // xorshift must not start at zero.
        Self(seed | 1)
    }

    pub fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64);
        Self::new(nanos)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Fisher-Yates.
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Queue {
    items: Vec<QueueItem>,
    /// Indices into `items` in play order.
    order: Vec<usize>,
    /// Position in `order` of the current track.
    pos: usize,
    shuffle: bool,
    repeat: Repeat,
    rng: Rng,
}

impl Queue {
    /// `start` is an index into `items`; it plays first even when shuffled.
    pub fn new(
        items: Vec<QueueItem>,
        start: usize,
        shuffle: bool,
        repeat: Repeat,
        rng: Rng,
    ) -> Self {
        let start = start.min(items.len().saturating_sub(1));
        let mut q = Self {
            order: (0..items.len()).collect(),
            items,
            pos: 0,
            shuffle,
            repeat,
            rng,
        };
        q.rebuild_order(start);
        q
    }

    /// Lay out `order` so that `current` is the current track.
    fn rebuild_order(&mut self, current: usize) {
        if self.shuffle {
            let mut rest: Vec<usize> = (0..self.items.len()).filter(|&i| i != current).collect();
            self.rng.shuffle(&mut rest);
            self.order = Vec::with_capacity(self.items.len());
            if current < self.items.len() {
                self.order.push(current);
            }
            self.order.extend(rest);
            self.pos = 0;
        } else {
            self.order = (0..self.items.len()).collect();
            self.pos = current.min(self.items.len().saturating_sub(1));
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn current(&self) -> Option<&QueueItem> {
        self.order.get(self.pos).and_then(|&i| self.items.get(i))
    }

    fn current_index(&self) -> usize {
        self.order.get(self.pos).copied().unwrap_or(0)
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffle != on {
            let current = self.current_index();
            self.shuffle = on;
            self.rebuild_order(current);
        }
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    /// Move to the next track. `auto` is true when the previous track ended
    /// by itself (repeat-one then replays it). Returns `None` at the end of
    /// the queue when repeat is off.
    pub fn next(&mut self, auto: bool) -> Option<&QueueItem> {
        if self.items.is_empty() {
            return None;
        }
        if auto && self.repeat == Repeat::One {
            return self.current();
        }
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
        } else if self.repeat == Repeat::Off && auto {
            return None;
        } else {
            // Wrap around (a manual "next" at the end also wraps).
            let last = self.current_index();
            if self.shuffle {
                self.rebuild_order(last);
                // Avoid replaying the same track right after the wrap.
                if self.order.len() > 1 {
                    self.pos = 1;
                }
            } else {
                self.pos = 0;
            }
        }
        self.current()
    }

    /// Move to the previous track; at the start it stays on the first one.
    pub fn prev(&mut self) -> Option<&QueueItem> {
        if self.pos > 0 {
            self.pos -= 1;
        } else if self.repeat == Repeat::All && !self.order.is_empty() {
            self.pos = self.order.len() - 1;
        }
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: i64) -> Vec<QueueItem> {
        (0..n)
            .map(|id| QueueItem {
                id,
                path: PathBuf::from(format!("{id}.mp3")),
            })
            .collect()
    }

    fn queue(n: i64, start: usize, shuffle: bool, repeat: Repeat) -> Queue {
        Queue::new(items(n), start, shuffle, repeat, Rng::new(42))
    }

    fn id(q: Option<&QueueItem>) -> Option<i64> {
        q.map(|i| i.id)
    }

    #[test]
    fn plays_in_order_and_stops_at_end() {
        let mut q = queue(3, 0, false, Repeat::Off);
        assert_eq!(id(q.current()), Some(0));
        assert_eq!(id(q.next(true)), Some(1));
        assert_eq!(id(q.next(true)), Some(2));
        assert_eq!(id(q.next(true)), None);
    }

    #[test]
    fn start_index_is_respected() {
        let q = queue(5, 3, false, Repeat::Off);
        assert_eq!(id(q.current()), Some(3));
        let q = queue(5, 3, true, Repeat::Off);
        assert_eq!(id(q.current()), Some(3));
    }

    #[test]
    fn repeat_all_wraps() {
        let mut q = queue(2, 1, false, Repeat::All);
        assert_eq!(id(q.next(true)), Some(0));
        assert_eq!(id(q.next(true)), Some(1));
    }

    #[test]
    fn repeat_one_replays_on_auto_but_manual_next_moves_on() {
        let mut q = queue(3, 0, false, Repeat::One);
        assert_eq!(id(q.next(true)), Some(0));
        assert_eq!(id(q.next(false)), Some(1));
    }

    #[test]
    fn manual_next_wraps_even_without_repeat() {
        let mut q = queue(2, 1, false, Repeat::Off);
        assert_eq!(id(q.next(false)), Some(0));
    }

    #[test]
    fn prev_goes_back_and_stays_at_start() {
        let mut q = queue(3, 2, false, Repeat::Off);
        assert_eq!(id(q.prev()), Some(1));
        assert_eq!(id(q.prev()), Some(0));
        assert_eq!(id(q.prev()), Some(0));
    }

    #[test]
    fn prev_wraps_with_repeat_all() {
        let mut q = queue(3, 0, false, Repeat::All);
        assert_eq!(id(q.prev()), Some(2));
    }

    #[test]
    fn shuffle_visits_every_track_once() {
        let mut q = queue(20, 7, true, Repeat::Off);
        let mut seen = vec![id(q.current()).unwrap()];
        while let Some(i) = id(q.next(true)) {
            seen.push(i);
        }
        assert_eq!(seen[0], 7);
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..20).collect::<Vec<_>>());
        assert_ne!(seen, (0..20).collect::<Vec<_>>(), "order should differ");
    }

    #[test]
    fn toggling_shuffle_keeps_current_track() {
        let mut q = queue(10, 4, false, Repeat::Off);
        q.set_shuffle(true);
        assert_eq!(id(q.current()), Some(4));
        q.set_shuffle(false);
        assert_eq!(id(q.current()), Some(4));
        assert_eq!(id(q.next(true)), Some(5));
    }

    #[test]
    fn shuffle_wrap_does_not_repeat_the_last_track_immediately() {
        let mut q = queue(5, 0, true, Repeat::All);
        let mut last = id(q.current());
        for _ in 0..40 {
            let cur = id(q.next(true));
            assert_ne!(cur, last);
            last = cur;
        }
    }

    #[test]
    fn empty_queue_is_safe() {
        let mut q = queue(0, 0, true, Repeat::All);
        assert!(q.current().is_none());
        assert!(q.next(true).is_none());
        assert!(q.next(false).is_none());
        assert!(q.prev().is_none());
    }

    #[test]
    fn single_track_with_shuffle_and_repeat_all() {
        let mut q = queue(1, 0, true, Repeat::All);
        assert_eq!(id(q.next(true)), Some(0));
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(Repeat::Off.cycle(), Repeat::All);
        assert_eq!(Repeat::All.cycle(), Repeat::One);
        assert_eq!(Repeat::One.cycle(), Repeat::Off);
    }
}
