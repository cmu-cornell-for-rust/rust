#![allow(warnings)]
#![cfg(feature = "tracing")]

use rustc_log::tracing_core::{Event, Subscriber};
use rustc_log::tracing_core::field::{Field, Visit};
use rustc_log::tracing_subscriber::{
    layer::Context,
    registry::LookupSpan,
    Layer,
};
use std::{
    marker::PhantomData,
    sync::{Arc, Mutex},
};
use std::io::{BufWriter, Write};
use std::sync::mpsc::{self, Sender};
use std::{
    cell::Cell,
    thread::JoinHandle,
};
use miri::borrow_tracker::tree_borrows::fsm::trace_file::TIMESTAMP;

/// Keep for documentation

// pub enum ChromeEvent {
//     E1 { alloc: String, tag: u32 },
//     E1a { alloc: String, kind: String, timestamp: u128 },
//     E2 { child: u32, parent: u32, size: u64, timestamp: u128 },
//     E3 { tag: u32, timestamp: u128 },
//     E4 { tag: u32, timestamp: u128 },
//     E5 { tag: u32, visited: u32, skipped: u32, timestamp: u128 },
//     E6,
//     E7 { tag: u32, removed: u32, timestamp: u128 },
// }

// impl std::fmt::Display for ChromeEvent {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         match self {
//             ChromeEvent::E1 { alloc, tag } => write!(f, "E1(a{}, t{})", alloc, tag),
//             ChromeEvent::E1a { alloc, kind, timestamp } => write!(f, "E1a(a{}, k{}, n{})", alloc, kind, timestamp),
//             ChromeEvent::E2 { child, parent, size, timestamp } => write!(f, "E2(t{}, t{}, s{}, n{})", child, parent, size, timestamp),
//             ChromeEvent::E3 { tag, timestamp } => write!(f, "E3(t{}, n{})", tag, timestamp),
//             ChromeEvent::E4 { tag, timestamp } => write!(f, "E4(t{}, n{})", tag, timestamp),
//             ChromeEvent::E5 { tag, visited, skipped, timestamp } => write!(f, "E5(t{}, {}, {}, n{})", tag, visited, skipped, timestamp),
//             ChromeEvent::E6 => write!(f, "E6"),
//             ChromeEvent::E7 { tag, removed, timestamp } => write!(f, "E7(t{}, {}, n{})", tag, removed, timestamp),
//         }
//     }
// }

pub enum Message {
    Event(String),
    Flush,
    Drop,
}

pub struct ChromeLayer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span> + Send + Sync,
{
    out: Arc<Mutex<Sender<Message>>>,
    _inner: PhantomData<S>,
}

pub struct FlushGuard {
    sender: Sender<Message>,
    handle: Cell<Option<JoinHandle<()>>>,
}

pub struct ChromeLayerBuilder<S>
where
    S: Subscriber + for<'span> LookupSpan<'span> + Send + Sync,
{
    out_writer: Option<Box<dyn Write + Send>>,
    _inner: PhantomData<S>,
}

impl<S> ChromeLayerBuilder<S>
where
    S: Subscriber + for<'span> LookupSpan<'span> + Send + Sync,
{
    pub fn new() -> Self {
        Self { out_writer: None, _inner: PhantomData }
    }

    pub fn writer<W: Write + Send + 'static>(mut self, writer: W) -> Self {
        self.out_writer = Some(Box::new(writer));
        self
    }

    pub fn build(self) -> (ChromeLayer<S>, FlushGuard) {
        ChromeLayer::new(self)
    }
}

impl<S> ChromeLayer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span> + Send + Sync,
{
    fn new(builder: ChromeLayerBuilder<S>) -> (Self, FlushGuard) {
        let (tx, rx) = mpsc::channel();
        let out_writer = builder.out_writer.unwrap_or_else(|| create_default_writer());

        let handle = std::thread::spawn(move || {
            let mut writer = BufWriter::new(out_writer);
            for msg in rx {
                match msg {
                    Message::Event(ev) => {
                        writeln!(writer, "{}", ev).unwrap();
                    }
                    Message::Flush => writer.flush().unwrap(),
                    Message::Drop => break,
                }
            }
            writer.flush().unwrap();
        });

        let guard = FlushGuard { sender: tx.clone(), handle: Cell::new(Some(handle)) };
        let layer = ChromeLayer { out: Arc::new(Mutex::new(tx)), _inner: PhantomData };
        (layer, guard)
    }
}

impl<S> Layer<S> for ChromeLayer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span> + Send + Sync,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        struct Visitor { message: Option<String> }

        impl Visit for Visitor {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                self.message = Some(format!("{value:?}"));
            }
        }

        let mut visitor = Visitor { message: None };
        event.record(&mut visitor);

        if let Some(msg) = visitor.message {
            let sender = self.out.lock().unwrap().clone();
            let _ = sender.send(Message::Event(msg));
        }
    }
}

impl FlushGuard {
    pub fn flush(&self) {
        if let Some(handle) = self.handle.take() {
            let _ = self.sender.send(Message::Flush);
            self.handle.set(Some(handle));
        }
    }
}

impl Drop for FlushGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = self.sender.send(Message::Drop);
            let _ = handle.join();
        }
    }
}

fn create_default_writer() -> Box<dyn Write + Send> {
    let filename = std::env::var("MIRI_TEST_NAME")
        .ok()
        .unwrap_or_else(|| (*TIMESTAMP).to_string());
    Box::new(
        std::fs::File::create(format!("./events-{}", filename))
            .expect("Failed to create trace file."),
    )
}
