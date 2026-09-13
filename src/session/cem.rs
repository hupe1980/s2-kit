//! The Customer Energy Manager's side of one S2 session.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::core::{
    AckOutcome, Backoff, CloseReason, CoreConfig, MessageHandle, Negotiation, OutOfOrderPolicy,
    Outgoing, SendError, SessionCore, SessionState,
};
use super::rm::state_name;
use super::{Inbound, InboundPolicy};
use crate::codec::{self, Strictness};
use crate::message::{Message, MessageKind};
use crate::types::common::{
    Consequence, ControlType, EnergyManagementRole, Handshake, HandshakeResponse,
    InstructionStatusUpdate, PowerForecast, PowerMeasurement, ReceptionStatusValues,
    ResourceManagerDetails, RevokableObjects, RevokeObject, SelectControlType, SessionRequest,
    SessionRequestType,
};
use crate::types::{Duration, Id, ProtocolVersion, Timestamp, WireProfile};
use crate::validate::{Report, Validate, rules};

/// How a [`CemSession`] behaves.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CemConfig {
    /// How the protocol version is agreed.
    pub negotiation: Negotiation,
    /// The versions this manager can speak, most preferred first.
    pub supported_versions: Vec<ProtocolVersion>,
    /// How long to wait for an acknowledgement.
    pub ack_timeout: Duration,
    /// What to do about a message that is not allowed in the current state.
    pub out_of_order: OutOfOrderPolicy,
    /// How far a peer's clock may differ before it is worth reporting.
    pub skew_tolerance: Duration,
    /// The largest inbound message that will be parsed.
    pub max_message_bytes: usize,
    /// Whether to prune unknown properties rather than refuse the message.
    pub strictness: Strictness,
    /// Whether the session runs under S2 Connect.
    pub s2_connect: bool,
    /// Whether to validate outbound messages and refuse to send an invalid one.
    pub validate_outbound: bool,
    /// Whether to refuse an instruction whose transition is blocked by a timer.
    ///
    /// Off by default: the S2 documentation, *Operation modes* says a Resource Manager may
    /// change mode without the CEM asking and even without a described transition, so
    /// the manager's picture of the timers is always slightly stale. Refusing to send on
    /// a stale picture is worse than sending and being told no.
    pub enforce_transitions: bool,
    /// How many reconnection attempts have already failed.
    ///
    /// A session is one connection, so it cannot count these itself. Carry the number
    /// across: the `reconnect_after` on a `Closed` event is
    /// [`Backoff::ceiling`](crate::session::Backoff::ceiling) for *this* attempt, and a
    /// session that always starts at zero always recommends two seconds — a back-off that
    /// does not back off.
    pub reconnect_attempt: u32,
}

impl Default for CemConfig {
    fn default() -> Self {
        Self {
            negotiation: Negotiation::Handshake,
            supported_versions: alloc::vec![ProtocolVersion::V1_0_0, ProtocolVersion::V0_0_2_BETA,],
            ack_timeout: Duration::from_secs(5),
            out_of_order: OutOfOrderPolicy::Report,
            skew_tolerance: Duration::from_secs(30),
            max_message_bytes: codec::DecodeOptions::DEFAULT_MAX_BYTES,
            strictness: Strictness::Strict,
            s2_connect: false,
            validate_outbound: true,
            enforce_transitions: false,
            reconnect_attempt: 0,
        }
    }
}

impl CemConfig {
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

    /// Support exactly these versions.
    #[must_use]
    pub fn supporting(mut self, versions: impl IntoIterator<Item = ProtocolVersion>) -> Self {
        self.supported_versions = versions.into_iter().collect();
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

/// Something a Customer Energy Manager's application needs to know about.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CemEvent {
    /// A protocol version was agreed.
    Negotiated {
        /// The version string that was chosen.
        version: ProtocolVersion,
        /// The wire shape it selects.
        profile: WireProfile,
    },
    /// The resource described itself. Now a control type can be selected.
    ResourceDescribed(Box<ResourceManagerDetails>),
    /// The control type this manager selected is active.
    Ready {
        /// What is active.
        control_type: ControlType,
    },
    /// The control type was deactivated.
    Deselected,
    /// The resource published a description or a constraint.
    Description {
        /// Which message.
        kind: MessageKind,
        /// The message.
        message: Message,
    },
    /// The resource reported a status.
    Status {
        /// Which message.
        kind: MessageKind,
        /// The message.
        message: Message,
    },
    /// A measurement arrived.
    Measurement(Box<PowerMeasurement>),
    /// A forecast arrived.
    Forecast(Box<PowerForecast>),
    /// What became of an instruction this manager sent.
    ///
    /// The whole update, not just the status: `timestamp` is "when status_type has
    /// changed the last time", which is **not** when the message arrived, and a manager
    /// reconciling a late `SUCCEEDED` against its own dispatch log needs the difference.
    InstructionStatus(InstructionStatusUpdate),
    /// The resource reported on a timer.
    ///
    /// Reported, not *finished*: `S2J messages/*.TimerStatus.finished_at` says that "if
    /// the timer was never started, the value can be an arbitrary DateTimeStamp in the
    /// past", so a past instant means the timer ran out **or** never ran, and a manager
    /// choosing a transition cares which. The supported reading is narrower — a
    /// `finished_at` in the future blocks, everything else does not — and is the one
    /// [`model::blocking_timers`](crate::model::blocking_timers) uses.
    TimerReported {
        /// The actuator it belongs to, where the control type has actuators.
        actuator: Option<Id>,
        /// Which timer.
        timer: Id,
        /// When it finishes. In the past means it is not blocking — which is not quite
        /// the same as saying it ever ran.
        finished_at: Timestamp,
    },
    /// A description that was published for the future is now in force.
    DescriptionActivated {
        /// Which message became effective.
        kind: MessageKind,
    },
    /// The resource withdrew an object.
    Revoked {
        /// What kind.
        object_type: RevokableObjects,
        /// Which one.
        object_id: Id,
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
    /// Nothing came back in time.
    AckTimedOut(MessageHandle),
    /// An acknowledgement arrived for something we never sent.
    UnmatchedAck {
        /// What it was about.
        subject: Id,
        /// How it was answered.
        status: ReceptionStatusValues,
    },
    /// The resource asked to end or restart the session.
    SessionRequested {
        /// Reconnect or terminate.
        request: SessionRequestType,
        /// What it said about it.
        diagnostic: Option<String>,
    },
    /// An inbound message was refused.
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

/// A Customer Energy Manager's side of one S2 session.
///
/// The mirror of [`RmSession`](super::RmSession), over the same core: the same state
/// table, the same acknowledgement ledger, the same registry. Wiring one of each
/// together through [`crate::testing::Conversation`] is how the documented conversations are
/// tested.
pub struct CemSession {
    core: SessionCore,
    config: CemConfig,
    events: VecDeque<CemEvent>,
    peer_versions: Vec<ProtocolVersion>,
    negotiated: Option<ProtocolVersion>,
    handshake_sent: bool,
    pending_control_type: Option<(Id, ControlType)>,
    policy: Option<Box<dyn InboundPolicy + Send>>,
    backoff: Backoff,
}

impl core::fmt::Debug for CemSession {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CemSession")
            .field("state", &self.core.state)
            .field("profile", &self.core.profile)
            .field("outstanding_acks", &self.core.acks.outstanding())
            .field("queued", &self.core.outbox.len())
            .finish_non_exhaustive()
    }
}

impl CemSession {
    /// A manager's side of a session, not yet opened.
    #[must_use]
    pub fn new(config: CemConfig) -> Self {
        let core = SessionCore::new(CoreConfig {
            role: EnergyManagementRole::Cem,
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
            config,
            events: VecDeque::new(),
            peer_versions: Vec::new(),
            negotiated: None,
            handshake_sent: false,
            pending_control_type: None,
            policy: None,
            backoff: Backoff::default(),
        }
    }

    /// Install a hook consulted before every inbound message is acknowledged.
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

    /// Everything this session has learned about the resource.
    #[must_use]
    pub fn registry(&self) -> &super::core::Registry {
        &self.core.registry
    }

    /// The resource's details, once it has sent them.
    #[must_use]
    pub fn details(&self) -> Option<&ResourceManagerDetails> {
        self.core.registry.details.as_ref()
    }

    /// Begin.
    ///
    /// With a handshake, this queues one — the Resource Manager also sends one, and
    /// neither order is wrong. With a version already agreed, it does nothing but wait
    /// for the resource's details.
    pub fn open(&mut self, now: Timestamp) {
        if !matches!(self.core.state, SessionState::Idle) {
            return;
        }
        match &self.config.negotiation {
            Negotiation::Handshake => {
                self.core.state = SessionState::Handshaking;
                self.handshake_sent = true;
                let handshake = Handshake {
                    message_id: self.core.next_message_id(),
                    role: EnergyManagementRole::Cem,
                    supported_protocol_versions: Some(self.config.supported_versions.clone()),
                };
                self.core.transmit(Message::from(handshake), now);
            }
            Negotiation::PreNegotiated(profile) => {
                let profile = *profile;
                self.core.profile = profile;
                self.negotiated = Some(profile.version());
                self.core.state = SessionState::Connected;
                self.events.push_back(CemEvent::Negotiated {
                    version: profile.version(),
                    profile,
                });
            }
        }
    }

    /// The next message to put on the wire.
    pub fn poll_transmit(&mut self) -> Option<Outgoing> {
        self.core.poll_transmit()
    }

    /// The next thing the application needs to know.
    pub fn poll_event(&mut self) -> Option<CemEvent> {
        self.events.pop_front()
    }

    /// When [`handle_timeout`](Self::handle_timeout) next has something to do.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<Timestamp> {
        self.core.poll_timeout()
    }

    /// Feed in one text frame. Never fails.
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
                    self.events.push_back(CemEvent::Warnings {
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
                self.events.push_back(CemEvent::Refused {
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
            self.events.push_back(CemEvent::Warnings {
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
                EnergyManagementRole::Rm,
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
            self.events.push_back(CemEvent::Refused {
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

        let policy = self
            .policy
            .as_mut()
            .and_then(|policy| policy.review(&message));
        if let Some((status, diagnostic)) = policy {
            self.core
                .acknowledge(id, status, Some(diagnostic.clone()), now);
            let mut refusal = report.clone();
            refusal.push(rules::APPLICATION_REFUSED, "", diagnostic);
            self.events.push_back(CemEvent::Refused {
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
            self.events.push_back(CemEvent::Warnings {
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

    fn process(&mut self, message: Message, now: Timestamp) {
        match &message {
            Message::Handshake(h) => {
                if let Some(versions) = &h.supported_protocol_versions {
                    self.peer_versions.clone_from(versions);
                }
                // The manager is the side that chooses: `S2J messages/Handshake` makes
                // the version list mandatory for the RM and optional for the CEM,
                // because the RM is the constrained side and the manager adapts.
                // Canonical comparison, not `==`: an RM that speaks S2 Connect as well
                // offers `v1.0.0` where this list holds `1.0.0`, and the two are the
                // same version (E25, D31). What goes back out is *our* spelling, so the
                // answer is always a string this side already offered.
                let Some(chosen) = self
                    .config
                    .supported_versions
                    .iter()
                    .find(|ours| self.peer_versions.iter().any(|theirs| ours.matches(theirs)))
                    .cloned()
                else {
                    self.close_with(CloseReason::NoCommonVersion);
                    return;
                };
                let Some(profile) = chosen.wire_profile() else {
                    self.close_with(CloseReason::NoCommonVersion);
                    return;
                };
                self.core.profile = profile;
                self.negotiated = Some(chosen.clone());
                self.core.state = SessionState::Connected;
                let message_id = self.core.next_message_id();
                self.core.transmit(
                    Message::from(HandshakeResponse {
                        message_id,
                        selected_protocol_version: chosen.clone(),
                    }),
                    now,
                );
                self.events.push_back(CemEvent::Negotiated {
                    version: chosen,
                    profile,
                });
                return;
            }
            Message::SessionRequest(request) => {
                self.events.push_back(CemEvent::SessionRequested {
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
                self.events.push_back(CemEvent::Revoked {
                    object_type: revoke.object_type,
                    object_id: revoke.object_id,
                });
                return;
            }
            _ => {}
        }

        self.core.registry.record(&message, Some(now));

        let kind = message.kind();
        match message {
            Message::ResourceManagerDetails(details) => {
                self.events.push_back(CemEvent::ResourceDescribed(details));
            }
            Message::PowerMeasurement(m) => {
                self.events.push_back(CemEvent::Measurement(Box::new(m)));
            }
            Message::PowerForecast(f) => self.events.push_back(CemEvent::Forecast(f)),
            Message::InstructionStatusUpdate(u) => {
                self.events.push_back(CemEvent::InstructionStatus(u));
            }
            Message::FrbcTimerStatus(ref s) => self.events.push_back(CemEvent::TimerReported {
                actuator: Some(s.actuator_id),
                timer: s.timer_id,
                finished_at: s.finished_at,
            }),
            Message::DdbcTimerStatus(ref s) => self.events.push_back(CemEvent::TimerReported {
                actuator: Some(s.actuator_id),
                timer: s.timer_id,
                finished_at: s.finished_at,
            }),
            Message::OmbcTimerStatus(ref s) => self.events.push_back(CemEvent::TimerReported {
                actuator: None,
                timer: s.timer_id,
                finished_at: s.finished_at,
            }),
            other => {
                let is_description = matches!(
                    kind,
                    MessageKind::FrbcSystemDescription
                        | MessageKind::OmbcSystemDescription
                        | MessageKind::DdbcSystemDescription
                        | MessageKind::PebcPowerConstraints
                        | MessageKind::PebcEnergyConstraint
                        | MessageKind::PpbcPowerProfileDefinition
                        | MessageKind::FrbcLeakageBehaviour
                        | MessageKind::FrbcUsageForecast
                        | MessageKind::FrbcFillLevelTargetProfile
                        | MessageKind::DdbcAverageDemandRateForecast
                );
                if is_description {
                    self.events.push_back(CemEvent::Description {
                        kind,
                        message: other,
                    });
                } else {
                    self.events.push_back(CemEvent::Status {
                        kind,
                        message: other,
                    });
                }
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
                // A control type becomes active when the resource says it accepted the
                // selection, not when the manager decided to ask.
                if let Some((id, control_type)) = self.pending_control_type
                    && id == handle.id
                {
                    self.pending_control_type = None;
                    if control_type == ControlType::NoSelection {
                        self.core.state = SessionState::Connected;
                        self.events.push_back(CemEvent::Deselected);
                    } else {
                        self.core.state = SessionState::ControlTypeSelected(control_type);
                        self.events.push_back(CemEvent::Ready { control_type });
                    }
                }
                self.events.push_back(CemEvent::Acked(handle));
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
                if self
                    .pending_control_type
                    .is_some_and(|(id, _)| id == handle.id)
                {
                    self.pending_control_type = None;
                }
                self.events.push_back(CemEvent::Nacked {
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
                self.events.push_back(CemEvent::AckTimedOut(handle));
            }
            AckOutcome::Unmatched { subject, status } => {
                self.core.stats.unmatched_acks = self.core.stats.unmatched_acks.saturating_add(1);
                self.events
                    .push_back(CemEvent::UnmatchedAck { subject, status });
            }
        }
    }

    /// Let time pass.
    pub fn handle_timeout(&mut self, now: Timestamp) {
        for handle in self.core.acks.expire(now) {
            if self
                .pending_control_type
                .is_some_and(|(id, _)| id == handle.id)
            {
                self.pending_control_type = None;
            }
            self.events.push_back(CemEvent::AckTimedOut(handle));
        }
        for message in self.core.registry.activate_due(now) {
            self.events.push_back(CemEvent::DescriptionActivated {
                kind: message.kind(),
            });
        }
    }

    /// The transport went away.
    pub fn transport_closed(&mut self, now: Timestamp) {
        let _ = now;
        self.close_with(CloseReason::TransportClosed);
    }

    /// Activate a control type.
    ///
    /// The session does not become active until the resource acknowledges it, which is
    /// when [`CemEvent::Ready`] arrives.
    pub fn select_control_type(
        &mut self,
        control_type: ControlType,
        now: Timestamp,
    ) -> Result<MessageHandle, SendError> {
        let message_id = self.core.next_message_id();
        let handle = self.send(
            SelectControlType {
                message_id,
                control_type,
            },
            now,
        )?;
        self.pending_control_type = Some((handle.id, control_type));
        Ok(handle)
    }

    /// Send an instruction.
    pub fn instruct(
        &mut self,
        instruction: impl Into<Message>,
        now: Timestamp,
    ) -> Result<MessageHandle, SendError> {
        let message = instruction.into();
        if self.config.enforce_transitions {
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
                crate::model::explain(&message, &ctx)
            };
            // Two different faults, and telling them apart is the point of having rule
            // identifiers at all: a timer that has not finished is a "wait", and an
            // abnormal-condition-only mode used without `abnormal_condition` is a
            // "never".
            if let Some(explanation) = explanation.filter(|e| !e.is_actionable()) {
                let (rule, reason) = if let Some((timer, label)) = explanation.blocked_by.first() {
                    (
                        rules::BLOCKED_BY_TIMER,
                        alloc::format!(
                            "{} {timer} has not finished, and it blocks this transition",
                            label.as_deref().unwrap_or("timer")
                        ),
                    )
                } else {
                    (
                        rules::ABNORMAL_ONLY,
                        "this instruction uses an option marked abnormal_condition_only                          without setting abnormal_condition"
                            .to_string(),
                    )
                };
                let mut report = Report::new();
                report.push(rule, "", reason);
                if let Some(violation) = report.violations().first() {
                    return Err(SendError::Invalid(Box::new(violation.clone())));
                }
            }
        }
        self.send(message, now)
    }

    /// Withdraw something published earlier.
    pub fn revoke(
        &mut self,
        object_type: RevokableObjects,
        object_id: Id,
        now: Timestamp,
    ) -> Result<MessageHandle, SendError> {
        let message_id = self.core.next_message_id();
        let handle = self.send(
            RevokeObject {
                message_id,
                object_type,
                object_id,
            },
            now,
        )?;
        self.core.registry.revoke(object_type, object_id);
        Ok(handle)
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

    /// Send a message.
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
            crate::validate::allowed(self.core.state.phase(), EnergyManagementRole::Cem, kind);
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
                    EnergyManagementRole::Cem,
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
                    .push_back(CemEvent::OutboundWarnings { kind, report });
            }
        }
        self.core.transmit(message, now).ok_or(SendError::Closed)
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
            CloseReason::Local => None,
            // `RECONNECT` — "Please reconnect the WebSocket session" — is an
            // instruction to come straight back, whoever sent it.
            CloseReason::PeerRequested(SessionRequestType::Reconnect)
            | CloseReason::LocallyRequested(SessionRequestType::Reconnect) => Some(Duration::ZERO),
            _ => Some(self.backoff.ceiling(self.core.reconnect_attempt)),
        };
        self.core.state = SessionState::Closed(reason.clone());
        self.events.push_back(CemEvent::Closed {
            reason,
            reconnect_after,
        });
    }
}
