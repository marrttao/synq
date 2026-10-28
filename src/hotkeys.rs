//! Keys that work while another program has the focus (a game in full
//! screen, a browser...). Windows only: it registers system-wide hotkeys on
//! a small thread with its own message loop. Elsewhere `supported()` is
//! false and nothing happens.

use std::sync::mpsc::Receiver;

use crate::keys::{Action, Chord};

#[derive(Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed(Action),
    /// A key could not be taken, e.g. another program owns it.
    Failed(String),
}

pub struct Hotkeys {
    pub events: Receiver<HotkeyEvent>,
    inner: imp::Inner,
}

impl Hotkeys {
    pub fn spawn() -> Option<Self> {
        let (events, inner) = imp::spawn()?;
        Some(Self { events, inner })
    }

    /// Replace everything that is registered with exactly these keys.
    pub fn set(&self, keys: Vec<(Action, Chord)>) {
        self.inner.set(keys);
    }
}

pub const fn supported() -> bool {
    cfg!(windows)
}

#[cfg(windows)]
mod imp {
    use std::ptr::null_mut;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey, VkKeyScanW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_APP, WM_HOTKEY,
        WM_QUIT, WM_USER,
    };

    use super::HotkeyEvent;
    use crate::keys::{Action, Chord};

    const WM_SET_KEYS: u32 = WM_APP + 1;

    type Pending = Arc<Mutex<Option<Vec<(Action, Chord)>>>>;

    pub struct Inner {
        thread_id: u32,
        pending: Pending,
    }

    impl Inner {
        pub fn set(&self, keys: Vec<(Action, Chord)>) {
            if let Ok(mut slot) = self.pending.lock() {
                *slot = Some(keys);
            }
            // SAFETY: plain message post to a thread id we got from the worker.
            let ok = unsafe { PostThreadMessageW(self.thread_id, WM_SET_KEYS, 0, 0) };
            if ok == 0 {
                tracing::error!("cannot reach the hotkey thread");
            }
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            // SAFETY: as above; the thread leaves its loop and unregisters.
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        }
    }

    pub fn spawn() -> Option<(Receiver<HotkeyEvent>, Inner)> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let pending: Pending = Arc::new(Mutex::new(None));
        let worker_pending = Arc::clone(&pending);
        let spawned = thread::Builder::new()
            .name("hotkeys".into())
            .spawn(move || run(&worker_pending, &tx, &ready_tx));
        if let Err(e) = spawned {
            tracing::error!("cannot start the hotkey thread: {e}");
            return None;
        }
        let thread_id = ready_rx.recv().ok()?;
        Some((rx, Inner { thread_id, pending }))
    }

    fn run(pending: &Pending, tx: &Sender<HotkeyEvent>, ready: &Sender<u32>) {
        // SAFETY: only documented Win32 calls on this thread's own queue.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            // Makes sure the thread has a message queue before anyone posts.
            PeekMessageW(&mut msg, null_mut(), WM_USER, WM_USER, PM_NOREMOVE);
            if ready.send(GetCurrentThreadId()).is_err() {
                return;
            }
            let mut table: Vec<Action> = Vec::new();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                match msg.message {
                    WM_HOTKEY => {
                        let id = msg.wParam as usize;
                        if let Some(action) = id.checked_sub(1).and_then(|i| table.get(i))
                            && tx.send(HotkeyEvent::Pressed(*action)).is_err()
                        {
                            break;
                        }
                    }
                    WM_SET_KEYS => {
                        for i in 0..table.len() {
                            UnregisterHotKey(null_mut(), (i + 1) as i32);
                        }
                        table.clear();
                        let keys = pending.lock().ok().and_then(|mut p| p.take());
                        for (action, chord) in keys.unwrap_or_default() {
                            let id = (table.len() + 1) as i32;
                            match register(id, chord) {
                                Ok(()) => table.push(action),
                                Err(why) => {
                                    tracing::warn!("hotkey {}: {why}", chord.display());
                                    let _ = tx.send(HotkeyEvent::Failed(format!(
                                        "Global {}: {why}",
                                        chord.display()
                                    )));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            for i in 0..table.len() {
                UnregisterHotKey(null_mut(), (i + 1) as i32);
            }
        }
    }

    unsafe fn register(id: i32, chord: Chord) -> Result<(), &'static str> {
        let (vk, shift) = virtual_key(chord.code).ok_or("this key cannot be global")?;
        let mut mods = MOD_NOREPEAT;
        if chord.mods.contains(KeyModifiers::CONTROL) {
            mods |= MOD_CONTROL;
        }
        if chord.mods.contains(KeyModifiers::ALT) {
            mods |= MOD_ALT;
        }
        if chord.mods.contains(KeyModifiers::SHIFT) || shift {
            mods |= MOD_SHIFT;
        }
        // SAFETY: no window handle is needed; messages go to this thread.
        if unsafe { RegisterHotKey(null_mut(), id, mods, vk) } == 0 {
            Err("is taken by another program")
        } else {
            Ok(())
        }
    }

    /// Windows key code, and whether Shift is needed to type the character.
    fn virtual_key(code: KeyCode) -> Option<(u32, bool)> {
        let plain = |v: u16| Some((u32::from(v), false));
        match code {
            KeyCode::Char(c) => {
                let mut buf = [0u16; 2];
                let unit = *c.encode_utf16(&mut buf).first()?;
                // SAFETY: pure lookup in the current keyboard layout.
                let r = unsafe { VkKeyScanW(unit) };
                if r == -1 {
                    return None;
                }
                Some((u32::from((r & 0xff) as u8), (r >> 8) & 1 != 0))
            }
            KeyCode::F(n @ 1..=24) => plain(0x6F + u16::from(n)),
            KeyCode::Backspace => plain(0x08),
            KeyCode::Tab => plain(0x09),
            KeyCode::Enter => plain(0x0D),
            KeyCode::PageUp => plain(0x21),
            KeyCode::PageDown => plain(0x22),
            KeyCode::End => plain(0x23),
            KeyCode::Home => plain(0x24),
            KeyCode::Left => plain(0x25),
            KeyCode::Up => plain(0x26),
            KeyCode::Right => plain(0x27),
            KeyCode::Down => plain(0x28),
            KeyCode::Insert => plain(0x2D),
            KeyCode::Delete => plain(0x2E),
            _ => None,
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::mpsc::Receiver;

    use super::HotkeyEvent;
    use crate::keys::{Action, Chord};

    pub struct Inner;

    impl Inner {
        pub fn set(&self, _keys: Vec<(Action, Chord)>) {}
    }

    pub fn spawn() -> Option<(Receiver<HotkeyEvent>, Inner)> {
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    use std::time::Duration;

    #[test]
    fn a_key_can_be_taken_once_and_is_freed_afterwards() {
        let chord = Chord::new(
            KeyCode::F(23),
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT,
        );
        let first = Hotkeys::spawn().unwrap();
        first.set(vec![(Action::Next, chord)]);
        assert!(
            first
                .events
                .recv_timeout(Duration::from_millis(300))
                .is_err(),
            "the first registration works"
        );

        let second = Hotkeys::spawn().unwrap();
        second.set(vec![(Action::Next, chord)]);
        match second.events.recv_timeout(Duration::from_secs(2)) {
            Ok(HotkeyEvent::Failed(msg)) => assert!(msg.contains("taken"), "{msg}"),
            other => panic!("expected a refusal, got {other:?}"),
        }

        // Letting go of the key makes it available again.
        first.set(Vec::new());
        std::thread::sleep(Duration::from_millis(200));
        second.set(vec![(Action::Next, chord)]);
        assert!(
            second
                .events
                .recv_timeout(Duration::from_millis(300))
                .is_err()
        );
    }
}
