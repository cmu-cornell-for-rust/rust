use once_cell::sync::Lazy;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, Sender},
    Mutex,
};
use std::thread;
use super::transition::Transition;

pub static TIMESTAMP: Lazy<u128> = Lazy::new(|| {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros()
});

pub static NOOP_TRANSITIONS: AtomicU64 = AtomicU64::new(0);
pub static EMPTY_FSM: AtomicU64 = AtomicU64::new(0);

pub enum TraceItem {
    Traces(u64, std::ops::Range<u64>, Vec<(Vec<Transition>, usize)>),
    Stats(u64, u64),
}

pub struct TraceFile {
    pub sender: Sender<TraceItem>,
}

pub static TRACE_FILE: Lazy<Mutex<TraceFile>> = Lazy::new(|| {
    let (tx, rx): (Sender<TraceItem>, Receiver<TraceItem>) = mpsc::channel();
    let filename = std::env::var("MIRI_TEST_NAME")
        .ok()
        .unwrap_or_else(|| (*TIMESTAMP).to_string());
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("traces-{}", filename))
        .expect("failed to open trace file");

    let mut writer = BufWriter::new(file);
    thread::spawn(move || {
        while let Ok(item) = rx.recv() {
            match item {
                TraceItem::Traces(root_tag, range, traces) => {
                    write!(writer, "t{}@{{{}..{}}}", root_tag, range.start, range.end).unwrap();
                    for (trace, count) in traces {
                        write!(writer, " {:?} {}", trace, count).unwrap();
                    }
                    writeln!(writer).unwrap();
                    writer.flush().unwrap();
                }
                TraceItem::Stats(noop, empty) => {
                    writeln!(writer, "__STATS__ noop_transitions={}, empty_fsm={}", noop, empty).unwrap();
                    writer.flush().unwrap();
                }
            }
        }
    });
    Mutex::new(TraceFile { sender: tx })
});

pub fn flush_global_stats() {
    let noop = NOOP_TRANSITIONS.swap(0, Ordering::Relaxed);
    let empty = EMPTY_FSM.swap(0, Ordering::Relaxed);
    TRACE_FILE.lock().unwrap()
        .sender
        .send(TraceItem::Stats(noop, empty))
        .expect("failed to send stats");
}
