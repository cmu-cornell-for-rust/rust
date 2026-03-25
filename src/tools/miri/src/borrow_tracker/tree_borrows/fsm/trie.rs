use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use super::transition::Transition;
use super::trace_file::{TRACE_FILE, TraceItem::Traces};

pub type TriePtr = Arc<Mutex<TrieNode>>;

pub struct TrieNode {
    pub event: Option<Transition>,
    pub children: HashMap<Transition, TriePtr>,
    pub count: usize,
}

pub struct Trie {
    pub root: TriePtr,
}

impl Trie {
    pub fn new() -> Self {
        let root = Arc::new(Mutex::new(TrieNode {
            event: None,
            children: HashMap::new(),
            count: 0,
        }));
        Self { root }
    }

    pub fn clone_root_and_inc(&self) -> TriePtr {
        let ptr = self.root.clone();
        let mut n = ptr.lock().unwrap();
        n.count += 1;
        drop(n);
        ptr
    }

    pub fn transition(&self, node: &TriePtr, ev: Transition) -> TriePtr {
        let mut n = node.lock().unwrap();
        if n.count > 0 {
            n.count -= 1;
        }
        let child = n.children.entry(ev).or_insert_with(|| {
            Arc::new(Mutex::new(TrieNode {
                event: Some(ev),
                children: HashMap::new(),
                count: 0,
            }))
        }).clone();
        {
            let mut child_lock = child.lock().unwrap();
            child_lock.count += 1;
        }
        child
    }

    pub fn flush_traces(&self, root_tag: u64, range: std::ops::Range<u64>) {
        let mut trace_map: HashMap<Vec<Transition>, usize> = HashMap::new();
        let root = self.root.lock().unwrap();
        for child in root.children.values() {
            self.dfs_collect(child, &mut Vec::new(), &mut trace_map);
        }
        drop(root);
        if !trace_map.is_empty() {
            let traces: Vec<(Vec<Transition>, usize)> = trace_map.into_iter().collect();
            TRACE_FILE.lock().unwrap().sender
                .send(Traces(root_tag, range, traces))
                .expect("failed to send traces");
        }
    }

    fn dfs_collect(
        &self,
        node: &TriePtr,
        trace: &mut Vec<Transition>,
        trace_map: &mut HashMap<Vec<Transition>, usize>,
    ) {
        let n = node.lock().unwrap();
        if let Some(ev) = n.event {
            trace.push(ev);
        }
        if n.count > 0 {
            *trace_map.entry(trace.clone()).or_insert(0) += n.count;
        }
        let children: Vec<TriePtr> = n.children.values().cloned().collect();
        drop(n);
        for child in children {
            self.dfs_collect(&child, trace, trace_map);
        }
        if !trace.is_empty() {
            trace.pop();
        }
    }
}
