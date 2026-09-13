//! The Resource Manager's side of one S2 session.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::core::{
    AckOutcome, Backoff, CloseReason, CoreConfig, MessageHandle, Negotiation, OutOfOrderPolicy,
    Outgoing, SendError, SessionCore, SessionState,
};
use super::{Inbound, InboundPolicy};
use crate::codec::{self, Strictness};
use crate::message::{Message, MessageKind};
use crate::model::{Explanation, explain};
use crate::types::common::{
    Consequence, ControlType, EnergyManagementRole, Handshake, InstructionStatus,
    InstructionStatusUpdate, ReceptionStatusValues, ResourceManagerDetails, RevokableObjects,
    SessionRequest, SessionRequestType,
};
use crate::types::{Duration, Id, ProtocolVersion, Timestamp, WireProfile};
use crate::validate::{Report, Validate, rules};

/// How a [`RmSession`] behaves.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RmConfig {
    /// How the protocol version is agreed.
    pub negotiation: Negotiation,
    /// The versions this Resource Manager offers, most preferred first.
    ///
    /// The default offers both, because every deployed implementation still negotiates
    /// `0.0.2-beta` while the schemas are tagged `v1.0.0`.
    pub offered_versions: Vec<ProtocolVersion>,
    /// How long to wait for an acknowledgement before reporting a timeout.
    ///
    /// A timeout is an **event**, not a disconnect, so this value is low-stakes.
    pub ack_timeout: Duration,
    /// What to do about a message that is not allowed in the current state.
    pub out_of_order: OutOfOrderPolicy,
    /// How far a peer's clock may differ before it is worth reporting.
    pub skew_tolerance: Duration,
    /// The largest inbound message that will be parsed.
    pub max_message_bytes: usize,
    /// Whether to prune unknown properties rather than refuse the message.
    pub strictness: Strictness,
    /// Whether the session runs under S2 Connect, where handshake messages must not
    /// appear.
    pub s2_connect: bool,
    /// Whether to validate outbound messages and refuse to send an invalid one.
    pub validate_outbound: bool,
    /// How many reconnection attempts have already failed.
    ///
    /// A session is one connection, so it cannot count these itself. Carry the number
    /// across: the `reconnect_after` on a `Closed` event is
    /// [`Backoff::ceiling`](crate::session::Backoff::ceiling) for *this* attempt, and a
    /// session that always starts at zero always recommends two seconds — a back-off that
    /// does not back off.
    pub reconnect_attempt: u32,
}

impl Default for RmConfig {
    fn default() -> Self {
        Self {
            negotiation: Negotiation::Handshake,
            offered_versions: alloc::vec![ProtocolVersion::V1_0_0, ProtocolVersion::V0_0_2_BETA,],
            ack_timeout: Duration::from_secs(5),
            out_of_order: OutOfOrderPolicy::Report,
            skew_tolerance: Duration::from_secs(30),
            max_message_bytes: codec::DecodeOptions::DEFAULT_MAX_BYTES,
            strictness: Strictness::Strict,
            s2_connect: false,
            validate_outbound: true,
            reconnect_attempt: 0,
        }
    }
}

impl RmConfig {
    /// Stop checking outbound messages before they go.
    ///
    /// On by default, because learning at the call site beats earning an
    /// `INVALID_CONTENT` from a peer. Turn it off for a bridge that must forward exactly
    /// what it was handed, or for a test that needs a peer's refusal rather than its own.
    #[must_use]
    pub fn without_outbound_validation(mut self) -> Self {
        self.validate_outbound = false;
        self
    }

    /// Use a version that was already agreed, as S2 Connect does.
    #[must_use]
    pub fn pre_negotiated(mut self, profile: WireProfile) -> Self {
        self.negotiation = Negotiation::PreNegotiated(profile);
        self.s2_connect = true;
        self
    }

    /// Offer exactly these versions.
    #[must_use]
    pub fn offering(mut self, versions: impl IntoIterator<Item = ProtocolVersion>) -> Self {
        self.offered_versions = versions.into_iter().collect();
        self
    }

    /// Close the session when a message arrives out of order, instead of reporting it.
    #[must_use]
    pub fn strict_ordering(mut self) -> Self {
        self.out_of_order = OutOfOrderPolicy::Close;
        self
    }

    /// Wait this long for an acknowledgement.
    #[must_use]
    pub fn ack_timeout(mut self, timeout: Duration) -> Self {
        self.ack_timeout = timeout;
        self
    }

    /// Start this session knowing that `attempts` reconnections have already failed.
    ///
    /// What makes the `reconnect_after` on a `Closed` event grow. The application owns
    /// the reconnection loop (D36), so it also owns the counter: reset it to zero after a
    /// session that reached `Connected`, and increment it after one that did not.
    #[must_use]
    pub const fn after_failed_attempts(mut self, attempts: u32) -> Self {
        self.reconnect_attempt = attempts;
        self
    }
}

/// An instruction the Customer Energy Manager sent, already acknowledged and already
/// resolved against the description this session published.
///
/// The point of resolving it here is that a Resource Manager should never have to look
/// an identifier up: it is handed the actuator, the operation mode, the factor, the
/// power that factor implies and the timers in the way. That is what every Resource
/// Manager writes by hand today, and what `hems` asked `s2energy` for.
#[derive(Debug, Clone, PartialEq)]
pub struct Instructed {
    /// The instruction's own identifier — what an [`InstructionStatusUpdate`] names.
    pub id: Id,
    /// Which kind of instruction it is.
    pub kind: MessageKind,
    /// The message itself, for anything the explanation does not cover.
    pub message: Message,
    /// What it means.
    pub explanation: Explanation,
}

/// Something a Resource Manager's application needs to know about.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum RmEvent {
    /// A protocol version was agreed.
    Negotiated {
        /// The version string that was chosen.
        version: ProtocolVersion,
        /// The wire shape it selects.
        profile: WireProfile,
    },
    /// The CEM activated a control type. Send the system description now.
    Ready {
        /// What it activated.
        control_type: ControlType,
    },
    /// The CEM deactivated the control type.
    Deselected,
    /// An instruction arrived, was acknowledged, and resolved.
    Instruction(Box<Instructed>),
    /// A description that was published for the future is now in force.
    DescriptionActivated {
        /// Which message became effective.
        kind: MessageKind,
    },
    /// Something we sent was accepted.
    Acked(MessageHandle),
    /// Something we sent was refused.
    Nacked {
        /// What was refused.
        handle: MessageHandle,
        /// How.
        status: ReceptionStatusValues,
        /// What the peer said.
        diagnostic: Option<String>,
        /// What the standard says to do about it.
        consequence: Consequence,
    },
    /// Nothing came back in time for something we sent.
    AckTimedOut(MessageHandle),
    /// An acknowledgement arrived for something we never sent, or had given up on.
    UnmatchedAck {
        /// What it was about.
        subject: Id,
        /// How it was answered.
        status: ReceptionStatusValues,
    },
    /// The CEM withdrew an object.
    Revoked {
        /// What kind.
        object_type: RevokableObjects,
        /// Which one.
        object_id: Id,
    },
    /// The CEM asked to end or restart the session.
    SessionRequested {
        /// Reconnect or terminate.
        request: SessionRequestType,
        /// What it said about it.
        diagnostic: Option<String>,
    },
    /// An inbound message was refused, and why.
    Refused {
        /// What it claimed to be.
        kind: Option<MessageKind>,
        /// How it was refused.
        status: ReceptionStatusValues,
        /// Everything wrong with it.
        report: Report,
    },
    /// An inbound message was accepted but is suspect.
    Warnings {
        /// What it was.
        kind: MessageKind,
        /// What is suspect about it.
        report: Report,
    },
    /// A message **this side** sent had warnings of its own.
    ///
    /// `validate_outbound` refuses an outbound message with an *error* at the call site;
    /// this is what it has to say about the rest. A resource publishing a description
    /// `s2-python` will refuse (`S2-ACT-001`), or a manager instructing a transition its
    /// own picture of the timers says is blocked (`S2-INST-005`), learns it here rather
    /// than from a peer. Separate from [`Warnings`](Self::Warnings) because the two mean
    /// opposite things: one is a peer worth watching, the other is this side.
    OutboundWarnings {
        /// What was sent.
        kind: MessageKind,
        /// What this side's own validator said about it.
        report: Report,
    },
    /// The session ended.
    Closed {
        /// Why.
        reason: CloseReason,
        /// How long to wait before reconnecting, if reconnecting makes sense.
        reconnect_after: Option<Duration>,
    },
}

/// A Resource Manager's side of one S2 session.
///
/// Sans-I/O: it never touches a socket and never reads a clock. Feed it text and a
/// `now`, drain what it wants to send, drain what it wants to tell you.
///
/// ```
/// use s2_kit::prelude::*;
/// use s2_kit::session::{RmConfig, RmSession};
///
/// # let details = ResourceManagerDetails::builder()
/// #     .resource_id(Id::parse("battery-1").unwrap())
/// #     .roles(vec![Role::new(RoleType::EnergyStorage, Commodity::Electricity)])
/// #     .instruction_processing_delay(Duration::from_millis(500))
/// #     .available_control_types(vec![ControlType::FillRateBasedControl])
/// #     .provides_forecast(false)
/// #     .provides_power_measurement_types(vec![CommodityQuantity::ElectricPowerL1])
/// #     .build();
/// let now: Timestamp = "2024-01-01T12:00:00Z".parse().unwrap();
/// let mut rm = RmSession::new(RmConfig::default(), details);
/// rm.open(now);
///
/// let hello = rm.poll_transmit().unwrap();
/// assert_eq!(hello.kind, MessageKind::Handshake);
/// ```
pub struct RmSession {
    core: SessionCore,
    details: ResourceManagerDetails,
    config: RmConfig,
    events: VecDeque<RmEvent>,
    peer_versions: Vec<ProtocolVersion>,
    negotiated: Option<ProtocolVersion>,
    handshake_seen: bool,
    policy: Option<Box<dyn InboundPolicy + Send>>,
    backoff: Backoff,
}

impl core::fmt::Debug for RmSession {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RmSession")
            .field("state", &self.core.state)
            .field("profile", &self.core.profile)
            .field("outstanding_acks", &self.core.acks.outstanding())
            .field("queued", &self.core.outbox.len())
            .finish_non_exhaustive()
    }
}

impl RmSession {
    /// A session for one resource, not yet opened.
    #[must_use]
    pub fn new(config: RmConfig, details: ResourceManagerDetails) -> Self {
        let core = SessionCore::new(CoreConfig {
            role: EnergyManagementRole::Rm,
            negotiation: config.negotiation.clone(),
            ack_timeout: config.ack_timeout,
            out_of_order: config.out_of_order,
            skew_tolerance: config.skew_tolerance,
            max_message_bytes: config.max_message_bytes,
            strictness: config.strictness,
            s2_connect: config.s2_connect,
            reconnect_attempt: config.reconnect_attempt,
        });
        Self {
            core,
            details,
            config,
            events: VecDeque::new(),
            peer_versions: Vec::new(),
            negotiated: None,
            handshake_seen: false,
            policy: None,
            backoff: Backoff::default(),
        }
    }

    /// Install a hook consulted before every inbound message is acknowledged.
    ///
    /// This is the only way to answer `TEMPORARY_ERROR` or `PERMANENT_ERROR`: the
    /// engine chooses every other status itself, synchronously, so that an
    /// acknowledgement can never be forgotten or delayed.
    pub fn set_inbound_policy(&mut self, policy: Box<dyn InboundPolicy + Send>) {
        self.policy = Some(policy);
    }

    /// Where the session has got to.
    #[must_use]
    pub fn state(&self) -> &SessionState {
        &self.core.state
    }

    /// The wire profile in use.
    #[must_use]
    pub fn profile(&self) -> WireProfile {
        self.core.profile
    }

    /// What this session has done, for an operator's metrics rather than for the
    /// application: how many messages crossed, how many were refused, and how long the
    /// peer takes to answer.
    #[must_use]
    pub fn stats(&self) -> super::core::Stats {
        self.core.stats
    }

    /// Everything this session has learned, for a validator or a user interface.
    #[must_use]
    pub fn registry(&self) -> &super::core::Registry {
        &self.core.registry
    }

    /// Begin. The Resource Manager speaks first.
    ///
    /// With a handshake, this queues one. With a version already agreed — S2 Connect —
    /// it goes straight to sending the resource's details, which is what
    /// `S2C §Communication - JSON messages` requires.
    pub fn open(&mut self, now: Timestamp) {
        if !matches!(self.core.state, SessionState::Idle) {
            return;
        }
        match &self.config.negotiation {
            Negotiation::Handshake => {
                let handshake = Handshake {
                    message_id: self.core.next_message_id(),
                    role: EnergyManagementRole::Rm,
                    supported_protocol_versions: Some(self.config.offered_versions.clone()),
                };
                self.core.state = SessionState::Handshaking;
                self.core.transmit(Message::from(handshake), now);
            }
            Negotiation::PreNegotiated(profile) => {
                let profile = *profile;
                self.core.profile = profile;
                self.negotiated = Some(profile.version());
                self.core.state = SessionState::Connected;
                self.events.push_back(RmEvent::Negotiated {
                    version: profile.version(),
                    profile,
                });
                self.send_details(now);
            }
        }
    }

    fn send_details(&mut self, now: Timestamp) {
        let mut details = self.details.clone();
        details.message_id = self.core.next_message_id();
        self.core.transmit(Message::from(details), now);
    }

    /// The next message to put on the wire.
    pub fn poll_transmit(&mut self) -> Option<Outgoing> {
        self.core.poll_transmit()
    }

    /// The next thing the application needs to know.
    pub fn poll_event(&mut self) -> Option<RmEvent> {
        self.events.pop_front()
    }

    /// When [`handle_timeout`](Self::handle_timeout) next has something to do.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<Timestamp> {
        self.core.poll_timeout()
    }

    /// Feed in one text frame. Never fails: every failure is answered and reported.
    ///
    /// A frame that arrives after the session is over is ignored rather than answered:
    /// the transport is gone, so an acknowledgement queued for it is one nobody will
    /// ever read — and a closed session that keeps filling its outbox is a closed
    /// session that keeps growing. [`handle_message`](Self::handle_message) makes the
    /// same check; this one is for the frames that never get that far.
    pub fn handle_text(&mut self, text: &str, now: Timestamp) -> Inbound {
        if self.core.state.is_closed() {
            return Inbound {
                kind: crate::codec::peek(text).ok().and_then(|p| p.kind),
                status: ReceptionStatusValues::Ok,
                report: Report::new(),
            };
        }
        self.core.stats.received = self.core.stats.received.saturating_add(1);
        let options = self.core.decode_options();
        match codec::decode_with(text, &options) {
            Ok(decoded) => {
                if !decoded.pruned.is_empty() {
                    let mut report = Report::new();
                    for path in &decoded.pruned {
                        report.push(
                            rules::UNKNOWN_PROPERTY,
                            path,
                            "property is not defined by the schema and was removed",
                        );
                    }
                    self.events.push_back(RmEvent::Warnings {
                        kind: decoded.message.kind(),
                        report,
                    });
                }
                self.handle_message(decoded.message, now)
            }
            Err(error) => {
                self.core.stats.undecodable = self.core.stats.undecodable.saturating_add(1);
                let kind = error.peek().and_then(|p| p.kind);
                let status = error.reception_status();
                // A `ReceptionStatus` is never acknowledged, even a broken one:
                // answering it would be an infinite politeness.
                if kind != Some(MessageKind::ReceptionStatus) {
                    self.core.acknowledge(
                        status.subject_message_id,
                        status.status,
                        status.diagnostic_label.clone(),
                        now,
                    );
                }
                let mut report = Report::new();
                report.push(rules::DECODE_FAILED, "", error.to_string());
                self.events.push_back(RmEvent::Refused {
                    kind,
                    status: status.status,
                    report: report.clone(),
                });
                Inbound {
                    kind,
                    status: status.status,
                    report,
                }
            }
        }
    }

    /// Feed in one already-decoded message.
    pub fn handle_message(&mut self, message: Message, now: Timestamp) -> Inbound {
        if self.core.state.is_closed() {
            return Inbound::ignored(message.kind());
        }

        // A ReceptionStatus is the one message that is never itself acknowledged.
        if let Message::ReceptionStatus(status) = &message {
            self.on_reception_status(status, now);
            return Inbound {
                kind: Some(MessageKind::ReceptionStatus),
                status: ReceptionStatusValues::Ok,
                report: Report::new(),
            };
        }

        let kind = message.kind();
        let Some(id) = message.id() else {
            return Inbound::ignored(kind);
        };

        // A duplicate delivery is answered again with the same status and not processed
        // twice, which makes the engine idempotent under a proxy that retries.
        if let Some(previous) = self.core.acks.previous_answer(id) {
            // Answered again with the same status, and deliberately not processed twice:
            // that is what makes the engine idempotent under a proxy that retries. It is
            // reported, because a peer that reuses identifiers is a bug worth seeing.
            self.core.acknowledge(id, previous, None, now);
            self.core.stats.duplicates = self.core.stats.duplicates.saturating_add(1);
            let mut report = Report::new();
            report.push_related(
                rules::DUPLICATE_MESSAGE_ID,
                "/message_id",
                alloc::format!("{id} was already answered {previous:?}; not processed again"),
                [id],
            );
            self.events.push_back(RmEvent::Warnings {
                kind,
                report: report.clone(),
            });
            return Inbound {
                kind: Some(kind),
                status: previous,
                report,
            };
        }

        let seen = self.core.acks.seen_ids();
        let report = {
            let ctx = self.core.registry.context(
                self.core.profile,
                EnergyManagementRole::Cem,
                self.core.state.phase(),
                self.core.s2_connect,
                now,
                &seen,
                self.core.skew_tolerance,
            );
            message.validate(&ctx)
        };

        if report.has_errors() {
            let status = ReceptionStatusValues::InvalidContent;
            self.core
                .acknowledge(id, status, report.diagnostic_label(), now);
            let closing = self.config.out_of_order == OutOfOrderPolicy::Close
                && (report.contains(rules::NOT_ALLOWED_IN_STATE)
                    || report.contains(rules::NOT_ALLOWED_FOR_ROLE));
            self.events.push_back(RmEvent::Refused {
                kind: Some(kind),
                status,
                report: report.clone(),
            });
            if closing {
                let reason = CloseReason::ProtocolError(
                    report
                        .diagnostic_label()
                        .unwrap_or_else(|| "out of order".to_string()),
                );
                self.close_with(reason);
            }
            return Inbound {
                kind: Some(kind),
                status,
                report,
            };
        }

        // The application's only chance to refuse something it cannot store or act on.
        let policy = self
            .policy
            .as_mut()
            .and_then(|policy| policy.review(&message));
        if let Some((status, diagnostic)) = policy {
            self.core
                .acknowledge(id, status, Some(diagnostic.clone()), now);
            let mut refusal = report.clone();
            refusal.push(rules::APPLICATION_REFUSED, "", diagnostic);
            self.events.push_back(RmEvent::Refused {
                kind: Some(kind),
                status,
                report: refusal,
            });
            return Inbound {
                kind: Some(kind),
                status,
                report,
            };
        }

        self.core
            .acknowledge(id, ReceptionStatusValues::Ok, None, now);
        if !report.is_empty() {
            self.core.stats.accepted_with_warnings =
                self.core.stats.accepted_with_warnings.saturating_add(1);
            self.events.push_back(RmEvent::Warnings {
                kind,
                report: report.clone(),
            });
        }
        self.process(message, now);
        Inbound {
            kind: Some(kind),
            status: ReceptionStatusValues::Ok,
            report,
        }
    }

    /// Act on a message that has been accepted and acknowledged.
    fn process(&mut self, message: Message, now: Timestamp) {
        match &message {
            Message::Handshake(h) => {
                self.handshake_seen = true;
                if let Some(versions) = &h.supported_protocol_versions {
                    self.peer_versions.clone_from(versions);
                }
            }
            Message::HandshakeResponse(response) => {
                let selected = response.selected_protocol_version.clone();
                // Canonical comparison, not `==`: the two specifications spell one
                // version two ways and a CEM built on S2 Connect answers `v1.0.0` where
                // an S2 JSON handshake writes `1.0.0` (E25, D31). Refusing that is
                // refusing a peer that selected exactly what was offered.
                if !self
                    .config
                    .offered_versions
                    .iter()
                    .any(|offered| offered.matches(&selected))
                {
                    self.close_with(CloseReason::UnsupportedVersion { selected });
                    return;
                }
                let Some(profile) = selected.wire_profile() else {
                    self.close_with(CloseReason::UnsupportedVersion { selected });
                    return;
                };
                self.core.profile = profile;
                self.negotiated = Some(selected.clone());
                self.core.state = SessionState::Connected;
                self.events.push_back(RmEvent::Negotiated {
                    version: selected,
                    profile,
                });
                self.send_details(now);
            }
            Message::SelectControlType(select) => {
                let control_type = select.control_type;
                if control_type == ControlType::NoSelection {
                    self.core.state = SessionState::Connected;
                    self.events.push_back(RmEvent::Deselected);
                } else {
                    self.core.state = SessionState::ControlTypeSelected(control_type);
                    self.events.push_back(RmEvent::Ready { control_type });
                }
            }
            Message::SessionRequest(request) => {
                self.events.push_back(RmEvent::SessionRequested {
                    request: request.request,
                    diagnostic: request.diagnostic_label.clone(),
                });
                self.close_with(CloseReason::PeerRequested(request.request));
                return;
            }
            Message::RevokeObject(revoke) => {
                self.core
                    .registry
                    .revoke(revoke.object_type, revoke.object_id);
                self.events.push_back(RmEvent::Revoked {
                    object_type: revoke.object_type,
                    object_id: revoke.object_id,
                });
                return;
            }
            _ => {}
        }

        self.core.registry.record(&message, Some(now));

        if let Some(id) = message.instruction_id() {
            let seen = self.core.acks.seen_ids();
            let explanation = {
                let ctx = self.core.registry.context(
                    self.core.profile,
                    EnergyManagementRole::Cem,
                    self.core.state.phase(),
                    self.core.s2_connect,
                    now,
                    &seen,
                    self.core.skew_tolerance,
                );
                explain(&message, &ctx)
            };
            if let Some(explanation) = explanation {
                self.events
                    .push_back(RmEvent::Instruction(Box::new(Instructed {
                        id,
                        kind: message.kind(),
                        message,
                        explanation,
                    })));
            }
        }
    }

    fn on_reception_status(
        &mut self,
        status: &crate::types::common::ReceptionStatus,
        now: Timestamp,
    ) {
        match self.core.acks.received(status, now) {
            AckOutcome::Accepted { handle, latency } => {
                self.core.stats.acked = self.core.stats.acked.saturating_add(1);
                self.core.stats.record_latency(latency);
                self.events.push_back(RmEvent::Acked(handle));
            }
            AckOutcome::Refused {
                handle,
                status,
                diagnostic,
                consequence,
                latency,
            } => {
                self.core.stats.nacked = self.core.stats.nacked.saturating_add(1);
                self.core.stats.record_latency(latency);
                self.events.push_back(RmEvent::Nacked {
                    handle,
                    status,
                    diagnostic: diagnostic.clone(),
                    consequence,
                });
                if consequence == Consequence::Disconnect {
                    self.close_with(CloseReason::PermanentError(
                        diagnostic.unwrap_or_else(|| "no diagnostic".to_string()),
                    ));
                }
            }
            AckOutcome::TimedOut(handle) => {
                self.core.stats.ack_timeouts = self.core.stats.ack_timeouts.saturating_add(1);
                self.events.push_back(RmEvent::AckTimedOut(handle));
            }
            AckOutcome::Unmatched { subject, status } => {
                self.core.stats.unmatched_acks = self.core.stats.unmatched_acks.saturating_add(1);
                self.events
                    .push_back(RmEvent::UnmatchedAck { subject, status });
            }
        }
    }

    /// Let time pass: expire acknowledgements and activate scheduled descriptions.
    pub fn handle_timeout(&mut self, now: Timestamp) {
        for handle in self.core.acks.expire(now) {
            self.events.push_back(RmEvent::AckTimedOut(handle));
        }
        for message in self.core.registry.activate_due(now) {
            self.events.push_back(RmEvent::DescriptionActivated {
                kind: message.kind(),
            });
        }
    }

    /// The transport went away.
    pub fn transport_closed(&mut self, now: Timestamp) {
        let _ = now;
        self.close_with(CloseReason::TransportClosed);
    }

    /// Send a message.
    ///
    /// Refused at the call site when the state table does not allow it, when it does not
    /// exist in the negotiated profile, or — unless `validate_outbound` is off — when it
    /// would only earn an `INVALID_CONTENT` in reply.
    pub fn send(
        &mut self,
        message: impl Into<Message>,
        now: Timestamp,
    ) -> Result<MessageHandle, SendError> {
        let message = message.into();
        let kind = message.kind();
        if self.core.state.is_closed() {
            return Err(SendError::Closed);
        }
        if !kind.exists_in(self.core.profile) {
            return Err(SendError::NotInProfile {
                kind,
                profile: self.core.profile,
            });
        }
        let allowance =
            crate::validate::allowed(self.core.state.phase(), EnergyManagementRole::Rm, kind);
        if !allowance.is_allowed() {
            return Err(SendError::NotAllowed {
                kind,
                state: state_name(&self.core.state),
            });
        }
        if self.config.validate_outbound {
            let seen = self.core.acks.seen_ids();
            let report = {
                let ctx = self.core.registry.context(
                    self.core.profile,
                    EnergyManagementRole::Rm,
                    self.core.state.phase(),
                    self.core.s2_connect,
                    now,
                    &seen,
                    self.core.skew_tolerance,
                );
                message.validate(&ctx)
            };
            if let Some(violation) = report.first_error() {
                return Err(SendError::Invalid(Box::new(violation.clone())));
            }
            if !report.is_empty() {
                self.core.stats.sent_with_warnings =
                    self.core.stats.sent_with_warnings.saturating_add(1);
                self.events
                    .push_back(RmEvent::OutboundWarnings { kind, report });
            }
        }
        self.core.transmit(message, now).ok_or(SendError::Closed)
    }

    /// Report what became of an instruction.
    ///
    /// The second of the two answers an instruction gets: the reception status said the
    /// message was read, this says what the household did about it.
    pub fn instruction_status(
        &mut self,
        instruction_id: Id,
        status: InstructionStatus,
        now: Timestamp,
    ) -> Result<MessageHandle, SendError> {
        let message_id = self.core.next_message_id();
        self.send(
            InstructionStatusUpdate {
                message_id,
                instruction_id,
                status_type: status,
                timestamp: now,
            },
            now,
        )
    }

    /// Ask the peer to end or restart the session.
    pub fn request_session(
        &mut self,
        request: SessionRequestType,
        diagnostic: Option<String>,
        now: Timestamp,
    ) -> Result<MessageHandle, SendError> {
        let message_id = self.core.next_message_id();
        let handle = self.send(
            SessionRequest {
                message_id,
                request,
                diagnostic_label: diagnostic,
            },
            now,
        )?;
        // Having asked to end the session, this side stops using it. The request is
        // already queued, so the peer still hears about it.
        self.close_with(CloseReason::LocallyRequested(request));
        Ok(handle)
    }

    /// End the session locally.
    pub fn close(&mut self) {
        self.close_with(CloseReason::Local);
    }

    fn close_with(&mut self, reason: CloseReason) {
        if self.core.state.is_closed() {
            return;
        }
        let reconnect_after = match &reason {
            // Nothing to come back to.
            CloseReason::Local => None,
            // `RECONNECT` — "Please reconnect the WebSocket session" — is an
            // instruction to come straight back, whoever sent it.
            CloseReason::PeerRequested(SessionRequestType::Reconnect)
            | CloseReason::LocallyRequested(SessionRequestType::Reconnect) => Some(Duration::ZERO),
            _ => Some(self.backoff.ceiling(self.core.reconnect_attempt)),
        };
        self.core.state = SessionState::Closed(reason.clone());
        self.events.push_back(RmEvent::Closed {
            reason,
            reconnect_after,
        });
    }
}

pub(crate) fn state_name(state: &SessionState) -> &'static str {
    match state {
        SessionState::Idle => "idle",
        SessionState::Handshaking => "handshaking",
        SessionState::Connected => "connected",
        SessionState::ControlTypeSelected(_) => "control type selected",
        SessionState::Closed(_) => "closed",
    }
}
