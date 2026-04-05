//! Presence system for ephemeral per-room user state.
//!
//! Tracks which users are "present" in a room (e.g., viewing a document,
//! in a channel) along with arbitrary state (cursor position, typing status).
//! Automatically expires stale entries and rate-limits updates to prevent
//! flooding.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

/// Default presence TTL: entries expire if not refreshed within this window.
const DEFAULT_TTL: Duration = Duration::from_secs(60);

/// Maximum presence updates per room per second.
const MAX_UPDATES_PER_SEC: u32 = 20;

/// A user's presence entry within a room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceEntry {
    /// Arbitrary state payload (cursor position, status, etc.).
    pub state: Value,

    /// When this entry was last updated (not serialized).
    #[serde(skip, default = "Instant::now")]
    pub last_seen: Instant,
}

/// A single presence room containing user entries.
#[derive(Debug)]
pub struct PresenceRoom {
    /// Room identifier.
    pub room_id: String,

    /// Map of user_id to their presence entry.
    entries: DashMap<String, PresenceEntry>,

    /// Rate limiter state: tracks the last update timestamp and count.
    rate_window_start: std::sync::Mutex<Instant>,
    rate_count: std::sync::atomic::AtomicU32,

    /// TTL for presence entries.
    ttl: Duration,
}

impl PresenceRoom {
    /// Create a new presence room with default TTL.
    pub fn new(room_id: String) -> Self {
        Self {
            room_id,
            entries: DashMap::new(),
            rate_window_start: std::sync::Mutex::new(Instant::now()),
            rate_count: std::sync::atomic::AtomicU32::new(0),
            ttl: DEFAULT_TTL,
        }
    }

    /// Create a presence room with a custom TTL.
    pub fn with_ttl(room_id: String, ttl: Duration) -> Self {
        Self {
            room_id,
            entries: DashMap::new(),
            rate_window_start: std::sync::Mutex::new(Instant::now()),
            rate_count: std::sync::atomic::AtomicU32::new(0),
            ttl,
        }
    }

    /// Update a user's presence state. Returns `false` if rate-limited.
    pub fn update(&self, user_id: &str, state: Value) -> bool {
        if !self.check_rate_limit() {
            warn!(
                room_id = %self.room_id,
                user_id = %user_id,
                "presence update rate-limited"
            );
            return false;
        }

        self.entries.insert(
            user_id.to_string(),
            PresenceEntry {
                state,
                last_seen: Instant::now(),
            },
        );
        true
    }

    /// Remove a user from this room.
    pub fn remove(&self, user_id: &str) -> Option<PresenceEntry> {
        self.entries.remove(user_id).map(|(_, e)| e)
    }

    /// Get a snapshot of all non-expired entries.
    pub fn snapshot(&self) -> Vec<(String, Value)> {
        let now = Instant::now();
        self.entries
            .iter()
            .filter(|entry| now.duration_since(entry.value().last_seen) < self.ttl)
            .map(|entry| (entry.key().clone(), entry.value().state.clone()))
            .collect()
    }

    /// Expire stale entries. Returns the user IDs that were removed.
    pub fn expire_stale(&self) -> Vec<String> {
        let now = Instant::now();
        let mut expired = Vec::new();

        self.entries.retain(|user_id, entry| {
            if now.duration_since(entry.last_seen) >= self.ttl {
                expired.push(user_id.clone());
                false
            } else {
                true
            }
        });

        expired
    }

    /// Number of non-expired users currently in the room.
    pub fn active_count(&self) -> usize {
        let now = Instant::now();
        self.entries
            .iter()
            .filter(|entry| now.duration_since(entry.value().last_seen) < self.ttl)
            .count()
    }

    /// Check if a specific user is present and not expired.
    pub fn is_present(&self, user_id: &str) -> bool {
        self.entries
            .get(user_id)
            .map(|entry| Instant::now().duration_since(entry.last_seen) < self.ttl)
            .unwrap_or(false)
    }

    /// Sliding-window rate limiter: max `MAX_UPDATES_PER_SEC` per second.
    fn check_rate_limit(&self) -> bool {
        let now = Instant::now();

        let mut window_start = match self.rate_window_start.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if now.duration_since(*window_start) >= Duration::from_secs(1) {
            // Reset window.
            *window_start = now;
            self.rate_count
                .store(1, std::sync::atomic::Ordering::Relaxed);
            return true;
        }

        let count = self
            .rate_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        count < MAX_UPDATES_PER_SEC
    }
}

/// Global presence manager holding all rooms.
///
/// Rooms are created lazily on first join and cleaned up when empty.
#[derive(Debug)]
pub struct PresenceManager {
    rooms: DashMap<String, Arc<PresenceRoom>>,
}

impl PresenceManager {
    /// Create a new presence manager.
    pub fn new() -> Self {
        Self {
            rooms: DashMap::new(),
        }
    }

    /// Join a room (creating it if necessary) and set initial state.
    /// Returns `false` if the update was rate-limited.
    pub fn join(&self, room_id: &str, user_id: &str, state: Value) -> bool {
        let room = self
            .rooms
            .entry(room_id.to_string())
            .or_insert_with(|| Arc::new(PresenceRoom::new(room_id.to_string())))
            .clone();

        room.update(user_id, state)
    }

    /// Update presence state in a room. Returns `false` if rate-limited
    /// or the room does not exist.
    pub fn update_state(&self, room_id: &str, user_id: &str, state: Value) -> bool {
        match self.rooms.get(room_id) {
            Some(room) => room.update(user_id, state),
            None => {
                debug!(
                    room_id = %room_id,
                    user_id = %user_id,
                    "presence update for non-existent room, auto-joining"
                );
                self.join(room_id, user_id, state)
            }
        }
    }

    /// Leave a room. If the room becomes empty, it is removed.
    pub fn leave(&self, room_id: &str, user_id: &str) {
        if let Some(room) = self.rooms.get(room_id) {
            room.remove(user_id);
            if room.active_count() == 0 {
                drop(room);
                // Re-check under remove lock to avoid races.
                self.rooms
                    .remove_if(room_id, |_, room| room.active_count() == 0);
            }
        }
    }

    /// Remove a user from all rooms (on disconnect).
    pub fn leave_all(&self, user_id: &str) {
        let room_ids: Vec<String> = self.rooms.iter().map(|r| r.key().clone()).collect();
        for room_id in room_ids {
            self.leave(&room_id, user_id);
        }
    }

    /// Get a snapshot of all users and their state in a room.
    pub fn room_snapshot(&self, room_id: &str) -> Vec<(String, Value)> {
        self.rooms
            .get(room_id)
            .map(|room| room.snapshot())
            .unwrap_or_default()
    }

    /// Run expiration across all rooms. Returns total number of expired entries.
    /// Should be called periodically (e.g., every 10 seconds).
    pub fn expire_all(&self) -> usize {
        let mut total = 0;
        let mut empty_rooms = Vec::new();

        for entry in self.rooms.iter() {
            let expired = entry.value().expire_stale();
            total += expired.len();
            if entry.value().active_count() == 0 {
                empty_rooms.push(entry.key().clone());
            }
        }

        // Clean up empty rooms.
        for room_id in empty_rooms {
            self.rooms
                .remove_if(&room_id, |_, room| room.active_count() == 0);
        }

        total
    }

    /// Total number of active rooms.
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }
}

impl Default for PresenceManager {
    fn default() -> Self {
        Self::new()
    }
}
