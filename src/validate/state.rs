//! The normative table of what may be sent when.
//!
//! `S2C §State of communication` gives it as a table with one row per state and one
//! column per role. This module is that table, and it is the only place the rule lives:
//! the session engines consult it rather than carrying a second copy.
//!
//! ```text
//! WebSocketConnected ──SelectControlType(ct)──▶ ControlTypeActivated(ct)
//!         ▲                                             │
//!         └──────── SelectControlType(NO_SELECTION) ─────┘
//! ```

use crate::message::MessageKind;
use crate::types::common::{ControlType, EnergyManagementRole};

/// Whether a message may be sent, and if not, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allowance {
    /// It may.
    Yes,
    /// Not in this state.
    WrongState,
    /// Not from this role.
    WrongRole,
}

impl Allowance {
    /// Whether the message is allowed.
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        matches!(self, Allowance::Yes)
    }
}

/// Where a session has got to, as the state table sees it.
///
/// The standard's own table has two rows — `WebSocket Connected` and
/// `ControlType <X> activated` — because under S2 Connect the version is settled before
/// the socket opens and there is nothing before the first row. A bare-WebSocket session
/// has one more: `S2J messages/Handshake` is exchanged first, and until it is, the two
/// sides have not agreed which schema the next message is to be read against.
///
/// [`Negotiating`](Self::Negotiating) is that row. Leaving it out is what lets a
/// `PowerMeasurement` sent before the handshake — a message whose *version* nobody has
/// agreed — pass a check named after the state it is not in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// Before the handshake has completed, on a session that has one.
    ///
    /// Only `Handshake`, `HandshakeResponse`, `SessionRequest` and `ReceptionStatus` may
    /// cross. A session under S2 Connect is never in this phase: session initiation
    /// already agreed the version, and the handshake messages "can not be sent".
    Negotiating,
    /// `WebSocketConnected`, the standard's first row.
    ///
    /// Also where a session sits after `NO_SELECTION` or `NOT_CONTROLABLE`, which are a
    /// deselection and a selection with nothing to instruct.
    #[default]
    Connected,
    /// `ControlTypeActivated`, the standard's per-control-type rows.
    Activated(ControlType),
}

impl Phase {
    /// The phase a control-type selection puts a session in.
    ///
    /// `NO_SELECTION` and `NOT_CONTROLABLE` are [`Connected`](Self::Connected): the first
    /// is a deselection, the second a legal selection after which nothing control-type
    /// specific may flow (docs `learn/examples/nocontrol`).
    #[must_use]
    pub fn selected(control_type: ControlType) -> Self {
        if control_type.is_controllable() {
            Phase::Activated(control_type)
        } else {
            Phase::Connected
        }
    }

    /// The control type that is active, if one that can be instructed is.
    #[must_use]
    pub const fn active_control_type(self) -> Option<ControlType> {
        match self {
            Phase::Activated(c) => Some(c),
            Phase::Negotiating | Phase::Connected => None,
        }
    }

    /// The row's name, for a diagnostic.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Phase::Negotiating => "negotiating",
            Phase::Connected => "WebSocketConnected",
            Phase::Activated(c) => c.abbreviation().unwrap_or("WebSocketConnected"),
        }
    }
}

/// Whether `sender` may send `kind` while the session is in `phase`.
#[must_use]
pub fn allowed(phase: Phase, sender: EnergyManagementRole, kind: MessageKind) -> Allowance {
    use MessageKind as K;

    if let Some(expected) = kind.sender()
        && expected != sender
    {
        return Allowance::WrongRole;
    }

    let active = phase.active_control_type();

    // The arms below are grouped by *reason*, not by outcome, so several share a body on
    // purpose: collapsing them would lose the row of the standard's table each belongs to.
    #[allow(clippy::match_same_arms)]
    match kind {
        // Never state-dependent: either side, at any point. A handshake has to be
        // answerable, and either side may give up at any moment.
        K::ReceptionStatus | K::SessionRequest => Allowance::Yes,

        // The handshake belongs to the start of a session, and only to it. Under S2
        // Connect it does not happen at all, which is a warning rather than a state error
        // (E9 / S2-STATE-003).
        K::Handshake | K::HandshakeResponse => match phase {
            Phase::Negotiating | Phase::Connected => Allowance::Yes,
            Phase::Activated(_) => Allowance::WrongState,
        },

        // Everything below needs an agreed version to be read against, so nothing but the
        // handshake itself crosses while one is still being agreed.
        _ if phase == Phase::Negotiating => Allowance::WrongState,

        // The RM's standing capabilities, and the CEM's ability to change its mind.
        // `S2C`: measurements and forecasts "can always be used, even if no Control Type
        // has been activated".
        K::ResourceManagerDetails
        | K::PowerMeasurement
        | K::PowerForecast
        | K::SelectControlType => Allowance::Yes,

        // Only meaningful once something can be instructed.
        K::InstructionStatusUpdate | K::RevokeObject => {
            if active.is_some() {
                Allowance::Yes
            } else {
                Allowance::WrongState
            }
        }

        // Everything else belongs to exactly one control type.
        _ => match (kind.control_type(), active) {
            (Some(needed), Some(current)) if needed == current => Allowance::Yes,
            _ => Allowance::WrongState,
        },
    }
}

/// Every message the role may send in the phase, for documentation and tests.
#[must_use]
pub fn allowed_kinds(phase: Phase, sender: EnergyManagementRole) -> alloc::vec::Vec<MessageKind> {
    MessageKind::ALL
        .iter()
        .copied()
        .filter(|k| allowed(phase, sender, *k).is_allowed())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use EnergyManagementRole::{Cem, Rm};
    use alloc::vec::Vec;

    fn names(phase: Phase, sender: EnergyManagementRole) -> Vec<&'static str> {
        let mut v: Vec<_> = allowed_kinds(phase, sender)
            .into_iter()
            .map(crate::message::MessageKind::as_str)
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn the_websocket_connected_row_is_the_standards_own() {
        // `S2C §State of communication`, row "WebSocket Connected". The handshake
        // messages are added because a bare-WebSocket session exchanges them here.
        assert_eq!(
            names(Phase::Connected, Cem),
            alloc::vec![
                "Handshake",
                "HandshakeResponse",
                "ReceptionStatus",
                "SelectControlType",
                "SessionRequest",
            ]
        );
        assert_eq!(
            names(Phase::Connected, Rm),
            alloc::vec![
                "Handshake",
                "PowerForecast",
                "PowerMeasurement",
                "ReceptionStatus",
                "ResourceManagerDetails",
                "SessionRequest",
            ]
        );
    }

    #[test]
    fn nothing_but_the_handshake_crosses_while_the_version_is_still_being_agreed() {
        // The row the standard's table does not have, because under S2 Connect there is
        // nothing before `WebSocketConnected`. A bare-WebSocket session does have one,
        // and a message sent in it is a message whose schema version nobody has agreed.
        assert_eq!(
            names(Phase::Negotiating, Rm),
            alloc::vec!["Handshake", "ReceptionStatus", "SessionRequest"]
        );
        assert_eq!(
            names(Phase::Negotiating, Cem),
            alloc::vec![
                "Handshake",
                "HandshakeResponse",
                "ReceptionStatus",
                "SessionRequest",
            ]
        );
        // Including the RM's standing capabilities, which are unrestricted *afterwards*.
        assert_eq!(
            allowed(Phase::Negotiating, Rm, MessageKind::PowerMeasurement),
            Allowance::WrongState
        );
        assert_eq!(
            allowed(Phase::Negotiating, Rm, MessageKind::ResourceManagerDetails),
            Allowance::WrongState
        );
        assert_eq!(
            allowed(Phase::Negotiating, Cem, MessageKind::SelectControlType),
            Allowance::WrongState
        );
    }

    #[test]
    fn the_handshake_is_over_once_a_control_type_is_active() {
        assert_eq!(
            allowed(
                Phase::Activated(ControlType::FillRateBasedControl),
                Rm,
                MessageKind::Handshake
            ),
            Allowance::WrongState
        );
    }

    #[test]
    fn the_frbc_row_is_the_standards_own() {
        let frbc = Phase::Activated(ControlType::FillRateBasedControl);
        assert_eq!(
            names(frbc, Cem),
            alloc::vec![
                "FRBC.Instruction",
                "ReceptionStatus",
                "RevokeObject",
                "SelectControlType",
                "SessionRequest",
            ]
        );
        assert_eq!(
            names(frbc, Rm),
            alloc::vec![
                "FRBC.ActuatorStatus",
                "FRBC.FillLevelTargetProfile",
                "FRBC.LeakageBehaviour",
                "FRBC.StorageStatus",
                "FRBC.SystemDescription",
                "FRBC.TimerStatus",
                "FRBC.UsageForecast",
                "InstructionStatusUpdate",
                "PowerForecast",
                "PowerMeasurement",
                "ReceptionStatus",
                "ResourceManagerDetails",
                "RevokeObject",
                "SessionRequest",
            ]
        );
    }

    #[test]
    fn the_pebc_row_is_the_standards_own() {
        let pebc = Phase::Activated(ControlType::PowerEnvelopeBasedControl);
        assert_eq!(
            names(pebc, Cem),
            alloc::vec![
                "PEBC.Instruction",
                "ReceptionStatus",
                "RevokeObject",
                "SelectControlType",
                "SessionRequest",
            ]
        );
        assert_eq!(
            names(pebc, Rm),
            alloc::vec![
                "InstructionStatusUpdate",
                "PEBC.EnergyConstraint",
                "PEBC.PowerConstraints",
                "PowerForecast",
                "PowerMeasurement",
                "ReceptionStatus",
                "ResourceManagerDetails",
                "RevokeObject",
                "SessionRequest",
            ]
        );
    }

    #[test]
    fn selecting_not_controlable_leaves_the_session_where_it_was() {
        // "NOT_CONTROLABLE" is a real selection, but nothing control-type specific may
        // follow it: docs `learn/examples/nocontrol`.
        assert_eq!(
            Phase::selected(ControlType::NotControllable),
            Phase::Connected
        );
        assert_eq!(Phase::selected(ControlType::NoSelection), Phase::Connected);
        assert_eq!(
            Phase::selected(ControlType::FillRateBasedControl),
            Phase::Activated(ControlType::FillRateBasedControl)
        );
    }

    #[test]
    fn a_control_type_message_in_the_wrong_control_type_is_a_state_error() {
        assert_eq!(
            allowed(
                Phase::Activated(ControlType::PowerEnvelopeBasedControl),
                Cem,
                MessageKind::FrbcInstruction
            ),
            Allowance::WrongState
        );
        assert_eq!(
            allowed(Phase::Connected, Cem, MessageKind::FrbcInstruction),
            Allowance::WrongState
        );
    }

    #[test]
    fn a_message_from_the_wrong_role_is_a_role_error() {
        // An RM does not instruct, and a CEM does not describe itself as a resource.
        // The role is checked before the phase, so it is the answer even where both are
        // wrong: a peer told "not in this state" would try again later for ever.
        assert_eq!(
            allowed(
                Phase::Activated(ControlType::FillRateBasedControl),
                Rm,
                MessageKind::FrbcInstruction
            ),
            Allowance::WrongRole
        );
        assert_eq!(
            allowed(Phase::Connected, Cem, MessageKind::ResourceManagerDetails),
            Allowance::WrongRole
        );
        assert_eq!(
            allowed(Phase::Connected, Rm, MessageKind::SelectControlType),
            Allowance::WrongRole
        );
        assert_eq!(
            allowed(Phase::Negotiating, Cem, MessageKind::ResourceManagerDetails),
            Allowance::WrongRole
        );
    }

    #[test]
    fn revocation_is_accepted_from_both_roles() {
        // E6: the state table lists RevokeObject only for the RM, but RevokableObjects
        // contains every instruction type, which only a CEM sends. Refusing the CEM's
        // revocation would make most of that enum unreachable.
        for role in [Cem, Rm] {
            assert_eq!(
                allowed(
                    Phase::Activated(ControlType::FillRateBasedControl),
                    role,
                    MessageKind::RevokeObject
                ),
                Allowance::Yes
            );
        }
    }

    #[test]
    fn instruction_statuses_need_something_to_be_about() {
        assert_eq!(
            allowed(Phase::Connected, Rm, MessageKind::InstructionStatusUpdate),
            Allowance::WrongState
        );
        assert_eq!(
            allowed(
                Phase::Activated(ControlType::OperationModeBasedControl),
                Rm,
                MessageKind::InstructionStatusUpdate
            ),
            Allowance::Yes
        );
    }

    #[test]
    fn every_message_is_allowed_somewhere() {
        // A message no phase and no role can send would be a hole in the table.
        for kind in MessageKind::ALL {
            let reachable = [
                Phase::Negotiating,
                Phase::Connected,
                Phase::Activated(ControlType::PowerEnvelopeBasedControl),
                Phase::Activated(ControlType::PowerProfileBasedControl),
                Phase::Activated(ControlType::OperationModeBasedControl),
                Phase::Activated(ControlType::FillRateBasedControl),
                Phase::Activated(ControlType::DemandDrivenBasedControl),
            ]
            .into_iter()
            .any(|phase| {
                [Cem, Rm]
                    .into_iter()
                    .any(|role| allowed(phase, role, *kind).is_allowed())
            });
            assert!(reachable, "{kind} can never be sent");
        }
    }
}
