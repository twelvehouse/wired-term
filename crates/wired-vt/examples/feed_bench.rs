#![allow(clippy::print_stdout)] // Printing the result is the purpose of this tool.

use std::time::Instant;

use wired_vt::{GridSnapshot, Vt, VtOptions};

const CHUNK: usize = 64 * 1024;
const SNAPSHOT_EVERY: usize = 512 * 1024;
const TARGET_BYTES: usize = 64 * 1024 * 1024;
const RUNS: usize = 5;

fn sample() -> Vec<u8> {
    let mut out = Vec::with_capacity(TARGET_BYTES + 4096);
    let mut i: u64 = 0;
    while out.len() < TARGET_BYTES {
        let line = match i % 5 {
            0..=2 => format!(
                "\x1b[2m2026-11-17 12:{:02}:{:02}\x1b[0m \x1b[32mINFO\x1b[0m \x1b[36mwired_core::reader\x1b[0m read {} bytes from pty {}\r\n",
                i % 60,
                (i * 7) % 60,
                i * 13 % 65536,
                i
            ),
            3 => format!("日本語のログ行 {i}: ターミナルの出力を流して速さを測ります。\r\n"),
            _ => format!("{}\r\n", "lorem ipsum dolor sit amet consectetur ".repeat(4)),
        };
        out.extend_from_slice(line.as_bytes());
        i += 1;
    }
    out
}

fn run(data: &[u8]) -> f64 {
    let mut vt = Vt::new(200, 50, &VtOptions::default()).expect("vt");
    let mut snapshot = GridSnapshot::default();
    let mut since_snapshot = 0;
    let start = Instant::now();
    for chunk in data.chunks(CHUNK) {
        vt.feed(chunk);
        let _ = vt.take_replies();
        since_snapshot += chunk.len();
        if since_snapshot >= SNAPSHOT_EVERY {
            since_snapshot = 0;
            let _ = vt.snapshot(&mut snapshot);
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    data.len() as f64 / (1024. * 1024.) / seconds
}

fn main() {
    let data = sample();
    let mut results: Vec<f64> = (0..RUNS).map(|_| run(&data)).collect();
    results.sort_by(f64::total_cmp);
    let median = results[RUNS / 2];
    let all: Vec<String> = results.iter().map(|r| format!("{r:.0}")).collect();
    println!("{:.0} MB/s median ({} MB, runs: {})", median, data.len() / (1024 * 1024), all.join(" "));
}
