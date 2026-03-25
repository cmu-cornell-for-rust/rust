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

pub enum ChromeEvent {
    E1 { alloc: String, tag: u32 },
    E2 { child: u32, parent: u32 },
    E3 { tag: u32 },
    E4 { tag: u32 },
    E5 { tag: u32, visited: u32, skipped: u32 },
    E6,
    E7 { tag: u32, removed: u32 },
}

impl std::fmt::Display for ChromeEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChromeEvent::E1 { alloc, tag } => write!(f, "E1(a{}, t{})", alloc, tag),
            ChromeEvent::E2 { child, parent } => write!(f, "E2(t{}, t{})", child, parent),
            ChromeEvent::E3 { tag } => write!(f, "E3(t{})", tag),
            ChromeEvent::E4 { tag } => write!(f, "E4(t{})", tag),
            ChromeEvent::E5 { tag, visited, skipped } => write!(f, "E5(t{}, {}, {})", tag, visited, skipped),
            ChromeEvent::E6 => write!(f, "E6"),
            ChromeEvent::E7 { tag, removed } => write!(f, "E7(t{}, {})", tag, removed),
        }
    }
}

pub enum Message {
    Event(ChromeEvent),
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
                        write!(writer, "{} ", ev).unwrap();
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
                if field.name() == "message" {
                    self.message = Some(format!("{value:?}"));
                }
            }
        }

        let mut visitor = Visitor { message: None };
        event.record(&mut visitor);

        let msg = match visitor.message {
            Some(m) => m.trim_matches('"').to_string(),
            None => return,
        };

        fn extract_between_angles(s: &str) -> Vec<String> {
            let mut res = Vec::new();
            let mut start = 0;
            while let Some(l) = s[start..].find('<') {
                let lpos = start + l + 1;
                if let Some(r) = s[lpos..].find('>') {
                    res.push(s[lpos..lpos + r].to_string());
                    start = lpos + r + 1;
                } else { break; }
            }
            res
        }

        let ev = if msg.starts_with("New allocation ") {
            let angles = extract_between_angles(&msg);
            if angles.len() >= 1 {
                let raw_alloc = msg.strip_prefix("New allocation ")
                    .unwrap().split(" has rpot tag").next().unwrap();
                let alloc_part = raw_alloc.strip_prefix("alloc").unwrap_or(raw_alloc);
                ChromeEvent::E1 { alloc: alloc_part.to_string(), tag: angles[0].parse().unwrap() }
            } else { return; }
        } else if msg.starts_with("reborrow: reference ") {
            let angles = extract_between_angles(&msg);
            if angles.len() >= 2 {
                ChromeEvent::E2 { child: angles[0].parse().unwrap(), parent: angles[1].parse().unwrap() }
            } else { return; }
        } else if msg.contains(" access with tag <") {
            let angles = extract_between_angles(&msg);
            if angles.len() >= 1 {
                if msg.starts_with("read") { ChromeEvent::E3 { tag: angles[0].parse().unwrap() } }
                else { ChromeEvent::E4 { tag: angles[0].parse().unwrap() } }
            } else { return; }
        } else if msg.contains(" access ") && msg.contains(" visited ") && msg.contains(" skipped ") {
            let angles = extract_between_angles(&msg);
            if angles.len() >= 1 {
                let mut visited = 0;
                let mut skipped = 0;
                let mut it = msg.split_whitespace();
                while let Some(word) = it.next() {
                    if word == "visited" {
                        visited = it.next().unwrap_or("0").parse().unwrap();
                    } else if word == "skipped" {
                        skipped = it.next().unwrap_or("0").parse().unwrap();
                        break;
                    }
                }
                ChromeEvent::E5 { tag: angles[0].parse().unwrap(), visited, skipped }
            } else { return; }
        } else if msg == "Provenance GC invoked" { ChromeEvent::E6 }
        else if msg.starts_with("Removed ") && msg.contains(" from root tag <") {
            let angles = extract_between_angles(&msg);
            if angles.len() >= 1 {
                let mut it = msg.split_whitespace();
                let _ = it.next();
                let removed = it.next().unwrap_or("0").parse().unwrap();
                ChromeEvent::E7 { tag: angles[0].parse().unwrap(), removed }
            } else { return; }
        } else { return; };

        let sender = self.out.lock().unwrap().clone();
        let _ = sender.send(Message::Event(ev));
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
    let ts = *TIMESTAMP;
    Box::new(
        std::fs::File::create(format!("./events-{}", ts))
            .expect("Failed to create trace file."),
    )
}
