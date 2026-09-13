//! What both session engines are built on: the outbox, the acknowledgement ledger, and
//! the registry of everything either side has published.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{self, DecodeOptions};
use crate::message::{Message, MessageKind};
use crate::types::common::{
    Consequence, ControlType, EnergyManagementRole, InstructionStatus, ReceptionStatus,
    ReceptionStatusValues, ResourceManagerDetails, RevokableObjects,
};
use crate::types::{Duration, Id, Timestamp, WireProfile, ddbc, frbc, ombc, pebc, ppbc};
use crate::validate::{Context, TimerState};

/// A message a driver should put on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// The encoded message, ready to send as one text frame.
    pub text: String,
    /// What it is.
    pub kind: MessageKind,
    /// Its identifier, for correlating with the acknowledgement that follows.
    pub message_id: Option<Id>,
}

/// A message that is waiting for its acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MessageHandle {
    /// The `message_id` that was sent.
    pub id: Id,
    /// What was sent.
    pub kind: MessageKind,
}

/// How the session agrees a protocol version with its peer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Negotiation {
    /// Exchange `Handshake` and `HandshakeResponse`, as a bare WebSocket session does.
    #[default]
    Handshake,
    /// The version was already agreed — by S2 Connect's session initiation, where
    /// `S2C §Communication - JSON messages` says the handshake messages "can not be
    /// sent".
    PreNegotiated(WireProfile),
}

/// Where a session has got to.
///
/// `Connected` and `ControlTypeSelected` are the standard's own two states,
/// `WebSocketConnected` and `ControlTypeActivated`. The rest are this crate's: a session
/// that has not started, one that is mid-handshake, and one that is over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Nothing has been sent yet.
    Idle,
    /// The handshake is in progress.
    Handshaking,
    /// `WebSocketConnected`: details, measurements and forecasts flow.
    Connected,
    /// `ControlTypeActivated`. Note that `NOT_CONTROLABLE` is a legal selection and
    /// lands here too, with nothing control-type specific allowed after it.
    ControlTypeSelected(ControlType),
    /// Over, and why.
    Closed(CloseReason),
}

impl SessionState {
    /// Which row of `S2C §State of communication` this session is on.
    ///
    /// [`Idle`](Self::Idle) and [`Handshaking`](Self::Handshaking) are both
    /// [`Phase::Negotiating`](crate::validate::Phase::Negotiating): a session that has not
    /// opened and one that is mid-handshake have the same answer to "may this message
    /// cross?", which is *no, the two sides have not agreed which schema to read it
    /// against*.
    ///
    /// A [`Closed`](Self::Closed) session reports
    /// [`Phase::Connected`](crate::validate::Phase::Connected) and nothing reads
    /// it: every path that consults the table refuses on [`is_closed`](Self::is_closed)
    /// first.
    #[must_use]
    pub fn phase(&self) -> crate::validate::Phase {
        use crate::validate::Phase;
        match self {
            SessionState::Idle | SessionState::Handshaking => Phase::Negotiating,
            SessionState::Connected | SessionState::Closed(_) => Phase::Connected,
            SessionState::ControlTypeSelected(c) => Phase::selected(*c),
        }
    }

    /// The control type that is active, if it is one that can be instructed.
    #[must_use]
    pub fn active_control_type(&self) -> Option<ControlType> {
        match self {
            SessionState::ControlTypeSelected(c) if c.is_controllable() => Some(*c),
            _ => None,
        }
    }

    /// Whether instructions and statuses may flow.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active_control_type().is_some()
    }

    /// Whether the session is over.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        matches!(self, SessionState::Closed(_))
    }
}

/// Why a session ended.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CloseReason {
    /// The application asked for it.
    #[error("closed locally")]
    Local,
    /// The peer sent a `SessionRequest`.
    #[error("the peer asked to {0:?}")]
    PeerRequested(crate::types::common::SessionRequestType),
    /// This side sent a `SessionRequest`.
    ///
    /// Distinct from [`PeerRequested`](Self::PeerRequested) because the two are not the
    /// same event to anybody reading a log: one is a device being told to go away, the
    /// other is a device deciding to. Both still carry the request type, because
    /// `RECONNECT` and `TERMINATE` differ in urgency whichever side asked
    /// (`S2J schemas/SessionRequestType`).
    #[error("this side asked to {0:?}")]
    LocallyRequested(crate::types::common::SessionRequestType),
    /// The transport went away.
    #[error("the transport closed")]
    TransportClosed,
    /// The peer selected a version that was never offered.
    #[error("the peer selected protocol version {selected}, which was not offered")]
    UnsupportedVersion {
        /// What it chose.
        selected: crate::types::ProtocolVersion,
    },
    /// The two sides have no version in common.
    #[error("no protocol version is supported by both sides")]
    NoCommonVersion,
    /// The peer reported an error it cannot recover from.
    ///
    /// `S2J schemas/ReceptionStatusValues`: `PERMANENT_ERROR` — "Consequence: Disconnect."
    #[error("the peer reported a permanent error: {0}")]
    PermanentError(String),
    /// The peer did something the protocol does not allow, and the session was
    /// configured to close on it.
    #[error("protocol error: {0}")]
    ProtocolError(String),
}

/// What to do when a message arrives that is not allowed in the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutOfOrderPolicy {
    /// Answer `INVALID_CONTENT`, report it, and carry on.
    ///
    /// `S2J schemas/ReceptionStatusValues` gives the consequence of `INVALID_CONTENT` as
    /// "Message is ignored, proceed if possible" — a peer that is early is not a peer
    /// that is hostile, and closing on it takes a resource out of a manager's reach for
    /// a race.
    #[default]
    Report,
    /// Answer, then close the session.
    Close,
}

/// Why an outbound message was refused before it reached the wire.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SendError {
    /// The session is over.
    #[error("the session is closed")]
    Closed,
    /// The message is not allowed in this state — the application learns at the call
    /// site rather than from the peer's rejection.
    #[error("{kind} is not allowed while the session is {state}")]
    NotAllowed {
        /// What was being sent.
        kind: MessageKind,
        /// Where the session is.
        state: &'static str,
    },
    /// The message does not exist in the negotiated wire profile.
    #[error("{kind} does not exist in S2 JSON {profile}")]
    NotInProfile {
        /// What was being sent.
        kind: MessageKind,
        /// The profile.
        profile: WireProfile,
    },
    /// The message is invalid, and sending it would only earn an `INVALID_CONTENT`.
    #[error("{0}")]
    Invalid(Box<crate::validate::Violation>),
}

/// What happened to a message that was waiting for an acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckOutcome {
    /// The peer accepted it.
    Accepted {
        /// What was sent.
        handle: MessageHandle,
        /// How long the answer took.
        latency: Duration,
    },
    /// The peer refused it.
    Refused {
        /// What was sent.
        handle: MessageHandle,
        /// How it was refused.
        status: ReceptionStatusValues,
        /// What the peer said about it.
        diagnostic: Option<String>,
        /// What the standard says to do about it.
        consequence: Consequence,
        /// How long the answer took.
        latency: Duration,
    },
    /// Nothing came back in time.
    TimedOut(MessageHandle),
    /// An acknowledgement arrived for something we never sent, or had given up on.
    Unmatched {
        /// What it was about.
        subject: Id,
        /// How it was answered.
        status: ReceptionStatusValues,
    },
}

/// One message the ledger is waiting on.
#[derive(Debug, Clone, Copy)]
struct Outstanding {
    handle: MessageHandle,
    /// When it went out, so the answer can be timed.
    sent_at: Timestamp,
    /// When to give up on it.
    deadline: Timestamp,
}

/// Tracks what has been sent and not yet acknowledged.
///
/// The point of a ledger rather than an `await` is that nothing blocks: a peer that never
/// answers produces an [`AckOutcome::TimedOut`], not a stalled task. That is the deadlock
/// `[s2-json #22]` describes, designed out.
#[derive(Debug, Clone, Default)]
pub struct AckLedger {
    pending: Vec<Outstanding>,
    /// Identifiers already answered, so a duplicate delivery is answered again with the
    /// same status rather than processed twice.
    answered: VecDeque<(Id, ReceptionStatusValues)>,
    max_answered: usize,
}

impl AckLedger {
    /// A ledger that remembers the last `max_answered` inbound identifiers.
    #[must_use]
    pub fn new(max_answered: usize) -> Self {
        Self {
            pending: Vec::new(),
            answered: VecDeque::new(),
            max_answered,
        }
    }

    /// Record an outbound message that expects an acknowledgement.
    pub fn sent(&mut self, handle: MessageHandle, sent_at: Timestamp, deadline: Timestamp) {
        self.pending.push(Outstanding {
            handle,
            sent_at,
            deadline,
        });
    }

    /// Match an inbound `ReceptionStatus` to what it answers.
    // `now` is `Copy` and belongs to the shape of every call that takes a clock.
    #[allow(clippy::needless_pass_by_value)]
    pub fn received(&mut self, status: &ReceptionStatus, now: Timestamp) -> AckOutcome {
        let Some(position) = self
            .pending
            .iter()
            .position(|entry| entry.handle.id == status.subject_message_id)
        else {
            return AckOutcome::Unmatched {
                subject: status.subject_message_id,
                status: status.status,
            };
        };
        let entry = self.pending.remove(position);
        let latency = now.saturating_duration_since(entry.sent_at);
        if status.status == ReceptionStatusValues::Ok {
            AckOutcome::Accepted {
                handle: entry.handle,
                latency,
            }
        } else {
            AckOutcome::Refused {
                handle: entry.handle,
                status: status.status,
                diagnostic: status.diagnostic_label.clone(),
                consequence: status.status.consequence(),
                latency,
            }
        }
    }

    /// Everything whose deadline has passed.
    pub fn expire(&mut self, now: Timestamp) -> Vec<MessageHandle> {
        let mut out = Vec::new();
        self.pending.retain(|entry| {
            if entry.deadline <= now {
                out.push(entry.handle);
                false
            } else {
                true
            }
        });
        out
    }

    /// The earliest deadline still outstanding.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Timestamp> {
        self.pending.iter().map(|entry| entry.deadline).min()
    }

    /// How many messages are waiting.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.pending.len()
    }

    /// Remember how an inbound message was answered.
    pub fn answered(&mut self, id: Id, status: ReceptionStatusValues) {
        if self.answered.len() >= self.max_answered {
            self.answered.pop_front();
        }
        self.answered.push_back((id, status));
    }

    /// How an inbound message was answered before, if it has been seen.
    #[must_use]
    pub fn previous_answer(&self, id: Id) -> Option<ReceptionStatusValues> {
        self.answered
            .iter()
            .rev()
            .find(|(seen, _)| *seen == id)
            .map(|(_, status)| *status)
    }

    /// Whether an identifier has been answered before.
    #[must_use]
    pub fn has_seen(&self, id: Id) -> bool {
        self.previous_answer(id).is_some()
    }

    /// Every identifier answered so far, for the duplicate-identifier rule.
    pub fn seen_ids(&self) -> Vec<Id> {
        self.answered.iter().map(|(id, _)| *id).collect()
    }
}

/// Everything either side has published on this session.
///
/// Every table is bounded: a peer cannot grow memory by sending identifiers it never
/// resolves ([`Registry::MAX_TRACKED`]).
#[derive(Debug, Clone, Default)]
pub struct Registry {
    /// The resource's details.
    pub details: Option<ResourceManagerDetails>,
    /// The FRBC system description in force.
    pub frbc: Option<frbc::SystemDescription>,
    /// The OMBC system description in force.
    pub ombc: Option<ombc::SystemDescription>,
    /// The DDBC system description in force.
    pub ddbc: Option<ddbc::SystemDescription>,
    /// Descriptions whose `valid_from` is still in the future.
    pub scheduled: Vec<(Timestamp, Message)>,
    /// Power constraints in force.
    pub pebc_constraints: Vec<pebc::PowerConstraints>,
    /// Energy constraints in force.
    pub pebc_energy: Vec<pebc::EnergyConstraint>,
    /// Power profiles published.
    pub ppbc_profiles: Vec<ppbc::PowerProfileDefinition>,
    /// Instruction identifiers used.
    pub instructions: Vec<Id>,
    /// `(message_id, instruction_id)` for every instruction seen on this session.
    ///
    /// Kept so that an `InstructionStatusUpdate` naming an identifier nobody instructed
    /// can say *why*: the commonest cause by far is a peer echoing the instruction's
    /// `message_id` instead of its `id`, and the two are both `ID`s and both present in
    /// the message it is answering. The official `s2-example-implementations` battery
    /// does exactly this, so anybody who talks to it meets the mistake on their first
    /// instruction.
    pub instruction_messages: Vec<(Id, Id)>,
    /// The latest status of each instruction.
    pub instruction_statuses: Vec<(Id, InstructionStatus)>,
    /// Objects that could be revoked.
    pub published: Vec<(RevokableObjects, Id)>,
    /// The operation mode each actuator is in (`Id::NIL` for OMBC's single machine).
    pub active_modes: Vec<(Id, Id)>,
    /// When each actuator's timers finish, keyed by actuator **and** timer: a timer
    /// identifier is unique only within the description that declares it.
    pub timers: Vec<TimerState>,
    /// The present fill level, once a storage status has arrived.
    pub fill_level: Option<f64>,
}

impl Registry {
    /// The most identifiers of any one kind that are remembered.
    pub const MAX_TRACKED: usize = 4096;

    /// The most descriptions that may be queued for a future `valid_from`.
    ///
    /// Far smaller than [`MAX_TRACKED`](Self::MAX_TRACKED) because each entry is a whole
    /// decoded message rather than an identifier. An eviction drops the description
    /// furthest from taking effect.
    pub const MAX_SCHEDULED: usize = 16;

    fn bound<T>(v: &mut Vec<T>) {
        if v.len() > Self::MAX_TRACKED {
            v.remove(0);
        }
    }

    fn upsert<T, K: PartialEq>(v: &mut Vec<T>, key: &K, value: T, key_of: impl Fn(&T) -> K) {
        if let Some(slot) = v.iter_mut().find(|existing| key_of(existing) == *key) {
            *slot = value;
        } else {
            v.push(value);
            Self::bound(v);
        }
    }

    /// Record what a message publishes, whichever direction it went.
    ///
    /// A description whose `valid_from` is in the future is held until it becomes
    /// effective; [`Registry::activate_due`] promotes it.
    // `Option<Timestamp>` is `Copy`; taking it by value reads better at every call site.
    #[allow(clippy::needless_pass_by_value)]
    pub fn record(&mut self, message: &Message, now: Option<Timestamp>) {
        if let Some((object_type, id)) = message.revokable_id() {
            Self::upsert(
                &mut self.published,
                &(object_type, id),
                (object_type, id),
                |(t, i)| (*t, *i),
            );
        }
        if let Some(id) = message.instruction_id() {
            if !self.instructions.contains(&id) {
                self.instructions.push(id);
                Self::bound(&mut self.instructions);
            }
            if let Some(message_id) = message.id() {
                Self::upsert(
                    &mut self.instruction_messages,
                    &message_id,
                    (message_id, id),
                    |(m, _)| *m,
                );
            }
        }

        let scheduled_for = match message {
            Message::FrbcSystemDescription(d) => Some(d.valid_from),
            Message::OmbcSystemDescription(d) => Some(d.valid_from),
            Message::DdbcSystemDescription(d) => Some(d.valid_from),
            _ => None,
        };
        if let (Some(valid_from), Some(now)) = (scheduled_for, now)
            && valid_from > now
        {
            self.schedule(valid_from, message);
            return;
        }

        self.apply(message);
    }

    /// Queue a description that is not in force yet, bounded and without duplicates.
    fn schedule(&mut self, valid_from: Timestamp, message: &Message) {
        // A description re-sent with the same identifier replaces the queued one rather
        // than joining it: the second announcement is a correction, not a second event.
        let identity = message.revokable_id();
        if let Some(slot) = self
            .scheduled
            .iter_mut()
            .find(|(_, queued)| queued.revokable_id() == identity)
        {
            *slot = (valid_from, message.clone());
            return;
        }
        self.scheduled.push((valid_from, message.clone()));
        while self.scheduled.len() > Self::MAX_SCHEDULED {
            // Drop the one furthest from taking effect.
            if let Some(furthest) = self
                .scheduled
                .iter()
                .enumerate()
                .max_by_key(|(_, (at, _))| *at)
                .map(|(i, _)| i)
            {
                self.scheduled.remove(furthest);
            } else {
                break;
            }
        }
    }

    /// Make a message's contents effective now.
    fn apply(&mut self, message: &Message) {
        match message {
            Message::ResourceManagerDetails(d) => self.details = Some((**d).clone()),
            // A new system description redefines the scope every operation-mode and
            // timer identifier lives in (`S2J schemas/Timer.id`), so what was known
            // about the old one is not knowledge about the new one. Both tables are
            // cleared, exactly as [`Registry::revoke`] clears them — keeping a
            // `finished_at` from a description that no longer exists is how one
            // actuator's retired timer comes to block a transition of its replacement.
            Message::FrbcSystemDescription(d) => {
                self.frbc = Some((**d).clone());
                self.forget_actuator_state();
            }
            Message::OmbcSystemDescription(d) => {
                self.ombc = Some((**d).clone());
                self.forget_actuator_state();
            }
            Message::DdbcSystemDescription(d) => {
                self.ddbc = Some((**d).clone());
                self.forget_actuator_state();
            }
            Message::PebcPowerConstraints(c) => Self::upsert(
                &mut self.pebc_constraints,
                &c.id,
                (**c).clone(),
                |existing| existing.id,
            ),
            Message::PebcEnergyConstraint(c) => {
                Self::upsert(&mut self.pebc_energy, &c.id, (**c).clone(), |e| e.id);
            }
            Message::PpbcPowerProfileDefinition(p) => {
                Self::upsert(&mut self.ppbc_profiles, &p.id, (**p).clone(), |e| e.id);
            }
            Message::FrbcStorageStatus(s) => self.fill_level = Some(s.present_fill_level),
            Message::FrbcActuatorStatus(s) => {
                Self::upsert(
                    &mut self.active_modes,
                    &s.actuator_id,
                    (s.actuator_id, s.active_operation_mode_id),
                    |(a, _)| *a,
                );
            }
            Message::DdbcActuatorStatus(s) => {
                Self::upsert(
                    &mut self.active_modes,
                    &s.actuator_id,
                    (s.actuator_id, s.active_operation_mode_id),
                    |(a, _)| *a,
                );
            }
            Message::OmbcStatus(s) => {
                Self::upsert(
                    &mut self.active_modes,
                    &Id::NIL,
                    (Id::NIL, s.active_operation_mode_id),
                    |(a, _)| *a,
                );
            }
            Message::FrbcTimerStatus(s) => {
                let actuator = s.actuator_id;
                Self::upsert(
                    &mut self.timers,
                    &(actuator, s.timer_id),
                    TimerState {
                        actuator,
                        timer: s.timer_id,
                        finished_at: s.finished_at,
                    },
                    |t| (t.actuator, t.timer),
                );
            }
            Message::DdbcTimerStatus(s) => {
                let actuator = s.actuator_id;
                Self::upsert(
                    &mut self.timers,
                    &(actuator, s.timer_id),
                    TimerState {
                        actuator,
                        timer: s.timer_id,
                        finished_at: s.finished_at,
                    },
                    |t| (t.actuator, t.timer),
                );
            }
            Message::OmbcTimerStatus(s) => {
                let actuator = Id::NIL;
                Self::upsert(
                    &mut self.timers,
                    &(actuator, s.timer_id),
                    TimerState {
                        actuator,
                        timer: s.timer_id,
                        finished_at: s.finished_at,
                    },
                    |t| (t.actuator, t.timer),
                );
            }
            Message::InstructionStatusUpdate(u) => Self::upsert(
                &mut self.instruction_statuses,
                &u.instruction_id,
                (u.instruction_id, u.status_type),
                |(i, _)| *i,
            ),
            Message::RevokeObject(r) => self.revoke(r.object_type, r.object_id),
            _ => {}
        }
    }

    /// Forget what was known about the operation modes and timers of a description
    /// that has just been replaced or withdrawn.
    fn forget_actuator_state(&mut self) {
        self.active_modes.clear();
        self.timers.clear();
    }

    /// Promote every scheduled description whose time has come.
    pub fn activate_due(&mut self, now: Timestamp) -> Vec<Message> {
        let mut due = Vec::new();
        self.scheduled.retain(|(at, message)| {
            if *at <= now {
                due.push(message.clone());
                false
            } else {
                true
            }
        });
        for message in &due {
            self.apply(message);
        }
        due
    }

    /// The earliest scheduled description still waiting.
    #[must_use]
    pub fn next_activation(&self) -> Option<Timestamp> {
        self.scheduled.iter().map(|(at, _)| *at).min()
    }

    /// Withdraw an object.
    ///
    /// A system description has no `id` of its own, so the standard has its `message_id`
    /// stand in ([`Message::revokable_id`](crate::message::Message::revokable_id)). That
    /// makes the identifier load-bearing rather than decorative: a Resource Manager that
    /// has published description **B** and then revokes the superseded **A** must not
    /// lose **B**. So the stored description is cleared only when it is the one named.
    pub fn revoke(&mut self, object_type: RevokableObjects, object_id: Id) {
        self.published
            .retain(|(t, i)| !(*t == object_type && *i == object_id));
        // A description still waiting for its `valid_from` is revokable too, and it is
        // the one a CEM is most likely to withdraw: it has not taken effect yet.
        self.scheduled
            .retain(|(_, message)| message.revokable_id() != Some((object_type, object_id)));
        match object_type {
            RevokableObjects::PebcPowerConstraints => {
                self.pebc_constraints.retain(|c| c.id != object_id);
            }
            RevokableObjects::PebcEnergyConstraint => {
                self.pebc_energy.retain(|c| c.id != object_id);
            }
            RevokableObjects::PpbcPowerProfileDefinition => {
                self.ppbc_profiles.retain(|p| p.id != object_id);
            }
            RevokableObjects::FrbcSystemDescription => {
                if self
                    .frbc
                    .as_ref()
                    .is_some_and(|d| d.message_id == object_id)
                {
                    self.frbc = None;
                    self.forget_actuator_state();
                }
            }
            RevokableObjects::OmbcSystemDescription => {
                if self
                    .ombc
                    .as_ref()
                    .is_some_and(|d| d.message_id == object_id)
                {
                    self.ombc = None;
                    self.forget_actuator_state();
                }
            }
            RevokableObjects::DdbcSystemDescription => {
                if self
                    .ddbc
                    .as_ref()
                    .is_some_and(|d| d.message_id == object_id)
                {
                    self.ddbc = None;
                    self.forget_actuator_state();
                }
            }
            _ => {
                // An instruction: it stays in `instructions` so that a later status
                // update can still be matched to it, and is marked revoked.
                Self::upsert(
                    &mut self.instruction_statuses,
                    &object_id,
                    (object_id, InstructionStatus::Revoked),
                    |(i, _)| *i,
                );
            }
        }
    }

    /// The validation context this registry supports.
    ///
    /// The argument list is long because a `Context` is: every one of these is a fact a
    /// cross-message rule needs and the registry does not own.
    #[allow(clippy::too_many_arguments)]
    pub fn context<'a>(
        &'a self,
        profile: WireProfile,
        sender: EnergyManagementRole,
        phase: crate::validate::Phase,
        s2_connect: bool,
        now: Timestamp,
        seen: &'a [Id],
        skew_tolerance: Duration,
    ) -> Context<'a> {
        Context {
            profile,
            sender: Some(sender),
            phase,
            s2_connect,
            details: self.details.as_ref(),
            frbc: self.frbc.as_ref(),
            ombc: self.ombc.as_ref(),
            ddbc: self.ddbc.as_ref(),
            pebc_constraints: &self.pebc_constraints,
            ppbc_profiles: &self.ppbc_profiles,
            instructions: &self.instructions,
            instruction_messages: &self.instruction_messages,
            instruction_statuses: &self.instruction_statuses,
            seen_message_ids: seen,
            published: &self.published,
            active_modes: &self.active_modes,
            timers: &self.timers,
            fill_level: self.fill_level,
            now: Some(now),
            skew_tolerance,
        }
    }
}

/// What a session has done, for an operator rather than an application.
///
/// An application reacts to events; an operator asks a fleet *which gateway is being
/// refused, and how slow is the slowest peer?* — and the engine that saw it is the only
/// place the answer exists. Counters rather than a `Metrics` trait: a hook every consumer
/// must implement to learn a number the session already has is a hook.
///
/// Read them with `stats()` and push them wherever the host's metrics go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stats {
    /// Messages put on the wire, `ReceptionStatus` included.
    pub sent: u64,
    /// Frames fed in, whether or not they decoded.
    pub received: u64,
    /// Frames that did not decode at all.
    pub undecodable: u64,
    /// Inbound messages answered with something other than `OK`.
    pub refused: u64,
    /// Inbound messages accepted with warnings — the ones worth looking at before they
    /// become refusals.
    pub accepted_with_warnings: u64,
    /// Outbound messages this side sent that its own validator had something to say
    /// about.
    ///
    /// Counted separately from the inbound ones because they mean the opposite thing: an
    /// inbound warning is a peer to talk to, an outbound one is a bug on this side that
    /// has not been refused yet.
    pub sent_with_warnings: u64,
    /// Duplicate deliveries answered again rather than processed twice.
    pub duplicates: u64,
    /// Outbound messages the peer accepted.
    pub acked: u64,
    /// Outbound messages the peer refused.
    pub nacked: u64,
    /// Outbound messages nothing came back for in time.
    pub ack_timeouts: u64,
    /// Acknowledgements for something never sent, or already given up on.
    pub unmatched_acks: u64,
    /// The sum of every acknowledgement round trip, in milliseconds.
    ///
    /// A sum rather than a mean, because a sum can be added up across sessions;
    /// [`mean_ack_latency_ms`](Self::mean_ack_latency_ms) divides it.
    pub ack_latency_total_ms: u64,
    /// The slowest acknowledgement, in milliseconds — the number a mean hides.
    pub ack_latency_max_ms: u64,
}

impl Stats {
    /// How many outbound messages were answered at all, either way.
    #[must_use]
    pub const fn answered(&self) -> u64 {
        self.acked + self.nacked
    }

    /// The mean acknowledgement round trip in milliseconds, if anything was answered.
    #[must_use]
    pub const fn mean_ack_latency_ms(&self) -> Option<u64> {
        match self.answered() {
            0 => None,
            n => Some(self.ack_latency_total_ms / n),
        }
    }

    pub(crate) fn record_latency(&mut self, latency: Duration) {
        let ms = latency.as_millis();
        self.ack_latency_total_ms = self.ack_latency_total_ms.saturating_add(ms);
        self.ack_latency_max_ms = self.ack_latency_max_ms.max(ms);
    }
}

/// The state both engines share.
#[derive(Debug, Clone)]
pub struct SessionCore {
    /// Where the session has got to.
    pub state: SessionState,
    /// How the version is agreed.
    pub negotiation: Negotiation,
    /// The profile in use, once it is known.
    pub profile: WireProfile,
    /// Whether this session runs under S2 Connect.
    pub s2_connect: bool,
    /// Messages waiting to go out.
    pub outbox: VecDeque<Outgoing>,
    /// What is waiting to be acknowledged.
    pub acks: AckLedger,
    /// What has been published.
    pub registry: Registry,
    /// How long to wait for an acknowledgement.
    pub ack_timeout: Duration,
    /// How the engine answers an out-of-order message.
    pub out_of_order: OutOfOrderPolicy,
    /// How far a peer's clock may differ before it is worth reporting.
    pub skew_tolerance: Duration,
    /// The size cap applied to inbound text.
    pub max_message_bytes: usize,
    /// Whether to prune unknown properties instead of refusing the message.
    pub strictness: codec::Strictness,
    /// How many reconnection attempts have already failed, for the back-off.
    ///
    /// A session object lives for exactly one connection, so this cannot be counted
    /// *here*: it is what the application carries across reconnections, through
    /// `RmConfig::after_failed_attempts` / `CemConfig::after_failed_attempts`. Left at
    /// zero, every `Closed` event would recommend the same two-second ceiling for ever,
    /// which is a back-off that does not back off.
    pub reconnect_attempt: u32,
    /// What this session has done.
    pub stats: Stats,
    /// Which role this engine plays, for the identifiers it mints.
    ///
    /// Only read when `uuid` is off, where it is the prefix that keeps the two sides'
    /// counters legible in a transcript.
    #[cfg_attr(feature = "uuid", allow(dead_code))]
    role: EnergyManagementRole,
    /// The counter behind [`SessionCore::next_message_id`] when `uuid` is off.
    minted: u64,
}

/// What both engines hand the core when they build it.
///
/// A struct rather than eight positional arguments: three of them are a `Duration`, a
/// `usize` and a `bool`, and `new(role, negotiation, ack_timeout, out_of_order,
/// skew_tolerance, ...)` is a call in which swapping the two durations compiles and
/// changes when a session gives up on an acknowledgement.
pub(crate) struct CoreConfig {
    pub role: EnergyManagementRole,
    pub negotiation: Negotiation,
    pub ack_timeout: Duration,
    pub out_of_order: OutOfOrderPolicy,
    pub skew_tolerance: Duration,
    pub max_message_bytes: usize,
    pub strictness: codec::Strictness,
    pub s2_connect: bool,
    pub reconnect_attempt: u32,
}

impl SessionCore {
    pub(crate) fn new(cfg: CoreConfig) -> Self {
        let CoreConfig {
            role,
            negotiation,
            ack_timeout,
            out_of_order,
            skew_tolerance,
            max_message_bytes,
            strictness,
            s2_connect,
            reconnect_attempt,
        } = cfg;
        let profile = match &negotiation {
            Negotiation::PreNegotiated(p) => *p,
            Negotiation::Handshake => WireProfile::V1_0_0,
        };
        Self {
            state: SessionState::Idle,
            negotiation,
            profile,
            s2_connect,
            outbox: VecDeque::new(),
            acks: AckLedger::new(256),
            registry: Registry::default(),
            ack_timeout,
            out_of_order,
            skew_tolerance,
            max_message_bytes,
            strictness,
            reconnect_attempt,
            stats: Stats::default(),
            role,
            minted: 0,
        }
    }

    /// An identifier for a message the engine is generating itself.
    ///
    /// A UUIDv4 with `uuid`, because `s2energy` refuses an `ID` that is not one (E1, R3);
    /// otherwise a per-session counter prefixed with the role, `rm-1`, which the pattern
    /// allows. Either way it must be **unique**: a repeated `message_id` collides in the
    /// acknowledgement ledger and reads as a duplicate to the peer.
    pub(crate) fn next_message_id(&mut self) -> Id {
        self.minted = self.minted.wrapping_add(1);
        #[cfg(feature = "uuid")]
        {
            Id::generate()
        }
        #[cfg(not(feature = "uuid"))]
        {
            use core::fmt::Write as _;
            let mut buffer = alloc::string::String::new();
            let prefix = match self.role {
                EnergyManagementRole::Rm => "rm",
                EnergyManagementRole::Cem => "cem",
            };
            // The pattern allows 64 characters; `cem-` plus a `u64` is at most 24.
            let _ = write!(buffer, "{prefix}-{}", self.minted);
            Id::parse(&buffer).unwrap_or(Id::NIL)
        }
    }

    pub(crate) fn decode_options(&self) -> DecodeOptions {
        DecodeOptions {
            strictness: self.strictness,
            profile: self.profile,
            max_bytes: self.max_message_bytes,
        }
    }

    /// Queue a message, register it for acknowledgement, and record what it publishes.
    // The message is consumed conceptually — the caller has handed it over — even though
    // the encoder and the registry both take it by reference.
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn transmit(&mut self, message: Message, now: Timestamp) -> Option<MessageHandle> {
        let kind = message.kind();
        let id = message.id();
        self.registry.record(&message, Some(now));
        self.outbox.push_back(Outgoing {
            text: codec::encode(&message),
            kind,
            message_id: id,
        });
        self.stats.sent = self.stats.sent.saturating_add(1);
        // `ReceptionStatus` is the one message with no `message_id` of its own, and the
        // one message that is never acknowledged. Both facts are the same fact: nothing
        // without an identifier can be answered, so nothing without one enters the
        // ledger.
        let handle = id.map(|id| MessageHandle { id, kind })?;
        let deadline = now
            .checked_add(self.ack_timeout)
            .unwrap_or(Timestamp::UNIX_EPOCH);
        self.acks.sent(handle, now, deadline);
        Some(handle)
    }

    /// Answer an inbound message, as the standard requires of every message but one.
    // `now` is `Copy` and belongs to the shape of every `handle_*`/`acknowledge` call.
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn acknowledge(
        &mut self,
        subject: Id,
        status: ReceptionStatusValues,
        diagnostic: Option<String>,
        now: Timestamp,
    ) {
        let _ = now;
        self.acks.answered(subject, status);
        if status != ReceptionStatusValues::Ok {
            self.stats.refused = self.stats.refused.saturating_add(1);
        }
        let message = Message::from(ReceptionStatus {
            subject_message_id: subject,
            status,
            diagnostic_label: diagnostic,
        });
        self.outbox.push_back(Outgoing {
            text: codec::encode(&message),
            kind: MessageKind::ReceptionStatus,
            message_id: None,
        });
        self.stats.sent = self.stats.sent.saturating_add(1);
    }

    /// The next instant at which [`handle_timeout`](crate::session::RmSession::handle_timeout)
    /// has something to do.
    ///
    /// `None` once the session is over. A closed session still holds unanswered messages
    /// in its ledger and possibly a description scheduled for tomorrow, but neither can
    /// ever resolve — the transport is gone — so waking a driver for them would keep an
    /// idle device's timer alive for ever.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<Timestamp> {
        if self.state.is_closed() {
            return None;
        }
        match (self.acks.next_deadline(), self.registry.next_activation()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The next message to put on the wire.
    pub fn poll_transmit(&mut self) -> Option<Outgoing> {
        self.outbox.pop_front()
    }
}

/// The reconnection delay S2 Connect prescribes.
///
/// `S2C §Reconnection strategy`: `delay_n = random(0, min(max_delay, base_delay × 2^n))`
/// with a base of two seconds and a maximum of six hundred.
///
/// The randomness is a parameter rather than a call into an RNG, so the core stays pure
/// and a test can pin it. [`Backoff::delay_with`] is the convenience for a driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// The first delay's ceiling.
    pub base: Duration,
    /// The ceiling no delay exceeds.
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(2),
            max: Duration::from_secs(600),
        }
    }
}

impl Backoff {
    /// The ceiling for attempt `n`, counting from zero.
    #[must_use]
    pub fn ceiling(self, attempt: u32) -> Duration {
        let scaled = self
            .base
            .as_millis()
            .saturating_mul(1u64 << attempt.min(40));
        Duration::from_millis(scaled.min(self.max.as_millis()))
    }

    /// The delay for attempt `n`, given a random number in `[0, 1)`.
    #[must_use]
    pub fn delay(self, attempt: u32, random: f64) -> Duration {
        let ceiling = self.ceiling(attempt).as_millis();
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation
        )]
        Duration::from_millis((ceiling as f64 * random.clamp(0.0, 1.0)) as u64)
    }

    /// The delay for attempt `n`, drawing the random number itself.
    #[cfg(feature = "std")]
    #[cfg_attr(docsrs, doc(cfg(feature = "std")))]
    #[must_use]
    pub fn delay_with(self, attempt: u32) -> Duration {
        use rand::RngExt as _;
        let mut rng = rand::rng();
        self.delay(attempt, rng.random_range(0.0..1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::{NumberRange, SessionRequestType};
    use alloc::vec;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn core_config(role: EnergyManagementRole) -> CoreConfig {
        CoreConfig {
            role,
            negotiation: Negotiation::Handshake,
            ack_timeout: Duration::from_secs(5),
            out_of_order: OutOfOrderPolicy::Report,
            skew_tolerance: Duration::from_secs(30),
            max_message_bytes: 1024,
            strictness: codec::Strictness::Strict,
            s2_connect: false,
            reconnect_attempt: 0,
        }
    }

    #[test]
    fn every_identifier_the_engine_mints_is_different() {
        // The property the acknowledgement ledger rests on, and the one a peer's duplicate
        // detection rests on — in every feature configuration, not only with `uuid`.
        let mut core = SessionCore::new(core_config(EnergyManagementRole::Rm));
        let minted: Vec<Id> = (0..64).map(|_| core.next_message_id()).collect();
        let mut unique = minted.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), minted.len(), "identifiers must not repeat");
        assert!(
            !minted.contains(&Id::NIL),
            "the nil identifier means `no message to name`, not `this message`"
        );

        // And the two roles are told apart in a transcript, which is what the prefix is
        // for when there is no UUID to read.
        let mut cem = SessionCore::new(core_config(EnergyManagementRole::Cem));
        let theirs = cem.next_message_id();
        assert_ne!(theirs, minted[0]);
        #[cfg(not(feature = "uuid"))]
        {
            assert!(minted[0].as_str().starts_with("rm-"));
            assert!(theirs.as_str().starts_with("cem-"));
        }
    }

    #[test]
    fn the_ledger_matches_answers_to_what_they_answer() {
        let mut ledger = AckLedger::new(8);
        let handle = MessageHandle {
            id: Id::new_const("m1"),
            kind: MessageKind::FrbcStorageStatus,
        };
        ledger.sent(
            handle,
            at("2024-01-01T12:00:00Z"),
            at("2024-01-01T12:00:05Z"),
        );
        assert_eq!(ledger.outstanding(), 1);

        let outcome = ledger.received(
            &ReceptionStatus::ok(Id::new_const("m1")),
            at("2024-01-01T12:00:00.250Z"),
        );
        // And the round trip is timed, which is the number an operator compares between
        // one gateway and the next.
        assert_eq!(
            outcome,
            AckOutcome::Accepted {
                handle,
                latency: Duration::from_millis(250)
            }
        );
        assert_eq!(ledger.outstanding(), 0);

        // A second answer for the same message matches nothing.
        let outcome = ledger.received(
            &ReceptionStatus::ok(Id::new_const("m1")),
            at("2024-01-01T12:00:01Z"),
        );
        assert!(matches!(outcome, AckOutcome::Unmatched { .. }));
    }

    #[test]
    fn an_unanswered_message_times_out_rather_than_blocking() {
        let mut ledger = AckLedger::new(8);
        let handle = MessageHandle {
            id: Id::new_const("m1"),
            kind: MessageKind::FrbcStorageStatus,
        };
        ledger.sent(
            handle,
            at("2024-01-01T12:00:00Z"),
            at("2024-01-01T12:00:05Z"),
        );
        assert_eq!(ledger.next_deadline(), Some(at("2024-01-01T12:00:05Z")));
        assert!(ledger.expire(at("2024-01-01T12:00:04Z")).is_empty());
        assert_eq!(ledger.expire(at("2024-01-01T12:00:05Z")), vec![handle]);
        assert_eq!(ledger.next_deadline(), None);

        // A late answer is reported, not treated as an error.
        let outcome = ledger.received(
            &ReceptionStatus::ok(Id::new_const("m1")),
            at("2024-01-01T12:00:01Z"),
        );
        assert!(matches!(outcome, AckOutcome::Unmatched { .. }));
    }

    #[test]
    fn a_refusal_carries_the_standards_own_consequence() {
        let mut ledger = AckLedger::new(8);
        let handle = MessageHandle {
            id: Id::new_const("m1"),
            kind: MessageKind::FrbcInstruction,
        };
        ledger.sent(
            handle,
            at("2024-01-01T12:00:00Z"),
            at("2024-01-01T12:00:05Z"),
        );
        let outcome = ledger.received(
            &ReceptionStatus::error(
                Id::new_const("m1"),
                ReceptionStatusValues::TemporaryError,
                "busy",
            ),
            at("2024-01-01T12:00:01Z"),
        );
        match outcome {
            AckOutcome::Refused { consequence, .. } => {
                assert_eq!(consequence, Consequence::Retry);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_answered_ring_is_bounded_and_remembers_the_answer() {
        let mut ledger = AckLedger::new(2);
        ledger.answered(Id::new_const("m1"), ReceptionStatusValues::Ok);
        ledger.answered(Id::new_const("m2"), ReceptionStatusValues::InvalidContent);
        assert_eq!(
            ledger.previous_answer(Id::new_const("m2")),
            Some(ReceptionStatusValues::InvalidContent)
        );
        ledger.answered(Id::new_const("m3"), ReceptionStatusValues::Ok);
        // m1 has fallen out of the ring; a peer cannot grow memory without bound.
        assert_eq!(ledger.previous_answer(Id::new_const("m1")), None);
        assert!(ledger.has_seen(Id::new_const("m3")));
    }

    #[test]
    fn a_description_valid_in_the_future_waits_its_turn() {
        let mut registry = Registry::default();
        let later = at("2024-01-01T13:00:00Z");
        let now = at("2024-01-01T12:00:00Z");
        let description = Message::from(frbc::SystemDescription {
            message_id: Id::new_const("m1"),
            valid_from: later,
            actuators: vec![],
            storage: frbc::StorageDescription {
                diagnostic_label: None,
                fill_level_label: None,
                provides_leakage_behaviour: false,
                provides_fill_level_target_profile: false,
                provides_usage_forecast: false,
                fill_level_range: NumberRange::new(0.0, 100.0),
            },
        });

        registry.record(&description, Some(now));
        assert!(registry.frbc.is_none(), "not effective yet");
        assert_eq!(registry.next_activation(), Some(later));
        assert!(registry.activate_due(at("2024-01-01T12:59:59Z")).is_empty());

        let due = registry.activate_due(later);
        assert_eq!(due.len(), 1);
        assert!(registry.frbc.is_some());
        assert_eq!(registry.next_activation(), None);
    }

    #[test]
    fn revoking_removes_the_object_and_marks_an_instruction_revoked() {
        let mut registry = Registry::default();
        let constraints = pebc::PowerConstraints {
            message_id: Id::new_const("m1"),
            id: Id::new_const("pc1"),
            valid_from: at("2024-01-01T12:00:00Z"),
            valid_until: None,
            consequence_type: pebc::PowerEnvelopeConsequenceType::Vanish,
            allowed_limit_ranges: vec![],
        };
        registry.record(&Message::from(constraints), None);
        assert_eq!(registry.pebc_constraints.len(), 1);
        assert_eq!(registry.published.len(), 1);

        registry.revoke(RevokableObjects::PebcPowerConstraints, Id::new_const("pc1"));
        assert!(registry.pebc_constraints.is_empty());
        assert!(registry.published.is_empty());

        registry.revoke(RevokableObjects::FrbcInstruction, Id::new_const("i1"));
        assert_eq!(
            registry.instruction_statuses,
            vec![(Id::new_const("i1"), InstructionStatus::Revoked)]
        );
    }

    fn frbc_description(message_id: Id, valid_from: Timestamp) -> Message {
        Message::from(frbc::SystemDescription {
            message_id,
            valid_from,
            actuators: vec![],
            storage: frbc::StorageDescription {
                diagnostic_label: None,
                fill_level_label: None,
                provides_leakage_behaviour: false,
                provides_fill_level_target_profile: false,
                provides_usage_forecast: false,
                fill_level_range: NumberRange::new(0.0, 100.0),
            },
        })
    }

    #[test]
    fn revoking_a_superseded_description_leaves_the_live_one_alone() {
        // A system description has no `id`, so its `message_id` stands in. That makes the
        // identifier load-bearing: an RM that published A, then B, and then withdraws the
        // stale A must still have B. Ignoring the identifier would blind the session to
        // the description it is actually running against.
        let mut registry = Registry::default();
        let now = at("2024-01-01T12:00:00Z");
        registry.record(&frbc_description(Id::new_const("sd-a"), now), Some(now));
        registry.record(&frbc_description(Id::new_const("sd-b"), now), Some(now));
        assert_eq!(
            registry.frbc.as_ref().map(|d| d.message_id),
            Some(Id::new_const("sd-b"))
        );

        registry.revoke(
            RevokableObjects::FrbcSystemDescription,
            Id::new_const("sd-a"),
        );
        assert_eq!(
            registry.frbc.as_ref().map(|d| d.message_id),
            Some(Id::new_const("sd-b")),
            "revoking the superseded description must not clear the live one"
        );

        // Revoking the one in force does clear it.
        registry.revoke(
            RevokableObjects::FrbcSystemDescription,
            Id::new_const("sd-b"),
        );
        assert!(registry.frbc.is_none());
    }

    #[test]
    fn revoking_a_description_that_has_not_taken_effect_yet_cancels_it() {
        let mut registry = Registry::default();
        let now = at("2024-01-01T12:00:00Z");
        let later = at("2024-01-01T13:00:00Z");
        registry.record(&frbc_description(Id::new_const("sd-a"), later), Some(now));
        assert_eq!(registry.next_activation(), Some(later));

        registry.revoke(
            RevokableObjects::FrbcSystemDescription,
            Id::new_const("sd-a"),
        );
        assert_eq!(registry.next_activation(), None);
        assert!(registry.activate_due(later).is_empty());
        assert!(registry.frbc.is_none(), "a revoked description never runs");
    }

    #[test]
    fn the_queue_of_future_descriptions_is_bounded_by_message_count_not_identifier_count() {
        // Each entry is a whole decoded message, so the bound has to be a small number.
        // A peer that announces a thousand future shapes of itself must not be able to
        // grow this session's memory a thousand messages' worth.
        let mut registry = Registry::default();
        let now = at("2024-01-01T12:00:00Z");
        for minute in 0..100u32 {
            let at_time = now
                .checked_add(Duration::from_secs(u64::from(minute) * 60 + 60))
                .unwrap();
            let id_str = alloc::format!("sd-{minute:04}");
            registry.record(
                &frbc_description(Id::parse(&id_str).unwrap(), at_time),
                Some(now),
            );
        }
        assert_eq!(registry.scheduled.len(), Registry::MAX_SCHEDULED);
        // The ones kept are the soonest, because those are the ones a CEM has to plan
        // around first.
        let furthest = registry.scheduled.iter().map(|(at, _)| *at).max().unwrap();
        assert_eq!(
            furthest,
            now.checked_add(Duration::from_secs(Registry::MAX_SCHEDULED as u64 * 60))
                .unwrap()
        );
    }

    #[test]
    fn re_announcing_a_future_description_corrects_it_rather_than_queueing_twice() {
        let mut registry = Registry::default();
        let now = at("2024-01-01T12:00:00Z");
        let first = at("2024-01-01T13:00:00Z");
        let corrected = at("2024-01-01T14:00:00Z");
        registry.record(&frbc_description(Id::new_const("sd-a"), first), Some(now));
        registry.record(
            &frbc_description(Id::new_const("sd-a"), corrected),
            Some(now),
        );
        assert_eq!(registry.scheduled.len(), 1);
        assert_eq!(registry.next_activation(), Some(corrected));
    }

    #[test]
    fn two_actuators_may_share_a_timer_identifier() {
        // `S2J schemas/Timer.id`: unique "in the scope of the ... FRBC.ActuatorDescription
        // in which it is used" — so `timer1` on actuator1 and `timer1` on actuator2 are
        // two different timers, and one finishing says nothing about the other.
        let mut registry = Registry::default();
        let mut status = |actuator: &'static str, finished_at: &str| {
            registry.record(
                &Message::from(frbc::TimerStatus {
                    message_id: Id::new_const("m1"),
                    timer_id: Id::new_const("timer1"),
                    actuator_id: Id::new_const(actuator),
                    finished_at: at(finished_at),
                }),
                None,
            );
        };
        status("actuator1", "2024-01-01T13:00:00Z");
        status("actuator2", "2024-01-01T11:00:00Z");

        assert_eq!(registry.timers.len(), 2, "one entry per actuator");
        let finished_at = |actuator: &'static str| {
            registry
                .timers
                .iter()
                .find(|t| t.actuator == Id::new_const(actuator) && t.timer == "timer1")
                .map(|t| t.finished_at)
        };
        assert_eq!(finished_at("actuator1"), Some(at("2024-01-01T13:00:00Z")));
        assert_eq!(finished_at("actuator2"), Some(at("2024-01-01T11:00:00Z")));
    }

    #[test]
    fn resending_a_description_replaces_rather_than_accumulates() {
        let mut registry = Registry::default();
        for level in [10.0, 20.0] {
            registry.record(
                &Message::from(frbc::StorageStatus {
                    message_id: Id::new_const("m1"),
                    present_fill_level: level,
                }),
                None,
            );
        }
        assert_eq!(registry.fill_level, Some(20.0));

        let make = |id: &'static str, until: &str| pebc::PowerConstraints {
            message_id: Id::new_const("mm"),
            id: Id::new_const(id),
            valid_from: at("2024-01-01T12:00:00Z"),
            valid_until: Some(at(until)),
            consequence_type: pebc::PowerEnvelopeConsequenceType::Vanish,
            allowed_limit_ranges: vec![],
        };
        registry.record(&Message::from(make("pc1", "2024-01-01T13:00:00Z")), None);
        registry.record(&Message::from(make("pc1", "2024-01-01T14:00:00Z")), None);
        assert_eq!(registry.pebc_constraints.len(), 1);
        assert_eq!(
            registry.pebc_constraints[0].valid_until,
            Some(at("2024-01-01T14:00:00Z"))
        );
    }

    #[test]
    fn the_backoff_is_the_one_s2_connect_prescribes() {
        let b = Backoff::default();
        // Ceilings double from two seconds and stop at ten minutes.
        assert_eq!(b.ceiling(0), Duration::from_secs(2));
        assert_eq!(b.ceiling(1), Duration::from_secs(4));
        assert_eq!(b.ceiling(2), Duration::from_secs(8));
        assert_eq!(b.ceiling(20), Duration::from_secs(600));
        assert_eq!(b.ceiling(100), Duration::from_secs(600));
        // And the delay is uniform below the ceiling.
        assert_eq!(b.delay(3, 0.0), Duration::ZERO);
        assert_eq!(b.delay(3, 1.0), Duration::from_secs(16));
        assert_eq!(b.delay(3, 0.5), Duration::from_secs(8));
    }

    #[test]
    fn session_state_reports_what_may_flow() {
        assert!(!SessionState::Connected.is_active());
        assert_eq!(SessionState::Connected.active_control_type(), None);
        let active = SessionState::ControlTypeSelected(ControlType::FillRateBasedControl);
        assert!(active.is_active());
        // NOT_CONTROLABLE is a selection, but nothing can be instructed after it.
        let inert = SessionState::ControlTypeSelected(ControlType::NotControllable);
        assert!(!inert.is_active());
        assert!(SessionState::Closed(CloseReason::Local).is_closed());
        let _ = SessionRequestType::Terminate;
    }
}
