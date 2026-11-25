use std::collections::VecDeque;

use parking_lot::{Condvar, Mutex};

const TYPING_MAX_LEN: usize = 4 * 1024;
const TYPING_QUEUE_LIMIT: usize = 64 * 1024;
/// Larger input (a paste) is admitted while the queue is empty, or while everything fits in this.
const PASTE_QUEUE_LIMIT: usize = 1024 * 1024;
/// Replies the program has asked for but not read; more are dropped, since holding back the
/// reader that produces them could deadlock output against input.
const REPLY_LIMIT: usize = 64 * 1024;
/// The writer hands the pseudo console at most this much at a time, so closing can stop a paste.
pub(crate) const WRITE_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Typing,
    Paste,
    Reply,
}

#[derive(Debug)]
pub(crate) struct Item {
    pub(crate) bytes: Vec<u8>,
    pub(crate) kind: Kind,
}

#[derive(Debug, Default)]
struct State {
    items: VecDeque<Item>,
    queued: usize,
    typing: usize,
    replies: usize,
    is_closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rejected {
    pub(crate) queued: usize,
}

#[derive(Debug, Default)]
pub(crate) struct InputQueue {
    state: Mutex<State>,
    available: Condvar,
}

impl InputQueue {
    pub(crate) fn closed() -> Self {
        Self { state: Mutex::new(State { is_closed: true, ..State::default() }), available: Condvar::new() }
    }

    pub(crate) fn push(&self, bytes: Vec<u8>) -> Result<(), Rejected> {
        let len = bytes.len();
        let mut state = self.state.lock();
        if state.is_closed || len == 0 {
            return Ok(());
        }
        let (kind, is_admitted) = if len <= TYPING_MAX_LEN {
            (Kind::Typing, state.typing + len <= TYPING_QUEUE_LIMIT)
        } else {
            (Kind::Paste, state.queued == 0 || state.queued + len <= PASTE_QUEUE_LIMIT)
        };
        if !is_admitted {
            return Err(Rejected { queued: state.queued });
        }
        Self::add(&mut state, Item { bytes, kind });
        drop(state);
        self.available.notify_one();
        Ok(())
    }

    /// Queues a reply the program asked for, or drops it over [`REPLY_LIMIT`].
    pub(crate) fn push_reply(&self, bytes: Vec<u8>) {
        let mut state = self.state.lock();
        if state.is_closed || state.replies + bytes.len() > REPLY_LIMIT {
            return;
        }
        Self::add(&mut state, Item { bytes, kind: Kind::Reply });
        drop(state);
        self.available.notify_one();
    }

    fn add(state: &mut State, item: Item) {
        let len = item.bytes.len();
        state.queued += len;
        match item.kind {
            Kind::Typing => state.typing += len,
            Kind::Reply => state.replies += len,
            Kind::Paste => {}
        }
        state.items.push_back(item);
    }

    pub(crate) fn next(&self) -> Option<Item> {
        let mut state = self.state.lock();
        loop {
            if state.is_closed {
                return None;
            }
            if let Some(item) = state.items.pop_front() {
                return Some(item);
            }
            self.available.wait(&mut state);
        }
    }

    pub(crate) fn written(&self, len: usize, kind: Kind) {
        let mut state = self.state.lock();
        state.queued = state.queued.saturating_sub(len);
        match kind {
            Kind::Typing => state.typing = state.typing.saturating_sub(len),
            Kind::Reply => state.replies = state.replies.saturating_sub(len),
            Kind::Paste => {}
        }
    }

    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.state.lock().queued
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.state.lock().is_closed
    }

    pub(crate) fn close(&self) {
        let mut state = self.state.lock();
        *state = State { is_closed: true, ..State::default() };
        drop(state);
        self.available.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_paste_is_admitted_alone_and_typing_waits_behind_it() {
        let queue = InputQueue::default();
        assert_eq!(queue.push(vec![b'p'; 5 * PASTE_QUEUE_LIMIT]), Ok(()));
        assert_eq!(queue.push(vec![b'q'; 2 * TYPING_MAX_LEN]), Err(Rejected { queued: 5 * PASTE_QUEUE_LIMIT }));
        assert_eq!(queue.push(b"k".to_vec()), Ok(()));
        let first = queue.next().expect("item");
        assert_eq!((first.bytes.len(), first.kind), (5 * PASTE_QUEUE_LIMIT, Kind::Paste));
        queue.written(first.bytes.len(), first.kind);
        assert_eq!(queue.next().map(|i| i.bytes), Some(b"k".to_vec()));
    }

    #[test]
    fn typing_and_replies_have_their_own_bounds() {
        let queue = InputQueue::default();
        for _ in 0..TYPING_QUEUE_LIMIT / TYPING_MAX_LEN {
            assert_eq!(queue.push(vec![b'k'; TYPING_MAX_LEN]), Ok(()));
        }
        assert!(queue.push(b"k".to_vec()).is_err());
        queue.push_reply(vec![b'r'; REPLY_LIMIT]);
        queue.push_reply(b"r".to_vec());
        assert_eq!(queue.queued(), TYPING_QUEUE_LIMIT + REPLY_LIMIT, "the reply over the bound is dropped");
        let item = queue.next().expect("item");
        queue.written(item.bytes.len(), item.kind);
        assert_eq!(queue.push(b"k".to_vec()), Ok(()));
    }

    #[test]
    fn closing_drops_everything_and_stops_the_writer() {
        let queue = std::sync::Arc::new(InputQueue::default());
        queue.push(b"x".to_vec()).expect("push");
        queue.close();
        assert_eq!(queue.queued(), 0);
        assert!(queue.next().is_none());
        assert_eq!(queue.push(b"y".to_vec()), Ok(()), "input after the end is dropped quietly");
        assert_eq!(queue.queued(), 0);
        let open = std::sync::Arc::new(InputQueue::default());
        let waiter = std::sync::Arc::clone(&open);
        let writer = std::thread::spawn(move || waiter.next().is_none());
        std::thread::sleep(std::time::Duration::from_millis(50));
        open.close();
        assert!(writer.join().expect("writer"));
    }
}
