use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    time::{Duration, Instant},
};

use simulator::SimulateCtx;
use sui_types::{base_types::ObjectID, digests::TransactionDigest};

use crate::types::Source;

pub struct ArbItem {
    pub coin: String,
    pub pool_id: Option<ObjectID>,
    pub tx_digest: TransactionDigest,
    pub sim_ctx: SimulateCtx,
    pub source: Source,
}

impl ArbItem {
    pub fn new(coin: String, pool_id: Option<ObjectID>, entry: ArbEntry) -> Self {
        Self {
            coin: coin.to_string(),
            pool_id,
            tx_digest: entry.digest,
            sim_ctx: entry.sim_ctx,
            source: entry.source,
        }
    }
}

/// The value stored in the HashMap for each coin.
pub struct ArbEntry {
    digest: TransactionDigest,
    sim_ctx: SimulateCtx,
    generation: u64,
    expires_at: Instant,
    source: Source,
}

#[derive(Eq, PartialEq)]
struct HeapItem {
    expires_at: Instant,
    generation: u64,
    coin: String,
    pool_id: Option<ObjectID>,
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        // Default BinaryHeap is a max-heap, so we invert ordering:
        // We want the earliest expiration at the front, so we compare timestamps reversed.
        self.expires_at
            .cmp(&other.expires_at)
            .then(self.generation.cmp(&other.generation))
            .reverse()
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A structure to manage ArbItems with uniqueness, reordering, and timed expiration.
pub struct ArbCache {
    map: HashMap<String, ArbEntry>,
    heap: BinaryHeap<HeapItem>,
    generation_counter: u64,
    expiration_duration: Duration,
}

impl ArbCache {
    pub fn new(expiration_duration: Duration) -> Self {
        Self {
            map: HashMap::new(),
            heap: BinaryHeap::new(),
            generation_counter: 0,
            expiration_duration,
        }
    }

    /// Insert or update an ArbItem.
    /// If the coin already exists, this updates it with a new generation and expiration time.
    pub fn insert(
        &mut self,
        coin: String,
        pool_id: Option<ObjectID>,
        digest: TransactionDigest,
        sim_ctx: SimulateCtx,
        source: Source,
    ) {
        let now = Instant::now();
        self.generation_counter += 1;
        let generation = self.generation_counter;
        let expires_at = now + self.expiration_duration;

        // Insert into the map
        self.map.insert(
            coin.clone(),
            ArbEntry {
                digest,
                sim_ctx,
                generation,
                expires_at,
                source,
            },
        );

        // Insert into the heap
        self.heap.push(HeapItem {
            expires_at,
            generation,
            coin,
            pool_id,
        });
    }

    /// Attempt to get an ArbItem by coin.
    #[allow(dead_code)]
    pub fn get(&self, coin: &str) -> Option<(TransactionDigest, SimulateCtx)> {
        self.map.get(coin).map(|entry| (entry.digest, entry.sim_ctx.clone()))
    }

    /// Periodically call this to remove expired entries.
    /// This will pop from the heap until it finds an entry that is not stale and not expired.
    pub fn remove_expired(&mut self) -> Vec<String> {
        let mut expired_coins = Vec::new();
        let now = Instant::now();
        while let Some(top) = self.heap.peek() {
            // If top is outdated (stale) or expired, pop it and remove from map if needed
            if let Some(entry) = self.map.get(&top.coin) {
                if entry.generation != top.generation {
                    // Stale entry, just discard from heap
                    self.heap.pop();
                    continue;
                }
                // Matching generation
                if entry.expires_at <= now {
                    // It's actually expired
                    expired_coins.push(top.coin.clone());
                    self.map.remove(&top.coin);
                    self.heap.pop();
                } else {
                    // The top is not expired and not stale. We can break now.
                    break;
                }
            } else {
                // Coin not in map means stale in heap
                self.heap.pop();
            }
        }
        expired_coins
    }

    pub fn pop_one(&mut self) -> Option<ArbItem> {
        let now = Instant::now();
        // Keep popping until we find a valid, current entry that's not expired.
        while let Some(top) = self.heap.pop() {
            if let Some(entry) = self.map.get(&top.coin) {
                if entry.generation == top.generation {
                    // It's the current entry for this coin
                    if entry.expires_at > now {
                        // It's valid and not expired. We can remove it and return.
                        let entry = self.map.remove(&top.coin).unwrap();
                        return Some(ArbItem::new(top.coin, top.pool_id, entry));
                    } else {
                        // It's current but expired, remove it from map and continue.
                        self.map.remove(&top.coin);
                    }
                } else {
                    // Stale entry, just continue without touching the map.
                    // Because a newer entry for this coin exists.
                }
            } else {
                // The map no longer has this coin, meaning it's stale.
                continue;
            }
        }
        // No valid entries were found
        None
    }
}
