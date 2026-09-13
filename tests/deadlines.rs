//! Every engine must be drivable by its own clock alone.
//!
//! This is the property that makes the sans-I/O design honest. A session that is never
//! spoken to again must still reach a state where it asks for nothing: no deadline, no
//! outstanding work, nothing for the application to poll. An engine that always returns
//! *some* deadline turns an idle device into a busy loop, and an engine whose deadlines
//! go backwards turns one into a spin.
//!
//! The M3 exit criterion: **every engine terminates from `poll_timeout` alone.**

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use s2_kit::prelude::*;
use s2_kit::session::{
    CemConfig, CemEvent, CemSession, Negotiation, RmConfig, RmEvent, RmSession, Session, SessionSet,
};
use s2_kit::testing::{Conversation, battery_details, battery_system};

fn at(s: &str) -> Timestamp {
    s.parse().expect("a valid RFC 3339 timestamp")
}

fn start() -> Timestamp {
    at("2024-01-01T12:00:00Z")
}

/// Drive a session by nothing but its own deadlines, throwing away everything it says.
///
/// Returns the deadlines it asked for, in order. A session that never quiesces makes this
/// run out of steps, which is the failure we are looking for.
fn run_on_its_own_clock<S: Session>(session: &mut S, from: Timestamp) -> Vec<Timestamp> {
    let mut seen = Vec::new();
    let mut now = from;
    for _ in 0..1_000 {
        // Nobody is listening; a session that stalls waiting to be drained is a bug.
        while session.poll_transmit().is_some() {}
        while session.poll_event().is_some() {}

        let Some(deadline) = session.poll_timeout() else {
            return seen;
        };
        assert!(
            deadline >= now,
            "a deadline in the past: asked for {deadline} at {now}"
        );
        seen.push(deadline);
        now = deadline;
        session.handle_timeout(now);
    }
    panic!(
        "the session never went quiet; it asked for {} deadlines, the last at {}",
        seen.len(),
        seen.last().expect("at least one")
    );
}

#[test]
fn a_resource_manager_left_alone_goes_quiet() {
    let mut rm = RmSession::new(RmConfig::default(), battery_details());
    rm.open(start());
    let deadlines = run_on_its_own_clock(&mut rm, start());
    // It asked for at least one — the handshake acknowledgement — and then stopped.
    assert!(!deadlines.is_empty(), "the handshake has a deadline");
    assert_eq!(rm.poll_timeout(), None, "and then nothing more");
}

#[test]
fn a_manager_left_alone_goes_quiet() {
    let mut cem = CemSession::new(CemConfig::default());
    cem.open(start());
    let deadlines = run_on_its_own_clock(&mut cem, start());
    assert!(!deadlines.is_empty());
    assert_eq!(cem.poll_timeout(), None);
}

#[test]
fn a_session_that_was_never_opened_asks_for_nothing() {
    // An engine constructed and then forgotten must not hold a timer open.
    let rm = RmSession::new(RmConfig::default(), battery_details());
    assert_eq!(rm.poll_timeout(), None);
    let cem = CemSession::new(CemConfig::default());
    assert_eq!(cem.poll_timeout(), None);
}

#[test]
fn deadlines_never_go_backwards() {
    let mut rm = RmSession::new(RmConfig::default(), battery_details());
    rm.open(start());
    let deadlines = run_on_its_own_clock(&mut rm, start());
    for pair in deadlines.windows(2) {
        assert!(
            pair[1] >= pair[0],
            "deadline went backwards: {} then {}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn a_timeout_handled_early_changes_nothing() {
    // A driver with a coarse timer wakes a session before its deadline. That must be a
    // no-op, not a spurious acknowledgement timeout.
    let mut rm = RmSession::new(RmConfig::default(), battery_details());
    rm.open(start());
    while rm.poll_transmit().is_some() {}
    let deadline = rm.poll_timeout().expect("waiting on the handshake ack");

    for _ in 0..10 {
        rm.handle_timeout(start());
    }
    assert_eq!(rm.poll_timeout(), Some(deadline), "the deadline stood");
    assert!(rm.poll_event().is_none(), "and nothing was reported");
}

#[test]
fn a_closed_session_asks_for_nothing_more() {
    let mut rm = RmSession::new(RmConfig::default(), battery_details());
    rm.open(start());
    while rm.poll_transmit().is_some() {}
    rm.close();
    while rm.poll_transmit().is_some() {}
    assert!(rm.state().is_closed());
    assert_eq!(rm.poll_timeout(), None);

    // And a transport that dies is the same: an event, then silence.
    let mut cem = CemSession::new(CemConfig::default());
    cem.open(start());
    while cem.poll_transmit().is_some() {}
    cem.transport_closed(start());
    let closed =
        core::iter::from_fn(|| cem.poll_event()).any(|e| matches!(e, CemEvent::Closed { .. }));
    assert!(closed, "a dead transport is reported");
    assert_eq!(cem.poll_timeout(), None);
}

#[test]
fn an_unanswered_message_times_out_exactly_once() {
    let mut rm = RmSession::new(RmConfig::default(), battery_details());
    rm.open(start());
    while rm.poll_transmit().is_some() {}

    let deadline = rm.poll_timeout().expect("the handshake is outstanding");
    rm.handle_timeout(deadline);
    let timeouts = core::iter::from_fn(|| rm.poll_event())
        .filter(|e| matches!(e, RmEvent::AckTimedOut(_)))
        .count();
    assert_eq!(timeouts, 1);

    // Sweeping again produces nothing: the ledger entry is gone, not merely stale.
    rm.handle_timeout(deadline);
    let again = core::iter::from_fn(|| rm.poll_event())
        .filter(|e| matches!(e, RmEvent::AckTimedOut(_)))
        .count();
    assert_eq!(again, 0);
}

#[test]
fn a_description_published_for_the_future_sets_the_deadline_that_activates_it() {
    // A description with a `valid_from` in the future is the one case where a deadline
    // exists for a reason other than an unanswered message.
    let mut c = Conversation::battery();
    c.open();
    c.assert_no_refusals();

    // FRBC must be active before a system description may be sent.
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .expect("the battery offers FRBC");
    c.pump();

    let noon = c.now;
    let later = noon
        .checked_add(Duration::from_secs(3600))
        .expect("an hour");
    c.rm.send(battery_system(later), noon)
        .expect("a system description may be sent once FRBC is active");
    c.pump();

    let deadline =
        c.rm.poll_timeout()
            .expect("the future description must be scheduled");
    assert!(
        deadline <= later,
        "scheduled no later than it becomes valid"
    );

    c.advance(Duration::from_secs(3600));
    let activated = c
        .take_rm_events()
        .into_iter()
        .any(|e| matches!(e, RmEvent::DescriptionActivated { .. }));
    assert!(activated, "the scheduled description came into force");
}

#[test]
fn a_whole_conversation_settles() {
    // Both engines together, driven only by the clock: the transcript must stop growing.
    let mut c = Conversation::battery();
    c.open();
    let after_open = c.transcript().len();

    for _ in 0..50 {
        c.advance(Duration::from_secs(60));
    }
    c.assert_no_refusals();

    // Fifty hours of silence produced no traffic at all: no retry storm, no keep-alive
    // invented by the engine (that belongs to the transport), no re-sent description.
    assert_eq!(
        c.transcript().len(),
        after_open,
        "an idle conversation must be silent"
    );
    assert_eq!(c.rm.poll_timeout(), None);
    assert_eq!(c.cem.poll_timeout(), None);
}

#[test]
fn a_fleet_wakes_once_for_the_soonest_of_all_its_deadlines() {
    let mut fleet: SessionSet<u32, RmSession> = SessionSet::new();
    for id in 0..16 {
        fleet.insert(id, RmSession::new(RmConfig::default(), battery_details()));
    }
    fleet.open_all(start());
    let _ = fleet.drain_transmit();

    let soonest = fleet
        .poll_timeout()
        .expect("sixteen outstanding handshakes");
    // Every session's own deadline is at or after the set's.
    for (_, session) in fleet.iter() {
        if let Some(own) = session.poll_timeout() {
            assert!(own >= soonest);
        }
    }

    // One sweep serves all sixteen, rather than sixteen wake-ups.
    fleet.handle_timeout(soonest);
    assert_eq!(fleet.drain_events().len(), 16);
    assert_eq!(fleet.poll_timeout(), None, "and then the fleet goes quiet");
}

#[test]
fn an_s2_connect_session_has_no_handshake_deadline_at_all() {
    // Under S2 Connect the version is agreed out of band, so the first thing an RM sends
    // is its details — and the deadline structure differs from the handshake case.
    let mut config = RmConfig::default();
    config.negotiation = Negotiation::PreNegotiated(WireProfile::V1_0_0);
    config.s2_connect = true;
    let mut rm = RmSession::new(config, battery_details());
    rm.open(start());
    let deadlines = run_on_its_own_clock(&mut rm, start());
    assert!(!deadlines.is_empty(), "the details are still acknowledged");
    assert_eq!(rm.poll_timeout(), None);
}

#[test]
fn the_reconnection_ceiling_grows_with_the_attempt_the_application_counts() {
    // A session is one connection, so the engine cannot count failed reconnections: it
    // is the application that dials again. Without carrying the number across, every
    // `Closed` event recommended the same two-second ceiling for ever — which is a
    // back-off that does not back off, and exactly what `S2C §Reconnection strategy`
    // (`delay_n = random(0, min(600 s, 2 s · 2^n))`) exists to avoid.
    let now: Timestamp = "2024-01-01T12:00:00Z".parse().unwrap();
    let ceilings: Vec<Option<Duration>> = (0..4)
        .map(|attempt| {
            let config = RmConfig::default().after_failed_attempts(attempt);
            let mut rm = RmSession::new(config, s2_kit::testing::battery_details());
            rm.open(now);
            rm.transport_closed(now);
            rm.poll_event()
                .into_iter()
                .find_map(|e| match e {
                    RmEvent::Closed {
                        reconnect_after, ..
                    } => Some(reconnect_after),
                    _ => None,
                })
                .flatten()
        })
        .collect();
    assert_eq!(
        ceilings,
        vec![
            Some(Duration::from_secs(2)),
            Some(Duration::from_secs(4)),
            Some(Duration::from_secs(8)),
            Some(Duration::from_secs(16)),
        ]
    );
}
