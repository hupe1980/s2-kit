//! Many sessions, one loop.
//!
//! A Customer Energy Manager talks to every flexible device in a building, and a fleet
//! manager to every device in a thousand. Each of those sessions has its own
//! acknowledgement deadlines, its own timers and its own validity windows, and the
//! obvious implementation — one task per session — turns a hundred devices into a hundred
//! stacks and a hundred timers.
//!
//! [`SessionSet`] keeps them in one map and answers the only question a single-threaded
//! event loop actually needs: **when must I next wake up, and for whom?**
//!
//! ```
//! use s2_kit::prelude::*;
//! use s2_kit::session::SessionSet;
//!
//! let mut fleet: SessionSet<String, CemSession> = SessionSet::new();
//! fleet.insert("battery".to_string(), CemSession::new(CemConfig::default()));
//! fleet.insert("heatpump".to_string(), CemSession::new(CemConfig::default()));
//!
//! let now = Timestamp::UNIX_EPOCH;
//! fleet.open_all(now);
//!
//! // One deadline for the whole fleet — the soonest of all of them.
//! if let Some(deadline) = fleet.poll_timeout() {
//!     // sleep until `deadline`, then:
//!     fleet.handle_timeout(deadline);
//! }
//!
//! // And one drain, tagged with whose session each frame belongs to.
//! for (device, out) in fleet.drain_transmit() {
//!     assert!(!out.text.is_empty());
//!     let _ = device;
//! }
//! ```

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::types::Timestamp;

use super::{Outgoing, Session};

/// A keyed collection of sessions driven as one.
///
/// `K` is whatever identifies a peer to the application: a node id, a serial number, a
/// database row id. The set never interprets it.
#[derive(Debug, Default)]
pub struct SessionSet<K, S> {
    sessions: BTreeMap<K, S>,
}

impl<K: Ord, S: Session> SessionSet<K, S> {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
        }
    }

    /// Add a session, returning whatever was under that key.
    pub fn insert(&mut self, key: K, session: S) -> Option<S> {
        self.sessions.insert(key, session)
    }

    /// Take a session out — when its transport has gone, for instance.
    pub fn remove(&mut self, key: &K) -> Option<S> {
        self.sessions.remove(key)
    }

    /// One session.
    pub fn get(&self, key: &K) -> Option<&S> {
        self.sessions.get(key)
    }

    /// One session, mutably, to feed a frame into.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut S> {
        self.sessions.get_mut(key)
    }

    /// How many sessions there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Every session, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &S)> {
        self.sessions.iter()
    }

    /// Every session, mutably, in key order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut S)> {
        self.sessions.iter_mut()
    }

    /// Open every session that has not been opened.
    pub fn open_all(&mut self, now: Timestamp) {
        for session in self.sessions.values_mut() {
            session.open(now);
        }
    }

    /// The soonest deadline across the whole set.
    ///
    /// This is the number an event loop sleeps on. A set of a thousand sessions still
    /// has exactly one timer.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<Timestamp> {
        self.sessions
            .values()
            .filter_map(Session::poll_timeout)
            .min()
    }

    /// Which session owns the soonest deadline, for a caller that would rather wake one
    /// session than sweep them all.
    #[must_use]
    pub fn next_deadline(&self) -> Option<(&K, Timestamp)> {
        self.sessions
            .iter()
            .filter_map(|(key, session)| session.poll_timeout().map(|at| (key, at)))
            .min_by_key(|(_, at)| *at)
    }

    /// Fire every deadline that has come due.
    ///
    /// Sweeping the whole set rather than only the session that woke us is deliberate:
    /// several deadlines commonly fall in the same millisecond, and a loop that handles
    /// one per wake-up turns a burst into a long tail of timer round-trips.
    pub fn handle_timeout(&mut self, now: Timestamp) {
        for session in self.sessions.values_mut() {
            if session.poll_timeout().is_some_and(|at| at <= now) {
                session.handle_timeout(now);
            }
        }
    }

    /// Everything every session wants to send, tagged with whose it is.
    ///
    /// The key is **cloned**, not borrowed. Draining and then acting on what was drained
    /// is the only thing a caller does with the result — write the frame to the socket it
    /// looks up by that key, close the session that just emitted it, remove one whose
    /// transport has gone — and a borrowed key would hold the set immutably for exactly as
    /// long as the result lives.
    pub fn drain_transmit(&mut self) -> Vec<(K, Outgoing)>
    where
        K: Clone,
    {
        let mut out = Vec::new();
        for (key, session) in &mut self.sessions {
            while let Some(frame) = session.poll_transmit() {
                out.push((key.clone(), frame));
            }
        }
        out
    }

    /// Everything every session wants to tell the application, tagged with whose it is.
    ///
    /// Cloned for the same reason [`drain_transmit`](Self::drain_transmit) clones: acting
    /// on an event usually means touching the set the event came from.
    pub fn drain_events(&mut self) -> Vec<(K, S::Event)>
    where
        K: Clone,
    {
        let mut out = Vec::new();
        for (key, session) in &mut self.sessions {
            while let Some(event) = session.poll_event() {
                out.push((key.clone(), event));
            }
        }
        out
    }

    /// Drop every session whose state machine has finished.
    ///
    /// A set that is never pruned is a memory leak with extra steps: a reconnecting
    /// device produces a new session each time, and the closed ones have nothing left to
    /// say.
    pub fn retain_open(&mut self) -> usize {
        let before = self.sessions.len();
        self.sessions
            .retain(|_, session| !session.state().is_closed());
        before - self.sessions.len()
    }

    /// Close every session.
    pub fn close_all(&mut self) {
        for session in self.sessions.values_mut() {
            session.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{CemConfig, CemSession, RmConfig, RmSession};
    use alloc::string::{String, ToString};
    use alloc::vec;

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_unix(1_700_000_000 + secs, 0)
    }

    fn fleet() -> SessionSet<String, CemSession> {
        let mut set = SessionSet::new();
        set.insert("a".to_string(), CemSession::new(CemConfig::default()));
        set.insert("b".to_string(), CemSession::new(CemConfig::default()));
        set
    }

    #[test]
    fn a_drain_can_be_acted_on_while_the_set_is_still_there() {
        // The reason the keys are cloned: draining and then acting on what was drained is
        // the only thing a caller ever does with the result, and every one of those
        // actions needs the set back.
        let mut set = fleet();
        set.open_all(t(0));
        let frames = set.drain_transmit();
        assert_eq!(frames.len(), 2, "one handshake each");
        for (key, frame) in frames {
            assert!(frame.text.contains("Handshake"));
            // Borrowed keys would have made this line a borrow-check error.
            set.get_mut(&key).expect("still there").close();
        }
        assert_eq!(set.retain_open(), 2);
    }

    #[test]
    fn a_fleet_has_one_deadline_not_a_thousand() {
        let mut set = fleet();
        set.open_all(t(0));
        // Both sessions are waiting on a handshake acknowledgement.
        let soonest = set.poll_timeout().expect("both are waiting on something");
        let (_, same) = set.next_deadline().expect("and one of them owns it");
        assert_eq!(soonest, same);
        assert!(soonest > t(0));
    }

    #[test]
    fn every_due_deadline_fires_in_one_sweep() {
        let mut set = fleet();
        set.open_all(t(0));
        let _ = set.drain_transmit();
        let deadline = set.poll_timeout().expect("a deadline");

        set.handle_timeout(deadline);
        // Both sessions reacted, not just the first one found.
        let events = set.drain_events();
        let mut keys: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 2, "both sessions should have timed out");
    }

    #[test]
    fn transmissions_are_tagged_with_whose_they_are() {
        let mut set = fleet();
        set.open_all(t(0));
        let frames = set.drain_transmit();
        assert!(!frames.is_empty());
        for (key, out) in &frames {
            assert!(key.as_str() == "a" || key.as_str() == "b");
            assert!(out.text.contains("message_type"));
        }
        // Draining twice yields nothing the second time.
        assert!(set.drain_transmit().is_empty());
    }

    #[test]
    fn closed_sessions_are_pruned_rather_than_accumulated() {
        let mut set = fleet();
        set.open_all(t(0));
        assert_eq!(set.retain_open(), 0);
        set.get_mut(&"a".to_string()).expect("session a").close();
        let _ = set.drain_transmit();
        assert_eq!(set.retain_open(), 1);
        assert_eq!(set.len(), 1);
        assert!(set.get(&"a".to_string()).is_none());
    }

    /// The 20 kWh battery, without the `testing` feature's fixture, so this module's
    /// tests run under `--no-default-features` as well.
    fn details() -> crate::types::common::ResourceManagerDetails {
        use crate::types::common::{
            Commodity, CommodityQuantity, ControlType, ResourceManagerDetails, Role, RoleType,
        };
        use crate::types::{Duration, Id};
        ResourceManagerDetails::builder()
            .message_id(Id::new_const("rmd-1"))
            .resource_id(Id::new_const("battery-1"))
            .roles(vec![Role::new(
                RoleType::EnergyStorage,
                Commodity::Electricity,
            )])
            .instruction_processing_delay(Duration::from_millis(500))
            .available_control_types(vec![ControlType::FillRateBasedControl])
            .provides_forecast(false)
            .provides_power_measurement_types(vec![CommodityQuantity::ElectricPower3PhaseSymmetric])
            .build()
    }

    #[test]
    fn the_set_works_for_resource_managers_too() {
        // The point of the trait: a gateway fronting many devices is the mirror image of
        // a CEM managing many, and neither should need its own collection type.
        let mut set: SessionSet<u32, RmSession> = SessionSet::new();
        set.insert(1, RmSession::new(RmConfig::default(), details()));
        set.insert(2, RmSession::new(RmConfig::default(), details()));
        set.open_all(t(0));
        assert_eq!(set.len(), 2);
        assert!(set.poll_timeout().is_some());
        assert!(!set.drain_transmit().is_empty());
        set.close_all();
        let _ = set.drain_transmit();
        assert_eq!(set.retain_open(), 2);
        assert!(set.is_empty());
    }
}
