//! A running terminal: [`wired_vt::Vt`] fed by a pseudo console through background threads.
#![forbid(unsafe_code)]

mod input;
mod notice;
mod session;

use std::{
    io::{ErrorKind, Read, Write},
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use parking_lot::{Mutex, MutexGuard};
use wired_pty::{Killer, Pty, PtyDimensions, SpawnConfig};
use wired_vt::{Vt, VtOptions};

use crate::{
    input::{InputQueue, WRITE_CHUNK},
    notice::render_notices,
    session::{Session, close_console},
};

pub use notice::{Notice, NoticeKind};
pub use wired_pty::PtyDimensions as Dimensions;

/// Errors from starting or controlling a terminal.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// VT engine error.
    #[error(transparent)]
    Vt(#[from] wired_vt::Error),
    /// Pseudo console error.
    #[error(transparent)]
    Pty(#[from] wired_pty::Error),
    /// Thread creation failed.
    #[error("failed to start terminal thread: {0}")]
    Thread(#[from] std::io::Error),
    /// Input was not sent: the program has not taken earlier input yet.
    #[error("not sent: the program has not read the last {queued} bytes of input yet")]
    InputFull {
        /// Bytes still waiting for the program.
        queued: usize,
    },
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Notifications for the UI, delivered from background threads. At most one `Wakeup` is
/// pending and `Ended` comes once, so the UI's queue never grows with output volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    /// New output, a new title or the bell (see [`Terminal::take_title`] and
    /// [`Terminal::take_bell`]); redraw. Not sent again until [`Terminal::ack_wakeup`].
    Wakeup,
    /// The session ended; everything the program wrote has been processed before this.
    Ended(SessionEnd),
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// The child process exited.
    Exited {
        /// Its exit code (`None` if it could not be determined).
        code: Option<u32>,
        /// `false` when the rest of its output did not arrive in time and was cut off.
        is_output_complete: bool,
    },
    /// Reading the program's output failed; the screen no longer updates.
    Failed(String),
}

/// Everything needed to start a terminal.
#[derive(Debug, Clone, Default)]
pub struct TerminalConfig {
    /// Program to run.
    pub spawn: SpawnConfig,
    /// Initial grid size.
    pub size: PtyDimensions,
    /// VT engine settings.
    pub vt: VtOptions,
}

fn terminal_env() -> [(String, String); 4] {
    [
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("TERM_PROGRAM".into(), "wired".into()),
        ("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into()),
    ]
}

const READ_CHUNK: usize = 64 * 1024;
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
struct Signals {
    is_wakeup_pending: AtomicBool,
    title: Mutex<Option<String>>,
    has_bell: AtomicBool,
}

/// A terminal session. Dropping it kills the child process and stops the threads in the
/// background; the drop itself returns at once.
pub struct Terminal {
    vt: Arc<Mutex<Vt>>,
    pty: Arc<Mutex<Option<Pty>>>,
    input: Arc<InputQueue>,
    signals: Arc<Signals>,
    closing: Option<Closing>,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal").finish_non_exhaustive()
    }
}

impl Terminal {
    /// Spawns the program and starts the I/O threads. `on_event` is called from those threads.
    /// If a thread cannot start, everything started so far is closed as on drop.
    pub fn spawn(config: TerminalConfig, on_event: impl Fn(TerminalEvent) + Send + Sync + 'static) -> Result<Self> {
        let on_event: Arc<dyn Fn(TerminalEvent) + Send + Sync> = Arc::new(on_event);
        let vt = Arc::new(Mutex::new(Vt::new(config.size.cols, config.size.rows, &config.vt)?));

        let mut spawn = config.spawn;
        spawn.env.extend(terminal_env());
        let spawned = wired_pty::spawn(&spawn, config.size)?;
        let input = Arc::new(InputQueue::default());
        let signals = Arc::new(Signals::default());
        let session = Arc::new(Session::new(Arc::clone(&on_event)));
        let pty = Arc::new(Mutex::new(Some(spawned.pty)));
        let mut closing = Closing {
            input: Arc::clone(&input),
            session: Arc::clone(&session),
            killer: Some(spawned.child.killer()),
            pty: Arc::clone(&pty),
            threads: Vec::new(),
        };
        closing.threads.push(spawn_writer(spawned.writer, Arc::clone(&input))?);
        closing.threads.push(spawn_reader(
            spawned.reader,
            Arc::clone(&vt),
            Arc::clone(&input),
            Arc::clone(&signals),
            Arc::clone(&session),
            on_event,
        )?);
        closing.threads.push(spawn_waiter(spawned.child, session, Arc::clone(&pty))?);
        Ok(Self { vt, pty, input, signals, closing: Some(closing) })
    }

    /// A screen without a program, for a pane whose program could not start: it shows notices
    /// and ignores input.
    pub fn without_program(size: PtyDimensions, vt: &VtOptions) -> Result<Self> {
        Ok(Self {
            vt: Arc::new(Mutex::new(Vt::new(size.cols, size.rows, vt)?)),
            pty: Arc::new(Mutex::new(None)),
            input: Arc::new(InputQueue::closed()),
            signals: Arc::new(Signals::default()),
            closing: None,
        })
    }

    /// Sends bytes to the program (keys, encoded paste, IME commits, mouse reports), after
    /// everything sent before. Typing is always taken unless the program has stopped reading;
    /// a paste is taken whole, but not while another large input still waits
    /// ([`Error::InputFull`]). After the program ended, input is dropped.
    pub fn write(&self, bytes: Vec<u8>) -> Result<()> {
        self.input.push(bytes).map_err(|r| Error::InputFull { queued: r.queued })
    }

    #[cfg(test)]
    fn queued_input(&self) -> usize {
        self.input.queued()
    }

    /// Locks the VT state (snapshot, scroll, key encoding). Keep the guard short-lived.
    pub fn lock(&self) -> MutexGuard<'_, Vt> {
        self.vt.lock()
    }

    /// Writes `notices` into the screen (not to the program) and shows the bottom, where they
    /// are. The caller redraws.
    pub fn write_notices(&self, notices: &[Notice]) {
        let mut vt = self.vt.lock();
        vt.feed(&render_notices(notices));
        vt.scroll_viewport(wired_vt::Scroll::Bottom);
    }

    /// Allows the next [`TerminalEvent::Wakeup`]. Call when handling one, before reading state.
    pub fn ack_wakeup(&self) {
        self.signals.is_wakeup_pending.store(false, Ordering::Release);
    }

    /// The title the program set since the last call (sanitized), if it set one.
    pub fn take_title(&self) -> Option<String> {
        self.signals.title.lock().take()
    }

    /// Whether the program rang the bell since the last call (any number of times).
    pub fn take_bell(&self) -> bool {
        self.signals.has_bell.swap(false, Ordering::AcqRel)
    }

    /// Resizes VT state (with reflow) and the pseudo console together. If the console cannot be
    /// resized, the VT goes back to its previous size, so both keep agreeing.
    pub fn resize(&self, size: PtyDimensions) -> Result<()> {
        let previous = {
            let mut vt = self.vt.lock();
            let previous = vt.size();
            vt.resize(size.cols, size.rows, u32::from(size.cell_width_px), u32::from(size.cell_height_px))?;
            previous
        };
        // The VT lock is not held here: the console may wait for the reader, which takes it.
        let resized = self.pty.lock().as_ref().map_or(Ok(()), |pty| pty.resize(size));
        if let Err(error) = resized {
            let (cols, rows, cell_width_px, cell_height_px) = previous;
            self.vt.lock().resize(cols, rows, cell_width_px, cell_height_px)?;
            return Err(error.into());
        }
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let Some(closing) = self.closing.take() else { return };
        // Closing can block (a write stuck in a full pipe, ClosePseudoConsole draining output), so
        // it runs on its own thread.
        let _ = thread::Builder::new().name("wired-pty-closer".into()).spawn(move || drop(closing));
    }
}

/// Everything a session owns that needs stopping, from the moment the child starts. Dropping it
/// closes: stop reporting and input, kill the child (a live child could keep the console busy),
/// close the console (this unblocks reads and writes), then join the threads until the deadline.
struct Closing {
    input: Arc<InputQueue>,
    session: Arc<Session>,
    killer: Option<Killer>,
    pty: Arc<Mutex<Option<Pty>>>,
    threads: Vec<JoinHandle<()>>,
}

impl Drop for Closing {
    fn drop(&mut self) {
        let deadline = Instant::now() + CLOSE_TIMEOUT;
        self.session.close();
        self.input.close();
        if let Some(mut killer) = self.killer.take() {
            let _ = killer.kill();
        }
        close_console(&self.pty);
        while self.threads.iter().any(|t| !t.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        for thread in self.threads.drain(..) {
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

fn spawn_writer(mut writer: Box<dyn Write + Send>, input: Arc<InputQueue>) -> Result<JoinHandle<()>> {
    Ok(thread::Builder::new().name("wired-pty-writer".into()).spawn(move || {
        while let Some(item) = input.next() {
            for piece in item.bytes.chunks(WRITE_CHUNK) {
                if input.is_closed() {
                    return;
                }
                let result = writer.write_all(piece).and_then(|()| writer.flush());
                input.written(piece.len(), item.kind);
                if result.is_err() {
                    input.close();
                    return;
                }
            }
        }
    })?)
}

fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    vt: Arc<Mutex<Vt>>,
    input: Arc<InputQueue>,
    signals: Arc<Signals>,
    session: Arc<Session>,
    on_event: Arc<dyn Fn(TerminalEvent) + Send + Sync>,
) -> Result<JoinHandle<()>> {
    Ok(thread::Builder::new().name("wired-pty-reader".into()).spawn(move || {
        let ended = Arc::clone(&session);
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || {
            let mut buf = vec![0u8; READ_CHUNK];
            let error = loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) => break None,
                    // A closed pipe is the end of the output, not a failure.
                    Err(e) if e.kind() == ErrorKind::BrokenPipe => break None,
                    Err(e) => break Some(format!("reading the program's output failed: {e}")),
                    Ok(n) => n,
                };
                let mut guard = vt.lock();
                // Asked under the screen's lock, so no chunk lands after the end notice.
                if session.is_reading_stopped() {
                    break None;
                }
                guard.feed(&buf[..n]);
                let replies = guard.take_replies();
                let title = guard.take_title_change();
                let bell = guard.take_bell();
                // Fair unlock: lets the UI thread take the lock between chunks during floods.
                MutexGuard::unlock_fair(guard);

                if !replies.is_empty() {
                    input.push_reply(replies);
                }
                if let Some(title) = title {
                    *signals.title.lock() = Some(title);
                }
                if bell {
                    signals.has_bell.store(true, Ordering::Release);
                }
                if !signals.is_wakeup_pending.swap(true, Ordering::AcqRel) {
                    on_event(TerminalEvent::Wakeup);
                }
            };
            session.output_ended(error);
        }));
        if let Err(panic) = outcome {
            let message = panic
                .downcast_ref::<&str>()
                .map(ToString::to_string)
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            ended.failed(message);
        }
    })?)
}

fn spawn_waiter(
    mut child: wired_pty::ChildProcess,
    session: Arc<Session>,
    pty: Arc<Mutex<Option<Pty>>>,
) -> Result<JoinHandle<()>> {
    Ok(thread::Builder::new().name("wired-pty-waiter".into()).spawn(move || {
        let code = child.wait().ok();
        session.exited(code, &pty);
    })?)
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::mpsc, time::Duration};

    use super::*;

    fn cmd_exe() -> PathBuf {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        root.join("System32").join("cmd.exe")
    }

    fn cmd(command: &str) -> TerminalConfig {
        TerminalConfig {
            spawn: SpawnConfig {
                program: cmd_exe(),
                args: vec!["/c".into(), command.into()],
                ..SpawnConfig::default()
            },
            size: PtyDimensions { cols: 80, rows: 10, ..PtyDimensions::default() },
            vt: VtOptions::default(),
        }
    }

    fn screen_text(terminal: &Terminal) -> String {
        let mut snap = wired_vt::GridSnapshot::default();
        terminal.lock().snapshot(&mut snap).expect("snapshot");
        snap.rows.iter().map(|r| r.text.clone()).collect()
    }

    #[test]
    fn the_end_comes_once_after_all_output() {
        let (tx, rx) = mpsc::channel();
        let terminal = Terminal::spawn(cmd("echo wired-ok & exit 3"), move |event| {
            let _ = tx.send(event);
        })
        .expect("spawn");
        let mut ended = Vec::new();
        while let Ok(event) = rx.recv_timeout(Duration::from_secs(10)) {
            match event {
                TerminalEvent::Wakeup => terminal.ack_wakeup(),
                TerminalEvent::Ended(end) => {
                    ended.push(end);
                    let text = screen_text(&terminal);
                    assert!(text.contains("wired-ok"), "screen: {text}");
                    break;
                }
            }
        }
        assert_eq!(ended, [SessionEnd::Exited { code: Some(3), is_output_complete: true }]);
        while let Ok(event) = rx.recv_timeout(Duration::from_millis(300)) {
            assert_eq!(event, TerminalEvent::Wakeup, "the end is reported only once");
        }
    }

    #[test]
    fn title_and_bell_are_kept_until_taken() {
        let (tx, rx) = mpsc::channel();
        let terminal = Terminal::spawn(cmd("title first&title second&echo \x07"), move |event| {
            let _ = tx.send(event);
        })
        .expect("spawn");
        while let Ok(event) = rx.recv_timeout(Duration::from_secs(10)) {
            if let TerminalEvent::Ended(_) = event {
                break;
            }
            terminal.ack_wakeup();
        }
        let title = terminal.take_title().expect("title");
        // An elevated console prefixes the title with "Administrator: ".
        assert!(title.ends_with("second") && !title.contains("first"), "{title}");
        assert_eq!(terminal.take_title(), None);
        assert!(terminal.take_bell());
        assert!(!terminal.take_bell());
    }

    #[test]
    fn dropping_never_blocks_on_a_stuck_child() {
        let terminal = Terminal::spawn(cmd("ping -n 30 127.0.0.1 >nul"), |_| {}).expect("spawn");
        terminal.write(vec![b'x'; 8 * 1024 * 1024]).expect("a paste into an empty queue is taken");
        std::thread::sleep(Duration::from_millis(300));
        assert!(terminal.queued_input() > 0, "the child should not have taken everything");
        assert!(matches!(terminal.write(vec![b'y'; 64 * 1024]), Err(Error::InputFull { .. })));
        terminal.write(b"k".to_vec()).expect("typing");
        let start = Instant::now();
        drop(terminal);
        assert!(start.elapsed() < Duration::from_millis(200), "drop took {:?}", start.elapsed());
    }

    #[test]
    fn closing_kills_the_child_and_stops_the_threads() {
        let spawned =
            wired_pty::spawn(&cmd("ping -n 30 127.0.0.1 >nul").spawn, PtyDimensions::default()).expect("spawn");
        let on_event: Arc<dyn Fn(TerminalEvent) + Send + Sync> = Arc::new(|_| {});
        let input = Arc::new(InputQueue::default());
        let mut child = spawned.child;
        let closing = Closing {
            input: Arc::clone(&input),
            session: Arc::new(Session::new(on_event)),
            killer: Some(child.killer()),
            pty: Arc::new(Mutex::new(Some(spawned.pty))),
            threads: vec![spawn_writer(spawned.writer, Arc::clone(&input)).expect("writer")],
        };
        let start = Instant::now();
        drop(closing);
        assert!(child.wait().is_ok());
        assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
        assert!(input.is_closed());
    }

    #[test]
    fn a_screen_without_program_shows_notices_and_drops_input() {
        let size = PtyDimensions { cols: 80, rows: 10, ..PtyDimensions::default() };
        let terminal = Terminal::without_program(size, &VtOptions::default()).expect("screen");
        terminal.write(b"ignored".to_vec()).expect("dropped quietly");
        assert_eq!(terminal.queued_input(), 0);
        terminal.resize(PtyDimensions { cols: 60, ..size }).expect("resize");
        terminal.write_notices(&[Notice::error("cannot start")]);
        let text = screen_text(&terminal);
        assert!(text.contains("cannot start"), "screen: {text}");
    }
}
