//! Sans-I/O session engines for both roles.
//!
//! A session is driven by four calls in and three out:
//!
//! ```text
//! session.handle_text(&text, now)      session.poll_transmit()
//! session.handle_message(message, now) session.poll_event()
//! session.handle_timeout(now)          session.poll_timeout()
//! session.transport_closed(now)
//! ```
//!
//! No sockets, no clock, no tasks. `now` is a parameter, which is what makes a
//! five-second acknowledgement timeout, a description that becomes valid at midnight and
//! a timer that blocks a transition into ordinary unit tests rather than things you wait
//! for.
//!
//! # What the engine does that an application would otherwise have to
//!
//! * **Acknowledges everything, synchronously.** `S2J` requires a `ReceptionStatus` for
//!   every message except a `ReceptionStatus`, and the engine chooses it inside
//!   `handle_text` — `INVALID_DATA` or `INVALID_MESSAGE` from the codec,
//!   `INVALID_CONTENT` from the state table or the validator (with the failing rule's
//!   identifier in the `diagnostic_label`), otherwise `OK`. Nothing is held across an
//!   `await`, so the deadlock `[s2-json #22]` describes cannot happen.
//! * **Tracks what is outstanding.** Every message sent enters a ledger with a deadline;
//!   the matching acknowledgement resolves it, and a deadline that passes produces an
//!   event rather than a stall.
//! * **Enforces the state table.** `S2C §State of communication` says what may be sent
//!   when; the engine refuses outbound messages at the call site and answers inbound ones
//!   with `INVALID_CONTENT`.
//! * **Resolves instructions.** An [`Instructed`] event carries the actuator, the
//!   operation mode, the factor, the power it implies and any timer in the way — so a
//!   Resource Manager never looks an identifier up by hand.
//! * **Remembers.** Descriptions, constraints, profiles, instruction statuses, timers
//!   and the active operation mode of every actuator, in a bounded [`Registry`].
//!
//! # Both roles, one core
//!
//! [`RmSession`] and [`CemSession`] are built on the same [`SessionCore`], so a CEM and
//! an RM from this crate can be wired back to back in memory — which is how the
//! standard's own documented conversations are tested.

mod analyze;
mod cem;
pub mod core;
mod rm;
mod set;

use alloc::string::String;

pub use analyze::{Analyzer, Observed};
pub use cem::{CemConfig, CemEvent, CemSession};
pub use core::{
    AckLedger, AckOutcome, Backoff, CloseReason, MessageHandle, Negotiation, OutOfOrderPolicy,
    Outgoing, Registry, SendError, SessionCore, SessionState, Stats,
};
pub use rm::{Instructed, RmConfig, RmEvent, RmSession};
pub use set::SessionSet;

use crate::message::Message;
use crate::message::MessageKind;
use crate::types::Timestamp;
use crate::types::common::ReceptionStatusValues;
use crate::validate::Report;

/// What both engines have in common: four calls in, three out, and a clock that is always
/// a parameter.
///
/// The trait exists so that code which only pumps a session — [`SessionSet`],
/// [`crate::io::Driver`], a test harness — does not need to know which role it is pumping.
/// Neither engine is written against it; it describes what they already are.
///
/// It is deliberately not object-safe in spirit but is in fact: [`Self::Event`] keeps the
/// two event enums distinct, because a CEM that receives `RmEvent::Instruction` would be
/// a confusion, not a convenience.
pub trait Session {
    /// What this side tells its application about.
    type Event;

    /// Begin. Has no effect on a session that is already open.
    fn open(&mut self, now: Timestamp);

    /// Feed in one frame as it arrived from the transport.
    fn handle_text(&mut self, text: &str, now: Timestamp) -> Inbound;

    /// Feed in one already-decoded message.
    fn handle_message(&mut self, message: Message, now: Timestamp) -> Inbound;

    /// Tell the session the time has reached [`Self::poll_timeout`].
    fn handle_timeout(&mut self, now: Timestamp);

    /// Tell the session its transport has gone.
    fn transport_closed(&mut self, now: Timestamp);

    /// Take the next frame to send, if any.
    fn poll_transmit(&mut self) -> Option<Outgoing>;

    /// Take the next event for the application, if any.
    fn poll_event(&mut self) -> Option<Self::Event>;

    /// When the session next needs to be woken, if ever.
    fn poll_timeout(&self) -> Option<Timestamp>;

    /// Where the session has got to.
    fn state(&self) -> &SessionState;

    /// Everything the session has learned about its peer.
    fn registry(&self) -> &Registry;

    /// What this session has done, for an operator's metrics.
    fn stats(&self) -> Stats;

    /// Queue a close.
    fn close(&mut self);
}

macro_rules! impl_session {
    ($ty:ty, $event:ty) => {
        impl Session for $ty {
            type Event = $event;

            fn open(&mut self, now: Timestamp) {
                <$ty>::open(self, now);
            }
            fn handle_text(&mut self, text: &str, now: Timestamp) -> Inbound {
                <$ty>::handle_text(self, text, now)
            }
            fn handle_message(&mut self, message: Message, now: Timestamp) -> Inbound {
                <$ty>::handle_message(self, message, now)
            }
            fn handle_timeout(&mut self, now: Timestamp) {
                <$ty>::handle_timeout(self, now);
            }
            fn transport_closed(&mut self, now: Timestamp) {
                <$ty>::transport_closed(self, now);
            }
            fn poll_transmit(&mut self) -> Option<Outgoing> {
                <$ty>::poll_transmit(self)
            }
            fn poll_event(&mut self) -> Option<$event> {
                <$ty>::poll_event(self)
            }
            fn poll_timeout(&self) -> Option<Timestamp> {
                <$ty>::poll_timeout(self)
            }
            fn state(&self) -> &SessionState {
                <$ty>::state(self)
            }
            fn registry(&self) -> &Registry {
                <$ty>::registry(self)
            }
            fn stats(&self) -> Stats {
                <$ty>::stats(self)
            }
            fn close(&mut self) {
                <$ty>::close(self);
            }
        }
    };
}

impl_session!(RmSession, RmEvent);
impl_session!(CemSession, CemEvent);

/// What an engine did with one inbound message.
#[derive(Debug, Clone, PartialEq)]
pub struct Inbound {
    /// What the message claimed to be, where that could be read at all.
    pub kind: Option<MessageKind>,
    /// The reception status that was queued in reply.
    pub status: ReceptionStatusValues,
    /// Everything the validator found, errors and warnings alike.
    pub report: Report,
}

impl Inbound {
    pub(crate) fn ignored(kind: MessageKind) -> Self {
        Self {
            kind: Some(kind),
            status: ReceptionStatusValues::Ok,
            report: Report::new(),
        }
    }

    /// Whether the message was accepted.
    #[must_use]
    pub fn accepted(&self) -> bool {
        self.status == ReceptionStatusValues::Ok
    }
}

/// A hook consulted before an inbound message is acknowledged.
///
/// The engine chooses every reception status itself, because an acknowledgement that an
/// application can forget is one that will be forgotten. This is the exception: the two
/// statuses that are about the *receiver* rather than the message —
/// `TEMPORARY_ERROR` ("Receiver encountered an error", so try again) and
/// `PERMANENT_ERROR` ("an error which it cannot recover from", so disconnect) — are
/// things only the application knows about.
///
/// It is called synchronously, inside `handle_text`, and must not block.
///
/// ```
/// use s2_kit::prelude::*;
/// use s2_kit::session::InboundPolicy;
///
/// struct RefuseWhenStorageIsDown {
///     storage_available: bool,
/// }
///
/// impl InboundPolicy for RefuseWhenStorageIsDown {
///     fn review(&mut self, message: &Message) -> Option<(ReceptionStatusValues, String)> {
///         if !self.storage_available && message.is_instruction() {
///             return Some((
///                 ReceptionStatusValues::TemporaryError,
///                 "the device is not reachable right now".into(),
///             ));
///         }
///         None
///     }
/// }
/// ```
pub trait InboundPolicy {
    /// Refuse the message, or `None` to let the engine decide.
    fn review(&mut self, message: &crate::Message) -> Option<(ReceptionStatusValues, String)>;
}

impl<F> InboundPolicy for F
where
    F: FnMut(&crate::Message) -> Option<(ReceptionStatusValues, String)>,
{
    fn review(&mut self, message: &crate::Message) -> Option<(ReceptionStatusValues, String)> {
        self(message)
    }
}
