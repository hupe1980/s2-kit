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

/// The control type that is active, if any.
///
/// `None` and `Some(NO_SELECTION)` mean the same thing — the session is in the
/// `WebSocketConnected` state — and so does `Some(NOT_CONTROLABLE)`, which is a legal
/// *selection* after which only measurements and forecasts flow.
#[must_use]
pub fn allowed(
    active: Option<ControlType>,
    sender: EnergyManagementRole,
    kind: MessageKind,
) -> Allowance {
    use MessageKind as K;

    if let Some(expected) = kind.sender()
        && expected != sender
    {
        return Allowance::WrongRole;
    }

    let active = active.filter(|c| c.is_controllable());

    // The arms below are grouped by *reason*, not by outcome, so several share a body on
    // purpose: collapsing them would lose the row of the standard's table each belongs to.
    #[allow(clippy::match_same_arms)]
    match kind {
        // Never state-dependent: either side, at any point.
        K::ReceptionStatus | K::SessionRequest => Allowance::Yes,

        // The handshake belongs to the start of a session. Under S2 Connect it does not
        // happen at all, which is a warning rather than a state error (E9 / S2-STATE-003).
        K::Handshake | K::HandshakeResponse => {
            if active.is_none() {
                Allowance::Yes
            } else {
                Allowance::WrongState
            }
        }

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

/// Every message the role may send in the state, for documentation and tests.
#[must_use]
pub fn allowed_kinds(
    active: Option<ControlType>,
    sender: EnergyManagementRole,
) -> alloc::vec::Vec<MessageKind> {
    MessageKind::ALL
        .iter()
        .copied()
        .filter(|k| allowed(active, sender, *k).is_allowed())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use EnergyManagementRole::{Cem, Rm};
    use alloc::vec::Vec;

    fn names(active: Option<ControlType>, sender: EnergyManagementRole) -> Vec<&'static str> {
        let mut v: Vec<_> = allowed_kinds(active, sender)
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
            names(None, Cem),
            alloc::vec![
                "Handshake",
                "HandshakeResponse",
                "ReceptionStatus",
                "SelectControlType",
                "SessionRequest",
            ]
        );
        assert_eq!(
            names(None, Rm),
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
    fn the_frbc_row_is_the_standards_own() {
        assert_eq!(
            names(Some(ControlType::FillRateBasedControl), Cem),
            alloc::vec![
                "FRBC.Instruction",
                "ReceptionStatus",
                "RevokeObject",
                "SelectControlType",
                "SessionRequest",
            ]
        );
        assert_eq!(
            names(Some(ControlType::FillRateBasedControl), Rm),
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
        assert_eq!(
            names(Some(ControlType::PowerEnvelopeBasedControl), Cem),
            alloc::vec![
                "PEBC.Instruction",
                "ReceptionStatus",
                "RevokeObject",
                "SelectControlType",
                "SessionRequest",
            ]
        );
        assert_eq!(
            names(Some(ControlType::PowerEnvelopeBasedControl), Rm),
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
            names(Some(ControlType::NotControllable), Rm),
            names(None, Rm)
        );
        assert_eq!(names(Some(ControlType::NoSelection), Rm), names(None, Rm));
    }

    #[test]
    fn a_control_type_message_in_the_wrong_control_type_is_a_state_error() {
        assert_eq!(
            allowed(
                Some(ControlType::PowerEnvelopeBasedControl),
                Cem,
                MessageKind::FrbcInstruction
            ),
            Allowance::WrongState
        );
        assert_eq!(
            allowed(None, Cem, MessageKind::FrbcInstruction),
            Allowance::WrongState
        );
    }

    #[test]
    fn a_message_from_the_wrong_role_is_a_role_error() {
        // An RM does not instruct, and a CEM does not describe itself as a resource.
        assert_eq!(
            allowed(
                Some(ControlType::FillRateBasedControl),
                Rm,
                MessageKind::FrbcInstruction
            ),
            Allowance::WrongRole
        );
        assert_eq!(
            allowed(None, Cem, MessageKind::ResourceManagerDetails),
            Allowance::WrongRole
        );
        assert_eq!(
            allowed(None, Rm, MessageKind::SelectControlType),
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
                    Some(ControlType::FillRateBasedControl),
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
            allowed(None, Rm, MessageKind::InstructionStatusUpdate),
            Allowance::WrongState
        );
        assert_eq!(
            allowed(
                Some(ControlType::OperationModeBasedControl),
                Rm,
                MessageKind::InstructionStatusUpdate
            ),
            Allowance::Yes
        );
    }

    #[test]
    fn every_message_is_allowed_somewhere() {
        // A message no state and no role can send would be a hole in the table.
        for kind in MessageKind::ALL {
            let reachable = [
                None,
                Some(ControlType::PowerEnvelopeBasedControl),
                Some(ControlType::PowerProfileBasedControl),
                Some(ControlType::OperationModeBasedControl),
                Some(ControlType::FillRateBasedControl),
                Some(ControlType::DemandDrivenBasedControl),
            ]
            .into_iter()
            .any(|active| {
                [Cem, Rm]
                    .into_iter()
                    .any(|role| allowed(active, role, *kind).is_allowed())
            });
            assert!(reachable, "{kind} can never be sent");
        }
    }
}
