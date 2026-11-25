#![allow(clippy::print_stdout)] // Printing the screen is the purpose of this tool.

use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use wired_core::{Terminal, TerminalConfig, TerminalEvent};
use wired_pty::{PtyDimensions, SpawnConfig};
use wired_vt::{GridSnapshot, VtOptions};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(program) = args.next() else {
        println!("usage: screen_dump <absolute program path> [args...]");
        return;
    };
    let config = TerminalConfig {
        spawn: SpawnConfig { program: PathBuf::from(program), args: args.collect(), ..SpawnConfig::default() },
        size: PtyDimensions { cols: 100, rows: 20, ..PtyDimensions::default() },
        vt: VtOptions::default(),
    };
    let (tx, rx) = mpsc::channel();
    let terminal = match Terminal::spawn(config, move |event| {
        let _ = tx.send(event);
    }) {
        Ok(terminal) => terminal,
        Err(error) => {
            println!("spawn failed: {error}");
            return;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(4);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(TerminalEvent::Wakeup) => terminal.ack_wakeup(),
            Ok(TerminalEvent::Ended(end)) => {
                println!("[ended: {end:?}]");
                break;
            }
            Err(_) => break,
        }
    }
    let mut snapshot = GridSnapshot::default();
    if terminal.lock().snapshot(&mut snapshot).is_ok() {
        for row in &snapshot.rows {
            println!("|{}", row.text.trim_end());
        }
    }
}
