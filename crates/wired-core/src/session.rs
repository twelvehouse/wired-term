use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use parking_lot::{Condvar, Mutex, MutexGuard};
use wired_pty::Pty;

use crate::{SessionEnd, TerminalEvent};

const CLOSE_DRAIN: Duration = Duration::from_secs(1);

/// Closes the pseudo console. It is taken out of the slot first and dropped after the lock is
/// released: closing can block, and resizes from the UI need the lock meanwhile.
pub(crate) fn close_console(pty: &Mutex<Option<Pty>>) {
    let console = pty.lock().take();
    drop(console);
}

type OnEvent = Arc<dyn Fn(TerminalEvent) + Send + Sync>;

#[derive(Debug, Default)]
struct State {
    is_output_done: bool,
    has_exited: bool,
    is_closing: bool,
    is_reported: bool,
    is_output_cut: bool,
}

pub(crate) struct Session {
    state: Mutex<State>,
    changed: Condvar,
    on_event: OnEvent,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("state", &*self.state.lock()).finish_non_exhaustive()
    }
}

impl Session {
    pub(crate) fn new(on_event: OnEvent) -> Self {
        Self { state: Mutex::new(State::default()), changed: Condvar::new(), on_event }
    }

    fn report(&self, mut state: MutexGuard<'_, State>, end: SessionEnd) {
        if state.is_reported || state.is_closing {
            return;
        }
        state.is_reported = true;
        drop(state);
        (self.on_event)(TerminalEvent::Ended(end));
    }

    pub(crate) fn output_ended(&self, error: Option<String>) {
        let mut state = self.state.lock();
        state.is_output_done = true;
        self.changed.notify_all();
        if let Some(error) = error
            && !state.has_exited
        {
            self.report(state, SessionEnd::Failed(error));
        }
    }

    pub(crate) fn failed(&self, message: String) {
        let mut state = self.state.lock();
        state.is_output_done = true;
        self.changed.notify_all();
        self.report(state, SessionEnd::Failed(message));
    }

    /// Closing the pseudo console flushes it and ends the reader's pipe, which the console never
    /// does by itself while it is open.
    pub(crate) fn exited(&self, code: Option<u32>, pty: &Mutex<Option<Pty>>) {
        {
            let mut state = self.state.lock();
            state.has_exited = true;
            if state.is_closing {
                return;
            }
        }
        close_console(pty);
        let is_output_complete = self.wait_for_output(CLOSE_DRAIN);
        let mut state = self.state.lock();
        state.is_output_cut = !is_output_complete;
        self.report(state, SessionEnd::Exited { code, is_output_complete });
    }

    pub(crate) fn is_reading_stopped(&self) -> bool {
        let state = self.state.lock();
        state.is_output_cut || state.is_closing
    }

    fn wait_for_output(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock();
        while !state.is_output_done && !state.is_closing {
            if self.changed.wait_until(&mut state, deadline).timed_out() {
                break;
            }
        }
        state.is_output_done
    }

    pub(crate) fn close(&self) {
        self.state.lock().is_closing = true;
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> (Session, Arc<Mutex<Vec<TerminalEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        (Session::new(Arc::new(move |e| sink.lock().push(e))), events)
    }

    #[test]
    fn the_end_is_reported_once_after_the_output() {
        let (session, events) = recording();
        session.output_ended(None);
        session.exited(Some(3), &Mutex::new(None));
        session.failed("late".into());
        let end = SessionEnd::Exited { code: Some(3), is_output_complete: true };
        assert_eq!(*events.lock(), [TerminalEvent::Ended(end)]);
        assert!(!session.is_reading_stopped());
    }

    #[test]
    fn exit_waits_for_the_output_to_finish() {
        let (session, events) = recording();
        let session = Arc::new(session);
        let reader = Arc::clone(&session);
        let start = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            reader.output_ended(Some("pipe closed".into()));
        });
        session.exited(Some(0), &Mutex::new(None));
        assert!(start.elapsed() >= Duration::from_millis(90));
        let end = SessionEnd::Exited { code: Some(0), is_output_complete: true };
        assert_eq!(*events.lock(), [TerminalEvent::Ended(end)]);
    }

    #[test]
    fn output_that_does_not_end_in_time_is_cut() {
        let (session, events) = recording();
        let start = Instant::now();
        session.exited(Some(0), &Mutex::new(None));
        assert!(start.elapsed() >= CLOSE_DRAIN);
        let end = SessionEnd::Exited { code: Some(0), is_output_complete: false };
        assert_eq!(*events.lock(), [TerminalEvent::Ended(end)]);
        assert!(session.is_reading_stopped(), "nothing may reach the screen after the end");
    }

    #[test]
    fn a_read_error_while_running_is_a_failure_and_closing_reports_nothing() {
        let (session, events) = recording();
        session.output_ended(Some("broken".into()));
        assert_eq!(*events.lock(), [TerminalEvent::Ended(SessionEnd::Failed("broken".into()))]);
        let (session, events) = recording();
        session.close();
        let start = Instant::now();
        session.exited(None, &Mutex::new(None));
        assert!(start.elapsed() < CLOSE_DRAIN / 4, "closing does not wait for output");
        assert!(events.lock().is_empty());
    }
}
