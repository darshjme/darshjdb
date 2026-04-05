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

    /// Rate limiter state: window start and count under a single lock
    /// to prevent split-brain between window reset and count.
    rate_state: std::sync::Mutex<(Instant, u32)>,

    /// TTL for presence entries.
    ttl: Duration,
}

impl PresenceRoom {
    /// Create a new presence room with default TTL.
    pub fn new(room_id: String) -> Self {
        Self {
            room_id,
            entries: DashMap::new(),
            rate_state: std::sync::Mutex::new((Instant::now(), 0)),
            ttl: DEFAULT_TTL,
        }
    }

    /// Create a presence room with a custom TTL.
    pub fn with_ttl(room_id: String, ttl: Duration) -> Self {
        Self {
            room_id,
            entries: DashMap::new(),
            rate_state: std::sync::Mutex::new((Instant::now(), 0)),
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
    ///
    /// Both the window start and the count are under a single mutex to prevent
    /// races where one thread resets the window while another increments
    /// a stale count from the previous window.
    fn check_rate_limit(&self) -> bool {
        let now = Instant::now();

        let mut state = match self.rate_state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        let (ref mut window_start, ref mut count) = *state;

        if now.duration_since(*window_start) >= Duration::from_secs(1) {
            // Reset window.
            *window_start = now;
            *count = 1;
            return true;
        }

        if *count >= MAX_UPDATES_PER_SEC {
            return false;
        }

        *count += 1;
        true
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- PresenceRoom tests ---

    #[test]
    fn room_update_and_snapshot() {
        let room = PresenceRoom::new("room-1".into());
        room.update("alice", json!({"cursor": 10}));
        room.update("bob", json!({"cursor": 20}));

        let snap = room.snapshot();
        assert_eq!(snap.len(), 2);
        let alice_entry = snap.iter().find(|(uid, _)| uid == "alice");
        assert!(alice_entry.is_some());
        assert_eq!(alice_entry.unwrap().1, json!({"cursor": 10}));
    }

    #[test]
    fn room_remove_user() {
        let room = PresenceRoom::new("room-1".into());
        room.update("alice", json!({}));
        room.update("bob", json!({}));

        let removed = room.remove("alice");
        assert!(removed.is_some());
        assert_eq!(room.active_count(), 1);
        assert!(!room.is_present("alice"));
        assert!(room.is_present("bob"));
    }

    #[test]
    fn room_remove_nonexistent() {
        let room = PresenceRoom::new("room-1".into());
        assert!(room.remove("ghost").is_none());
    }

    #[test]
    fn room_expiry() {
        let room = PresenceRoom::with_ttl("room-1".into(), Duration::from_millis(1));
        room.update("alice", json!({}));

        // Wait for expiry.
        std::thread::sleep(Duration::from_millis(10));

        assert!(!room.is_present("alice"));
        assert_eq!(room.active_count(), 0);

        let expired = room.expire_stale();
        assert_eq!(expired, vec!["alice"]);

        // After expiry, the entry is gone.
        let snap = room.snapshot();
        assert!(snap.is_empty());
    }

    #[test]
    fn room_expiry_mixed() {
        let room = PresenceRoom::with_ttl("room-1".into(), Duration::from_millis(50));
        room.update("alice", json!({}));

        std::thread::sleep(Duration::from_millis(60));

        // Alice expired, now add Bob.
        room.update("bob", json!({}));

        assert!(!room.is_present("alice"));
        assert!(room.is_present("bob"));

        let expired = room.expire_stale();
        assert_eq!(expired, vec!["alice"]);
        assert_eq!(room.active_count(), 1);
    }

    #[test]
    fn room_rate_limiting() {
        let room = PresenceRoom::new("room-1".into());

        // The first MAX_UPDATES_PER_SEC updates should succeed.
        for i in 0..MAX_UPDATES_PER_SEC {
            assert!(
                room.update("user", json!({"i": i})),
                "update {i} should succeed"
            );
        }

        // The next update should be rate-limited.
        assert!(
            !room.update("user", json!({"i": "over-limit"})),
            "update over limit should be rejected"
        );
    }

    #[test]
    fn room_rate_limit_resets_after_window() {
        let room = PresenceRoom::new("room-1".into());

        for _ in 0..MAX_UPDATES_PER_SEC {
            room.update("user", json!({}));
        }
        assert!(!room.update("user", json!({})));

        // Wait for the rate window to reset.
        std::thread::sleep(Duration::from_secs(1));

        assert!(room.update("user", json!({"after_reset": true})));
    }

    #[test]
    fn room_update_overwrites_state() {
        let room = PresenceRoom::new("room-1".into());
        room.update("alice", json!({"v": 1}));
        room.update("alice", json!({"v": 2}));

        let snap = room.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].1, json!({"v": 2}));
    }

    // --- PresenceManager tests ---

    #[test]
    fn manager_join_creates_room() {
        let mgr = PresenceManager::new();
        assert_eq!(mgr.room_count(), 0);

        mgr.join("room-1", "alice", json!({}));
        assert_eq!(mgr.room_count(), 1);
    }

    #[test]
    fn manager_join_and_snapshot() {
        let mgr = PresenceManager::new();
        mgr.join("room-1", "alice", json!({"status": "online"}));
        mgr.join("room-1", "bob", json!({"status": "away"}));

        let snap = mgr.room_snapshot("room-1");
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn manager_leave_removes_user() {
        let mgr = PresenceManager::new();
        mgr.join("room-1", "alice", json!({}));
        mgr.join("room-1", "bob", json!({}));

        mgr.leave("room-1", "alice");
        let snap = mgr.room_snapshot("room-1");
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, "bob");
    }

    #[test]
    fn manager_leave_last_user_cleans_room() {
        let mgr = PresenceManager::new();
        mgr.join("room-1", "alice", json!({}));

        mgr.leave("room-1", "alice");
        assert_eq!(mgr.room_count(), 0);
    }

    #[test]
    fn manager_leave_all() {
        let mgr = PresenceManager::new();
        mgr.join("room-1", "alice", json!({}));
        mgr.join("room-2", "alice", json!({}));
        mgr.join("room-2", "bob", json!({}));

        mgr.leave_all("alice");

        // room-1 should be cleaned up (was only alice).
        assert_eq!(mgr.room_snapshot("room-1").len(), 0);
        // room-2 still has bob.
        assert_eq!(mgr.room_snapshot("room-2").len(), 1);
    }

    #[test]
    fn manager_update_state_existing_room() {
        let mgr = PresenceManager::new();
        mgr.join("room-1", "alice", json!({"v": 1}));

        mgr.update_state("room-1", "alice", json!({"v": 2}));
        let snap = mgr.room_snapshot("room-1");
        assert_eq!(snap[0].1, json!({"v": 2}));
    }

    #[test]
    fn manager_update_state_auto_joins() {
        let mgr = PresenceManager::new();
        // update_state on nonexistent room auto-joins.
        mgr.update_state("room-1", "alice", json!({"auto": true}));
        assert_eq!(mgr.room_count(), 1);
        let snap = mgr.room_snapshot("room-1");
        assert_eq!(snap.len(), 1);
    }

    #[test]
    fn manager_expire_all() {
        let mgr = PresenceManager::new();

        // Use a room that we can manipulate through the manager.
        // Join with default TTL (60s), so nothing expires.
        mgr.join("room-1", "alice", json!({}));
        let expired = mgr.expire_all();
        assert_eq!(expired, 0);
        assert_eq!(mgr.room_count(), 1);
    }

    #[test]
    fn manager_snapshot_nonexistent_room() {
        let mgr = PresenceManager::new();
        let snap = mgr.room_snapshot("no-such-room");
        assert!(snap.is_empty());
    }

    #[test]
    fn manager_leave_nonexistent_room() {
        let mgr = PresenceManager::new();
        // Should not panic.
        mgr.leave("no-such-room", "alice");
    }

    #[test]
    fn manager_leave_nonexistent_user() {
        let mgr = PresenceManager::new();
        mgr.join("room-1", "alice", json!({}));
        // Leaving a user not in the room should not panic.
        mgr.leave("room-1", "ghost");
        assert_eq!(mgr.room_snapshot("room-1").len(), 1);
    }
}
