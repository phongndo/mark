use std::{collections::HashMap, hash::Hash, sync::Arc};

use crate::LineTextFingerprint;

use super::{state::StateId, tokenizer::ScopedToken};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LineCacheKey {
    pub language: String,
    pub bundle_version: String,
    pub entry: StateId,
    pub first_line: bool,
    pub fingerprint: LineTextFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedLine {
    pub tokens: Arc<[ScopedToken]>,
    pub exit: StateId,
}

#[derive(Debug, Clone)]
pub struct LineCache<K, V> {
    entries: HashMap<K, LruEntry<V>>,
    capacity: usize,
    tick: u64,
}

#[derive(Debug, Clone)]
struct LruEntry<V> {
    value: V,
    last_used: u64,
}

impl<K, V> LineCache<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity,
            tick: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        if capacity == 0 {
            self.clear();
            return;
        }
        while self.entries.len() > self.capacity {
            let Some(oldest) = self.oldest_key() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn is_enabled(&self) -> bool {
        self.capacity > 0
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn get(&mut self, key: &K) -> Option<V> {
        let last_used = self.next_tick();
        let entry = self.entries.get_mut(key)?;
        entry.last_used = last_used;
        Some(entry.value.clone())
    }

    pub fn insert(&mut self, key: K, value: V) -> bool {
        if self.capacity == 0 {
            return false;
        }

        let last_used = self.next_tick();
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.value = value;
            entry.last_used = last_used;
            return false;
        }

        let mut evicted = false;
        if self.entries.len() >= self.capacity
            && let Some(oldest) = self.oldest_key()
        {
            self.entries.remove(&oldest);
            evicted = true;
        }

        self.entries.insert(key, LruEntry { value, last_used });
        evicted
    }

    fn next_tick(&mut self) -> u64 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }

    fn oldest_key(&self) -> Option<K> {
        self.entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())
    }
}
