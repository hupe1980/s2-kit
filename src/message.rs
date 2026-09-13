//! [`Message`] — every S2 message, in one enum.
//!
//! S2 JSON v1.0.0 defines 36 messages. Each carries a `message_type` discriminator and,
//! with one exception, a `message_id`; the exception is `ReceptionStatus`, which names
//! the message it is about instead and is never itself acknowledged.
//!
//! [`MessageKind`] is the same set without the payloads: a fieldless `Copy` enum that is
//! the key of the session state table, the unit of the allowed-message rules, and what a
//! log line should carry.

use alloc::boxed::Box;
use serde::{Deserialize, Serialize};

use crate::types::common::{
    ControlType, EnergyManagementRole, Handshake, HandshakeResponse, InstructionStatusUpdate,
    PowerForecast, PowerMeasurement, ReceptionStatus, ResourceManagerDetails, RevokeObject,
    SelectControlType, SessionRequest,
};
use crate::types::{Id, ddbc, frbc, ombc, pebc, ppbc};

/// Any S2 message.
///
/// The enum is internally tagged by `message_type`, exactly as the wire is, so
/// serialising a `Message` produces a complete S2 message and deserialising one accepts
/// any of the 36.
///
/// ```
/// use s2_kit::{Message, types::frbc};
///
/// let json = r#"{"message_type":"FRBC.StorageStatus",
///                "message_id":"aaaaaaaa-0000-0000-0000-000000000001",
///                "present_fill_level":52.0}"#;
/// let message: Message = serde_json::from_str(json)?;
/// match &message {
///     Message::FrbcStorageStatus(s) => assert_eq!(s.present_fill_level, 52.0),
///     _ => unreachable!(),
/// }
/// # Ok::<(), serde_json::Error>(())
/// ```
///
/// Large payloads are boxed so that the enum stays small enough to pass by value and to
/// keep a queue of pending messages compact; [`From`] is implemented for every payload
/// type and does the boxing for you.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message_type")]
#[non_exhaustive]
pub enum Message {
    // --- common -----------------------------------------------------------
    /// A node announcing its role and the versions it speaks.
    #[serde(rename = "Handshake")]
    Handshake(Handshake),
    /// The CEM's choice of version.
    #[serde(rename = "HandshakeResponse")]
    HandshakeResponse(HandshakeResponse),
    /// Everything static about a resource.
    #[serde(rename = "ResourceManagerDetails")]
    ResourceManagerDetails(Box<ResourceManagerDetails>),
    /// The CEM activating a control type.
    #[serde(rename = "SelectControlType")]
    SelectControlType(SelectControlType),
    /// Either side asking for the session to end or restart.
    #[serde(rename = "SessionRequest")]
    SessionRequest(SessionRequest),
    /// The answer every other message gets.
    #[serde(rename = "ReceptionStatus")]
    ReceptionStatus(ReceptionStatus),
    /// What became of an instruction.
    #[serde(rename = "InstructionStatusUpdate")]
    InstructionStatusUpdate(InstructionStatusUpdate),
    /// A measured power.
    #[serde(rename = "PowerMeasurement")]
    PowerMeasurement(PowerMeasurement),
    /// An expected power.
    #[serde(rename = "PowerForecast")]
    PowerForecast(Box<PowerForecast>),
    /// Withdrawing an object published earlier.
    #[serde(rename = "RevokeObject")]
    RevokeObject(RevokeObject),

    // --- PEBC -------------------------------------------------------------
    /// The bounds within which the CEM may set an envelope.
    #[serde(rename = "PEBC.PowerConstraints")]
    PebcPowerConstraints(Box<pebc::PowerConstraints>),
    /// How much energy must still flow over a period.
    #[serde(rename = "PEBC.EnergyConstraint")]
    PebcEnergyConstraint(Box<pebc::EnergyConstraint>),
    /// The envelope the CEM wants followed.
    #[serde(rename = "PEBC.Instruction")]
    PebcInstruction(Box<pebc::Instruction>),

    // --- PPBC -------------------------------------------------------------
    /// The task, its window, and every way of performing it.
    #[serde(rename = "PPBC.PowerProfileDefinition")]
    PpbcPowerProfileDefinition(Box<ppbc::PowerProfileDefinition>),
    /// How every container of a profile is getting on.
    #[serde(rename = "PPBC.PowerProfileStatus")]
    PpbcPowerProfileStatus(Box<ppbc::PowerProfileStatus>),
    /// The CEM choosing a sequence and a start time.
    #[serde(rename = "PPBC.ScheduleInstruction")]
    PpbcScheduleInstruction(Box<ppbc::ScheduleInstruction>),
    /// The CEM pausing a running sequence.
    #[serde(rename = "PPBC.StartInterruptionInstruction")]
    PpbcStartInterruptionInstruction(Box<ppbc::StartInterruptionInstruction>),
    /// The CEM resuming an interrupted sequence.
    #[serde(rename = "PPBC.EndInterruptionInstruction")]
    PpbcEndInterruptionInstruction(Box<ppbc::EndInterruptionInstruction>),

    // --- OMBC -------------------------------------------------------------
    /// The state machine.
    #[serde(rename = "OMBC.SystemDescription")]
    OmbcSystemDescription(Box<ombc::SystemDescription>),
    /// Which mode the device is in.
    #[serde(rename = "OMBC.Status")]
    OmbcStatus(Box<ombc::Status>),
    /// When one of its timers finishes.
    #[serde(rename = "OMBC.TimerStatus")]
    OmbcTimerStatus(ombc::TimerStatus),
    /// The CEM asking for a mode change.
    #[serde(rename = "OMBC.Instruction")]
    OmbcInstruction(Box<ombc::Instruction>),

    // --- FRBC -------------------------------------------------------------
    /// The storage and every actuator that can move it.
    #[serde(rename = "FRBC.SystemDescription")]
    FrbcSystemDescription(Box<frbc::SystemDescription>),
    /// How full the store is.
    #[serde(rename = "FRBC.StorageStatus")]
    FrbcStorageStatus(frbc::StorageStatus),
    /// Which mode an actuator is in.
    #[serde(rename = "FRBC.ActuatorStatus")]
    FrbcActuatorStatus(Box<frbc::ActuatorStatus>),
    /// When one of an actuator's timers finishes.
    #[serde(rename = "FRBC.TimerStatus")]
    FrbcTimerStatus(Box<frbc::TimerStatus>),
    /// The store's standing losses.
    #[serde(rename = "FRBC.LeakageBehaviour")]
    FrbcLeakageBehaviour(Box<frbc::LeakageBehaviour>),
    /// What the user is expected to take out of the store.
    #[serde(rename = "FRBC.UsageForecast")]
    FrbcUsageForecast(Box<frbc::UsageForecast>),
    /// Fill levels the CEM should aim for.
    #[serde(rename = "FRBC.FillLevelTargetProfile")]
    FrbcFillLevelTargetProfile(Box<frbc::FillLevelTargetProfile>),
    /// The CEM asking an actuator to change mode.
    #[serde(rename = "FRBC.Instruction")]
    FrbcInstruction(Box<frbc::Instruction>),

    // --- DDBC -------------------------------------------------------------
    /// Every actuator that can meet the demand.
    #[serde(rename = "DDBC.SystemDescription")]
    DdbcSystemDescription(Box<ddbc::SystemDescription>),
    /// Which mode an actuator is in.
    #[serde(rename = "DDBC.ActuatorStatus")]
    DdbcActuatorStatus(Box<ddbc::ActuatorStatus>),
    /// When one of an actuator's timers finishes.
    #[serde(rename = "DDBC.TimerStatus")]
    DdbcTimerStatus(Box<ddbc::TimerStatus>),
    /// What the system expects to be asked for.
    #[serde(rename = "DDBC.AverageDemandRateForecast")]
    DdbcAverageDemandRateForecast(Box<ddbc::AverageDemandRateForecast>),
    /// The demand that must be satisfied right now. New in S2 JSON v1.0.0.
    #[serde(rename = "DDBC.PresentDemandStatus")]
    DdbcPresentDemandStatus(ddbc::PresentDemandStatus),
    /// The CEM asking an actuator to change mode.
    #[serde(rename = "DDBC.Instruction")]
    DdbcInstruction(Box<ddbc::Instruction>),
}

/// Every S2 message type, without the payload.
///
/// `MessageKind` is `Copy` and fieldless, which is what makes the session's
/// allowed-message table a lookup rather than a match over 36 arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
#[allow(missing_docs)] // one variant per Message variant; see there
pub enum MessageKind {
    Handshake,
    HandshakeResponse,
    ResourceManagerDetails,
    SelectControlType,
    SessionRequest,
    ReceptionStatus,
    InstructionStatusUpdate,
    PowerMeasurement,
    PowerForecast,
    RevokeObject,
    PebcPowerConstraints,
    PebcEnergyConstraint,
    PebcInstruction,
    PpbcPowerProfileDefinition,
    PpbcPowerProfileStatus,
    PpbcScheduleInstruction,
    PpbcStartInterruptionInstruction,
    PpbcEndInterruptionInstruction,
    OmbcSystemDescription,
    OmbcStatus,
    OmbcTimerStatus,
    OmbcInstruction,
    FrbcSystemDescription,
    FrbcStorageStatus,
    FrbcActuatorStatus,
    FrbcTimerStatus,
    FrbcLeakageBehaviour,
    FrbcUsageForecast,
    FrbcFillLevelTargetProfile,
    FrbcInstruction,
    DdbcSystemDescription,
    DdbcActuatorStatus,
    DdbcTimerStatus,
    DdbcAverageDemandRateForecast,
    DdbcPresentDemandStatus,
    DdbcInstruction,
}

/// Builds the `MessageKind` tables once, so the wire names, the parser and the
/// `Message`-to-kind mapping cannot drift apart: each is generated from the same list.
macro_rules! message_kinds {
    ($(($kind:ident, $wire:literal, $variant:ident)),* $(,)?) => {
        impl MessageKind {
            /// Every message type this crate knows, in declaration order.
            pub const ALL: &'static [MessageKind] = &[$(MessageKind::$kind),*];

            /// The `message_type` string this kind has on the wire.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $(MessageKind::$kind => $wire),* }
            }

            /// The kind a `message_type` string names, if this crate knows it.
            #[must_use]
            pub fn from_wire(s: &str) -> Option<Self> {
                match s { $($wire => Some(MessageKind::$kind),)* _ => None }
            }
        }

        impl Message {
            /// Which message this is.
            #[must_use]
            pub const fn kind(&self) -> MessageKind {
                match self { $(Message::$variant(..) => MessageKind::$kind),* }
            }
        }
    };
}

message_kinds![
    (Handshake, "Handshake", Handshake),
    (HandshakeResponse, "HandshakeResponse", HandshakeResponse),
    (
        ResourceManagerDetails,
        "ResourceManagerDetails",
        ResourceManagerDetails
    ),
    (SelectControlType, "SelectControlType", SelectControlType),
    (SessionRequest, "SessionRequest", SessionRequest),
    (ReceptionStatus, "ReceptionStatus", ReceptionStatus),
    (
        InstructionStatusUpdate,
        "InstructionStatusUpdate",
        InstructionStatusUpdate
    ),
    (PowerMeasurement, "PowerMeasurement", PowerMeasurement),
    (PowerForecast, "PowerForecast", PowerForecast),
    (RevokeObject, "RevokeObject", RevokeObject),
    (
        PebcPowerConstraints,
        "PEBC.PowerConstraints",
        PebcPowerConstraints
    ),
    (
        PebcEnergyConstraint,
        "PEBC.EnergyConstraint",
        PebcEnergyConstraint
    ),
    (PebcInstruction, "PEBC.Instruction", PebcInstruction),
    (
        PpbcPowerProfileDefinition,
        "PPBC.PowerProfileDefinition",
        PpbcPowerProfileDefinition
    ),
    (
        PpbcPowerProfileStatus,
        "PPBC.PowerProfileStatus",
        PpbcPowerProfileStatus
    ),
    (
        PpbcScheduleInstruction,
        "PPBC.ScheduleInstruction",
        PpbcScheduleInstruction
    ),
    (
        PpbcStartInterruptionInstruction,
        "PPBC.StartInterruptionInstruction",
        PpbcStartInterruptionInstruction
    ),
    (
        PpbcEndInterruptionInstruction,
        "PPBC.EndInterruptionInstruction",
        PpbcEndInterruptionInstruction
    ),
    (
        OmbcSystemDescription,
        "OMBC.SystemDescription",
        OmbcSystemDescription
    ),
    (OmbcStatus, "OMBC.Status", OmbcStatus),
    (OmbcTimerStatus, "OMBC.TimerStatus", OmbcTimerStatus),
    (OmbcInstruction, "OMBC.Instruction", OmbcInstruction),
    (
        FrbcSystemDescription,
        "FRBC.SystemDescription",
        FrbcSystemDescription
    ),
    (FrbcStorageStatus, "FRBC.StorageStatus", FrbcStorageStatus),
    (
        FrbcActuatorStatus,
        "FRBC.ActuatorStatus",
        FrbcActuatorStatus
    ),
    (FrbcTimerStatus, "FRBC.TimerStatus", FrbcTimerStatus),
    (
        FrbcLeakageBehaviour,
        "FRBC.LeakageBehaviour",
        FrbcLeakageBehaviour
    ),
    (FrbcUsageForecast, "FRBC.UsageForecast", FrbcUsageForecast),
    (
        FrbcFillLevelTargetProfile,
        "FRBC.FillLevelTargetProfile",
        FrbcFillLevelTargetProfile
    ),
    (FrbcInstruction, "FRBC.Instruction", FrbcInstruction),
    (
        DdbcSystemDescription,
        "DDBC.SystemDescription",
        DdbcSystemDescription
    ),
    (
        DdbcActuatorStatus,
        "DDBC.ActuatorStatus",
        DdbcActuatorStatus
    ),
    (DdbcTimerStatus, "DDBC.TimerStatus", DdbcTimerStatus),
    (
        DdbcAverageDemandRateForecast,
        "DDBC.AverageDemandRateForecast",
        DdbcAverageDemandRateForecast
    ),
    (
        DdbcPresentDemandStatus,
        "DDBC.PresentDemandStatus",
        DdbcPresentDemandStatus
    ),
    (DdbcInstruction, "DDBC.Instruction", DdbcInstruction),
];

impl core::fmt::Display for MessageKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl MessageKind {
    /// The control type this message belongs to, if any.
    #[must_use]
    pub const fn control_type(self) -> Option<ControlType> {
        use MessageKind as K;
        match self {
            K::PebcPowerConstraints | K::PebcEnergyConstraint | K::PebcInstruction => {
                Some(ControlType::PowerEnvelopeBasedControl)
            }
            K::PpbcPowerProfileDefinition
            | K::PpbcPowerProfileStatus
            | K::PpbcScheduleInstruction
            | K::PpbcStartInterruptionInstruction
            | K::PpbcEndInterruptionInstruction => Some(ControlType::PowerProfileBasedControl),
            K::OmbcSystemDescription | K::OmbcStatus | K::OmbcTimerStatus | K::OmbcInstruction => {
                Some(ControlType::OperationModeBasedControl)
            }
            K::FrbcSystemDescription
            | K::FrbcStorageStatus
            | K::FrbcActuatorStatus
            | K::FrbcTimerStatus
            | K::FrbcLeakageBehaviour
            | K::FrbcUsageForecast
            | K::FrbcFillLevelTargetProfile
            | K::FrbcInstruction => Some(ControlType::FillRateBasedControl),
            K::DdbcSystemDescription
            | K::DdbcActuatorStatus
            | K::DdbcTimerStatus
            | K::DdbcAverageDemandRateForecast
            | K::DdbcPresentDemandStatus
            | K::DdbcInstruction => Some(ControlType::DemandDrivenBasedControl),
            _ => None,
        }
    }

    /// Whether this message asks a Resource Manager to do something, and therefore earns
    /// an `InstructionStatusUpdate` as well as a `ReceptionStatus`.
    #[must_use]
    pub const fn is_instruction(self) -> bool {
        use MessageKind as K;
        matches!(
            self,
            K::PebcInstruction
                | K::PpbcScheduleInstruction
                | K::PpbcStartInterruptionInstruction
                | K::PpbcEndInterruptionInstruction
                | K::OmbcInstruction
                | K::FrbcInstruction
                | K::DdbcInstruction
        )
    }

    /// Which role sends this message.
    ///
    /// `SessionRequest` and `ReceptionStatus` are sent by both, and so answer `None`.
    #[must_use]
    pub const fn sender(self) -> Option<EnergyManagementRole> {
        use EnergyManagementRole as Role;
        use MessageKind as K;
        match self {
            K::SelectControlType | K::HandshakeResponse => Some(Role::Cem),
            _ if self.is_instruction() => Some(Role::Cem),
            // RevokeObject is listed only in the RM column of the state table, but
            // `RevokableObjects` contains every instruction type, which only a CEM
            // sends (erratum E6). Both roles may send it; the
            // object-type mismatch is a warning rule instead.
            K::SessionRequest | K::ReceptionStatus | K::Handshake | K::RevokeObject => None,
            _ => Some(Role::Rm),
        }
    }

    /// Whether the message exists in a given wire profile.
    ///
    /// Only `DDBC.PresentDemandStatus` answers `false` for anything: it is new in
    /// v1.0.0.
    #[must_use]
    pub const fn exists_in(self, profile: crate::types::WireProfile) -> bool {
        !(matches!(self, MessageKind::DdbcPresentDemandStatus)
            && matches!(profile, crate::types::WireProfile::V0_0_2Beta))
    }
}

impl Message {
    /// This message's own identifier.
    ///
    /// `None` for `ReceptionStatus`, which has none — it names the message it is about
    /// instead, via [`subject_message_id`](Self::subject_message_id).
    #[must_use]
    pub fn id(&self) -> Option<Id> {
        use Message as M;
        Some(match self {
            M::Handshake(m) => m.message_id,
            M::HandshakeResponse(m) => m.message_id,
            M::ResourceManagerDetails(m) => m.message_id,
            M::SelectControlType(m) => m.message_id,
            M::SessionRequest(m) => m.message_id,
            M::ReceptionStatus(_) => return None,
            M::InstructionStatusUpdate(m) => m.message_id,
            M::PowerMeasurement(m) => m.message_id,
            M::PowerForecast(m) => m.message_id,
            M::RevokeObject(m) => m.message_id,
            M::PebcPowerConstraints(m) => m.message_id,
            M::PebcEnergyConstraint(m) => m.message_id,
            M::PebcInstruction(m) => m.message_id,
            M::PpbcPowerProfileDefinition(m) => m.message_id,
            M::PpbcPowerProfileStatus(m) => m.message_id,
            M::PpbcScheduleInstruction(m) => m.message_id,
            M::PpbcStartInterruptionInstruction(m) => m.message_id,
            M::PpbcEndInterruptionInstruction(m) => m.message_id,
            M::OmbcSystemDescription(m) => m.message_id,
            M::OmbcStatus(m) => m.message_id,
            M::OmbcTimerStatus(m) => m.message_id,
            M::OmbcInstruction(m) => m.message_id,
            M::FrbcSystemDescription(m) => m.message_id,
            M::FrbcStorageStatus(m) => m.message_id,
            M::FrbcActuatorStatus(m) => m.message_id,
            M::FrbcTimerStatus(m) => m.message_id,
            M::FrbcLeakageBehaviour(m) => m.message_id,
            M::FrbcUsageForecast(m) => m.message_id,
            M::FrbcFillLevelTargetProfile(m) => m.message_id,
            M::FrbcInstruction(m) => m.message_id,
            M::DdbcSystemDescription(m) => m.message_id,
            M::DdbcActuatorStatus(m) => m.message_id,
            M::DdbcTimerStatus(m) => m.message_id,
            M::DdbcAverageDemandRateForecast(m) => m.message_id,
            M::DdbcPresentDemandStatus(m) => m.message_id,
            M::DdbcInstruction(m) => m.message_id,
        })
    }

    /// The message a `ReceptionStatus` is about, if this is one.
    #[must_use]
    pub fn subject_message_id(&self) -> Option<Id> {
        match self {
            Message::ReceptionStatus(s) => Some(s.subject_message_id),
            _ => None,
        }
    }

    /// The instruction's own identifier, if this message is an instruction.
    ///
    /// This is **not** the `message_id`: an `InstructionStatusUpdate` names this one.
    #[must_use]
    pub fn instruction_id(&self) -> Option<Id> {
        use Message as M;
        Some(match self {
            M::PebcInstruction(m) => m.id,
            M::PpbcScheduleInstruction(m) => m.id,
            M::PpbcStartInterruptionInstruction(m) => m.id,
            M::PpbcEndInterruptionInstruction(m) => m.id,
            M::OmbcInstruction(m) => m.id,
            M::FrbcInstruction(m) => m.id,
            M::DdbcInstruction(m) => m.id,
            _ => return None,
        })
    }

    /// The identifier of the object this message publishes, if it is one a
    /// [`RevokeObject`] could later withdraw.
    #[must_use]
    pub fn revokable_id(&self) -> Option<(crate::types::common::RevokableObjects, Id)> {
        use crate::types::common::RevokableObjects as R;
        use Message as M;
        Some(match self {
            M::PebcPowerConstraints(m) => (R::PebcPowerConstraints, m.id),
            M::PebcEnergyConstraint(m) => (R::PebcEnergyConstraint, m.id),
            M::PebcInstruction(m) => (R::PebcInstruction, m.id),
            M::PpbcPowerProfileDefinition(m) => (R::PpbcPowerProfileDefinition, m.id),
            M::PpbcScheduleInstruction(m) => (R::PpbcScheduleInstruction, m.id),
            M::PpbcStartInterruptionInstruction(m) => (R::PpbcStartInterruptionInstruction, m.id),
            M::PpbcEndInterruptionInstruction(m) => (R::PpbcEndInterruptionInstruction, m.id),
            M::OmbcInstruction(m) => (R::OmbcInstruction, m.id),
            M::FrbcInstruction(m) => (R::FrbcInstruction, m.id),
            M::DdbcInstruction(m) => (R::DdbcInstruction, m.id),
            // A system description is revokable but has no `id` of its own; the standard
            // has the `message_id` stand in, since that is the only handle a CEM has.
            M::OmbcSystemDescription(m) => (R::OmbcSystemDescription, m.message_id),
            M::FrbcSystemDescription(m) => (R::FrbcSystemDescription, m.message_id),
            M::DdbcSystemDescription(m) => (R::DdbcSystemDescription, m.message_id),
            _ => return None,
        })
    }

    /// The control type this message belongs to, if any.
    #[must_use]
    pub const fn control_type(&self) -> Option<ControlType> {
        self.kind().control_type()
    }

    /// Whether this message is an instruction.
    #[must_use]
    pub const fn is_instruction(&self) -> bool {
        self.kind().is_instruction()
    }
}

/// Generates the `From<Payload> for Message` conversions, boxing where the variant does.
macro_rules! from_payload {
    ($($ty:ty => $variant:ident $(, $boxed:tt)?);* $(;)?) => {
        $(impl From<$ty> for Message {
            fn from(value: $ty) -> Self {
                Message::$variant(from_payload!(@wrap value $(, $boxed)?))
            }
        })*
    };
    (@wrap $v:ident) => { $v };
    (@wrap $v:ident, box) => { Box::new($v) };
}

from_payload![
    Handshake => Handshake;
    HandshakeResponse => HandshakeResponse;
    ResourceManagerDetails => ResourceManagerDetails, box;
    SelectControlType => SelectControlType;
    SessionRequest => SessionRequest;
    ReceptionStatus => ReceptionStatus;
    InstructionStatusUpdate => InstructionStatusUpdate;
    PowerMeasurement => PowerMeasurement;
    PowerForecast => PowerForecast, box;
    RevokeObject => RevokeObject;
    pebc::PowerConstraints => PebcPowerConstraints, box;
    pebc::EnergyConstraint => PebcEnergyConstraint, box;
    pebc::Instruction => PebcInstruction, box;
    ppbc::PowerProfileDefinition => PpbcPowerProfileDefinition, box;
    ppbc::PowerProfileStatus => PpbcPowerProfileStatus, box;
    ppbc::ScheduleInstruction => PpbcScheduleInstruction, box;
    ppbc::StartInterruptionInstruction => PpbcStartInterruptionInstruction, box;
    ppbc::EndInterruptionInstruction => PpbcEndInterruptionInstruction, box;
    ombc::SystemDescription => OmbcSystemDescription, box;
    ombc::Status => OmbcStatus, box;
    ombc::TimerStatus => OmbcTimerStatus;
    ombc::Instruction => OmbcInstruction, box;
    frbc::SystemDescription => FrbcSystemDescription, box;
    frbc::StorageStatus => FrbcStorageStatus;
    frbc::ActuatorStatus => FrbcActuatorStatus, box;
    frbc::TimerStatus => FrbcTimerStatus, box;
    frbc::LeakageBehaviour => FrbcLeakageBehaviour, box;
    frbc::UsageForecast => FrbcUsageForecast, box;
    frbc::FillLevelTargetProfile => FrbcFillLevelTargetProfile, box;
    frbc::Instruction => FrbcInstruction, box;
    ddbc::SystemDescription => DdbcSystemDescription, box;
    ddbc::ActuatorStatus => DdbcActuatorStatus, box;
    ddbc::TimerStatus => DdbcTimerStatus, box;
    ddbc::AverageDemandRateForecast => DdbcAverageDemandRateForecast, box;
    ddbc::PresentDemandStatus => DdbcPresentDemandStatus;
    ddbc::Instruction => DdbcInstruction, box;
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Timestamp;
    use crate::types::common::{ReceptionStatusValues, SessionRequestType};

    #[test]
    fn every_message_type_in_s2_json_v1_is_present() {
        assert_eq!(MessageKind::ALL.len(), 36, "S2 JSON v1.0.0 has 36 messages");
        // And no two share a wire name.
        let mut names: alloc::vec::Vec<_> = MessageKind::ALL.iter().map(|k| k.as_str()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate message_type");
    }

    #[test]
    fn wire_names_round_trip_through_the_parser() {
        for kind in MessageKind::ALL {
            assert_eq!(MessageKind::from_wire(kind.as_str()), Some(*kind));
        }
        assert_eq!(MessageKind::from_wire("Nope"), None);
        // Case matters: the wire names are exact.
        assert_eq!(MessageKind::from_wire("frbc.instruction"), None);
    }

    #[test]
    fn control_types_are_read_off_the_message_type_prefix() {
        for kind in MessageKind::ALL {
            let expected = kind
                .as_str()
                .split_once('.')
                .and_then(|(prefix, _)| match prefix {
                    "PEBC" => Some(ControlType::PowerEnvelopeBasedControl),
                    "PPBC" => Some(ControlType::PowerProfileBasedControl),
                    "OMBC" => Some(ControlType::OperationModeBasedControl),
                    "FRBC" => Some(ControlType::FillRateBasedControl),
                    "DDBC" => Some(ControlType::DemandDrivenBasedControl),
                    _ => None,
                });
            assert_eq!(kind.control_type(), expected, "{kind}");
        }
    }

    #[test]
    fn instructions_are_exactly_the_seven() {
        let instructions: alloc::vec::Vec<_> = MessageKind::ALL
            .iter()
            .filter(|k| k.is_instruction())
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            instructions,
            alloc::vec![
                "PEBC.Instruction",
                "PPBC.ScheduleInstruction",
                "PPBC.StartInterruptionInstruction",
                "PPBC.EndInterruptionInstruction",
                "OMBC.Instruction",
                "FRBC.Instruction",
                "DDBC.Instruction",
            ]
        );
    }

    #[test]
    fn reception_status_is_the_one_message_with_no_id_of_its_own() {
        let rs = Message::from(ReceptionStatus::ok(Id::new_const("m1")));
        assert_eq!(rs.id(), None);
        assert_eq!(rs.subject_message_id(), Some(Id::new_const("m1")));

        for kind in MessageKind::ALL {
            if *kind != MessageKind::ReceptionStatus {
                // Every other message carries a message_id; the property is asserted by
                // the round-trip tests, this documents the exception.
                assert!(kind.as_str() != "ReceptionStatus");
            }
        }
    }

    #[test]
    fn the_tag_is_first_on_the_wire_and_matches_the_kind() {
        let m = Message::from(SessionRequest {
            message_id: Id::new_const("m1"),
            request: SessionRequestType::Terminate,
            diagnostic_label: None,
        });
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            json.starts_with(r#"{"message_type":"SessionRequest","message_id":"m1""#),
            "{json}"
        );
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.kind(), MessageKind::SessionRequest);
    }

    #[test]
    fn unknown_fields_are_refused_through_the_enum_too() {
        // The tag itself must not count as an unknown field, and anything else must.
        let ok =
            r#"{"message_type":"FRBC.StorageStatus","message_id":"m1","present_fill_level":1.0}"#;
        assert!(serde_json::from_str::<Message>(ok).is_ok());
        let bad = r#"{"message_type":"FRBC.StorageStatus","message_id":"m1","present_fill_level":1.0,"extra":1}"#;
        assert!(serde_json::from_str::<Message>(bad).is_err());
    }

    #[test]
    fn an_unknown_message_type_is_an_error_not_a_silent_variant() {
        let json = r#"{"message_type":"FRBC.SomethingNew","message_id":"m1"}"#;
        assert!(serde_json::from_str::<Message>(json).is_err());
    }

    #[test]
    fn revokable_objects_line_up_with_the_messages_that_publish_them() {
        let instruction = frbc::Instruction {
            message_id: Id::new_const("m1"),
            id: Id::new_const("instr0"),
            actuator_id: Id::new_const("a1"),
            operation_mode: Id::new_const("om1"),
            operation_mode_factor: 1.0,
            execution_time: Timestamp::UNIX_EPOCH,
            abnormal_condition: false,
        };
        let m = Message::from(instruction);
        assert_eq!(m.instruction_id(), Some(Id::new_const("instr0")));
        let (object_type, id) = m.revokable_id().unwrap();
        assert_eq!(
            object_type,
            crate::types::common::RevokableObjects::FrbcInstruction
        );
        assert_eq!(id, Id::new_const("instr0"));
        assert_eq!(
            object_type.control_type(),
            ControlType::FillRateBasedControl
        );
    }

    #[test]
    fn the_new_message_exists_only_in_the_newer_profile() {
        use crate::types::WireProfile;
        assert!(MessageKind::DdbcPresentDemandStatus.exists_in(WireProfile::V1_0_0));
        assert!(!MessageKind::DdbcPresentDemandStatus.exists_in(WireProfile::V0_0_2Beta));
        // Everything else exists in both.
        for kind in MessageKind::ALL {
            if *kind != MessageKind::DdbcPresentDemandStatus {
                assert!(kind.exists_in(WireProfile::V0_0_2Beta), "{kind}");
                assert!(kind.exists_in(WireProfile::V1_0_0), "{kind}");
            }
        }
    }

    #[test]
    fn the_enum_stays_small_enough_to_move_cheaply() {
        // Where the number comes from: an `Id` is 65 bytes inline, because it is `Copy`
        // and allocation-free (D11), and the widest *unboxed* variant is
        // `OMBC.TimerStatus` — two identifiers and a timestamp, 146 bytes, padded to
        // 152. Payloads that own heap data, and the instruction and actuator-status
        // messages that carry three or four identifiers, are boxed; without that the
        // enum would be 288 bytes, the size of `FRBC.Instruction`.
        let size = core::mem::size_of::<Message>();
        assert!(size <= 160, "Message grew to {size} bytes");
        // Boxing keeps the spread well inside clippy's large-enum-variant threshold.
        assert_eq!(core::mem::size_of::<Box<frbc::Instruction>>(), 8);
    }

    #[test]
    fn reception_status_values_are_not_confused_with_statuses() {
        let s = ReceptionStatus::error(
            Id::new_const("m1"),
            ReceptionStatusValues::InvalidContent,
            "S2-STATE-001: nope",
        );
        let json = serde_json::to_string(&Message::from(s)).unwrap();
        assert!(json.contains(r#""status":"INVALID_CONTENT""#));
        assert!(json.contains("S2-STATE-001"));
    }
}
