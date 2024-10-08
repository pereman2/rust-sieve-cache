#![doc = include_str!("../README.md")]

use std::borrow::Borrow;
use std::hash::Hash;
use std::{collections::HashMap, ptr::NonNull};

struct Node<K: Eq + Hash + Clone, V> {
    key: K,
    value: V,
    prev: Option<NonNull<Node<K, V>>>,
    next: Option<NonNull<Node<K, V>>>,
    visited: bool,
}

impl<K: Eq + Hash + Clone, V> Node<K, V> {
    fn new(key: K, value: V) -> Self {
        Self {
            key,
            value,
            prev: None,
            next: None,
            visited: false,
        }
    }
}

type EvictDictator<K: Eq + Hash + Clone, V> = fn(&K, &V) -> bool;

/// A cache based on the SIEVE eviction algorithm.
pub struct SieveCache<K: Eq + Hash + Clone, V> {
    map: HashMap<K, Box<Node<K, V>>>,
    head: Option<NonNull<Node<K, V>>>,
    tail: Option<NonNull<Node<K, V>>>,
    hand: Option<NonNull<Node<K, V>>>,
    capacity: usize,
    len: usize,
    evict_condition: Option<EvictDictator<K, V>>,
}

unsafe impl<K: Eq + Hash + Clone, V> Send for SieveCache<K, V> {}

impl<K: Eq + Hash + Clone, V> SieveCache<K, V> {
    /// Create a new cache with the given capacity.
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if capacity == 0 {
            return Err("capacity must be greater than 0");
        }
        Ok(Self {
            map: HashMap::with_capacity(capacity),
            head: None,
            tail: None,
            hand: None,
            capacity,
            len: 0,
            evict_condition: None,
        })
    }

    pub fn with_evict_condition(
        capacity: usize,
        evict_dictator: EvictDictator<K, V>,
    ) -> Result<Self, &'static str> {
        if capacity == 0 {
            return Err("capacity must be greater than 0");
        }
        Ok(Self {
            map: HashMap::with_capacity(capacity),
            head: None,
            tail: None,
            hand: None,
            capacity,
            len: 0,
            evict_condition: Some(evict_dictator),
        })
    }

    /// Return the capacity of the cache.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Return the number of cached values.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Return `true` when no values are currently cached.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return `true` if there is a value in the cache mapped to by `key`.
    #[inline]
    pub fn contains_key<Q>(&mut self, key: &Q) -> bool
    where
        Q: Hash + Eq + ?Sized,
        K: Borrow<Q>,
    {
        self.map.contains_key(key)
    }

    /// Get an immutable reference to the value in the cache mapped to by `key`.
    ///
    /// If no value exists for `key`, this returns `None`.
    pub fn get<Q>(&mut self, key: &Q) -> Option<&V>
    where
        Q: Hash + Eq + ?Sized,
        K: Borrow<Q>,
    {
        let node_ = self.map.get_mut(key)?;
        node_.visited = true;
        Some(&node_.value)
    }

    /// Get a mutable reference to the value in the cache mapped to by `key`.
    ///
    /// If no value exists for `key`, this returns `None`.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        Q: Hash + Eq + ?Sized,
        K: Borrow<Q>,
    {
        let node_ = self.map.get_mut(key)?;
        node_.visited = true;
        Some(&mut node_.value)
    }

    /// Map `key` to `value` in the cache, possibly evicting old entries.
    ///
    /// This method returns `(true, true)` when this is a new entry, and `(false, true)` if an existing entry was
    /// updated. Last value of the pair is false if it failed to evict an entry to insert a new one.
    pub fn insert(&mut self, key: K, value: V) -> (bool, bool) {
        let node = self.map.get_mut(&key);
        if let Some(node_) = node {
            node_.visited = true;
            node_.value = value;
            return (false, true);
        }
        if self.len >= self.capacity {
            if !self.evict() {
                return (false, false);
            }
        }
        let node = Box::new(Node::new(key.clone(), value));
        self.add_node(NonNull::from(node.as_ref()));
        debug_assert!(!node.visited);
        self.map.insert(key, node);
        debug_assert!(self.len < self.capacity);
        self.len += 1;
        (true, true)
    }

    /// Remove the cache entry mapped to by `key`.
    ///
    /// This method returns the value removed from the cache. If `key` did not map to any value,
    /// then this returns `None`.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let node_ = self.map.get_mut(key)?;
        let node__ = NonNull::from(node_.as_ref());
        if self.hand == Some(node__) {
            self.hand = node_.as_ref().prev;
        }
        let value = self.map.remove(key).map(|node| node.value);
        self.remove_node(node__);
        debug_assert!(self.len > 0);
        self.len -= 1;
        value
    }

    fn add_node(&mut self, mut node: NonNull<Node<K, V>>) {
        unsafe {
            node.as_mut().next = self.head;
            node.as_mut().prev = None;
            if let Some(mut head) = self.head {
                head.as_mut().prev = Some(node);
            }
        }
        self.head = Some(node);
        if self.tail.is_none() {
            self.tail = self.head;
        }
    }

    fn remove_node(&mut self, node: NonNull<Node<K, V>>) {
        unsafe {
            if let Some(mut prev) = node.as_ref().prev {
                prev.as_mut().next = node.as_ref().next;
            } else {
                self.head = node.as_ref().next;
            }
            if let Some(mut next) = node.as_ref().next {
                next.as_mut().prev = node.as_ref().prev;
            } else {
                self.tail = node.as_ref().prev;
            }
        }
    }

    fn evict(&mut self) -> bool {
        let mut node = self.hand.or(self.tail);
        let len = self.len();
        let mut visited = 0;
        while node.is_some() {
            if visited >= len {
                // We cannot evict anything
                return false;
            }
            let mut node_ = node.unwrap();
            visited += 1;
            unsafe {
                let node_ref = node_.as_ref();
                if !node_ref.visited && self.evict_condition.is_none() {
                    break;
                }
                if !node_ref.visited
                    && self.evict_condition.is_some()
                        & self.evict_condition.unwrap()(&node_ref.key, &node_ref.value)
                {
                    break;
                }
                node_.as_mut().visited = false;
                if node_.as_ref().prev.is_some() {
                    node = node_.as_ref().prev;
                } else {
                    node = self.tail;
                }
            }
        }
        if let Some(node_) = node {
            unsafe {
                self.hand = node_.as_ref().prev;
                self.map.remove(&node_.as_ref().key);
            }
            self.remove_node(node_);
            debug_assert!(self.len > 0);
            self.len -= 1;
        }
        true
    }
}

#[test]
fn test() {
    let mut cache = SieveCache::new(3).unwrap();
    assert!(cache.insert("foo".to_string(), "foocontent".to_string()).0);
    assert!(cache.insert("bar".to_string(), "barcontent".to_string()).0);
    cache.remove("bar");
    assert!(
        cache
            .insert("bar2".to_string(), "bar2content".to_string())
            .0
    );
    assert!(
        cache
            .insert("bar3".to_string(), "bar3content".to_string())
            .0
    );
    assert_eq!(cache.get("foo"), Some(&"foocontent".to_string()));
    assert_eq!(cache.get("bar"), None);
    assert_eq!(cache.get("bar2"), Some(&"bar2content".to_string()));
    assert_eq!(cache.get("bar3"), Some(&"bar3content".to_string()));
}

#[test]
fn test_visited_flag_update() {
    let mut cache = SieveCache::new(2).unwrap();
    cache.insert("key1".to_string(), "value1".to_string());
    cache.insert("key2".to_string(), "value2".to_string());
    // update `key1` entry.
    cache.insert("key1".to_string(), "updated".to_string());
    // new entry is added.
    cache.insert("key3".to_string(), "value3".to_string());
    assert_eq!(cache.get("key1"), Some(&"updated".to_string()));
}

fn evict_string_cond(k: &String, v: &String) -> bool {
    dbg!(v);
    return v.len() < 6;
}

#[test]
fn test_with_eviction() {
    let mut cache = SieveCache::with_evict_condition(3, evict_string_cond).unwrap();
    assert!(cache.insert("a".to_string(), "aaaaaa".to_string()).0);
    assert!(cache.insert("b".to_string(), "bbbbbb".to_string()).0);
    assert!(cache.insert("c".to_string(), "cccccc".to_string()).0);
    assert!(cache.insert("bar".to_string(), "barc".to_string()).1 == false);
    assert_eq!(cache.get("a"), Some(&"aaaaaa".to_string()));
    assert_eq!(cache.get("b"), Some(&"bbbbbb".to_string()));
    assert_eq!(cache.get("c"), Some(&"cccccc".to_string()));
    assert_eq!(cache.get("bar"), None);
}

#[test]
fn test_with_eviction_2() {
    let mut cache = SieveCache::with_evict_condition(3, evict_string_cond).unwrap();
    assert!(cache.insert("a".to_string(), "aaaaaa".to_string()).0);
    assert!(cache.insert("b".to_string(), "bbbbbb".to_string()).0);
    assert!(cache.insert("c".to_string(), "c".to_string()).0);
    assert!(cache.insert("bar".to_string(), "barc".to_string()).1 == true);
    assert_eq!(cache.get("a"), Some(&"aaaaaa".to_string()));
    assert_eq!(cache.get("b"), Some(&"bbbbbb".to_string()));
    assert_eq!(cache.get("bar"), Some(&"barc".to_string()));
    assert_eq!(cache.get("c"), None);
}
