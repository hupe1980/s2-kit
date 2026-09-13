//! Types common to every control type, and the ten messages that are not part of one.
//!
//! Measurements and forecasts are deliberately outside the control types: docs
//! the S2 documentation, *Control types* — "Sending measurements and sending forecasts to the CEM is
//! not part of any of the Control Types. This functionality can always be used, even if
//! no Control Type has been activated by the CEM."

use alloc::string::String;
use alloc::vec::Vec;

use bon::Builder;
use serde::{Deserialize, Serialize};

use super::{Duration, Id, ProtocolVersion, Timestamp};

// ---------------------------------------------------------------------------
// Enumerations
// ---------------------------------------------------------------------------

/// A form of energy a resource exchanges with the grid.
///
/// Only energy exchanged *with the grid* counts — never a device's internal flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[non_exhaustive]
pub enum Commodity {
    /// Natural gas.
    Gas,
    /// Heat.
    Heat,
    /// Electricity.
    Electricity,
    /// Heating oil.
    Oil,
}

/// A commodity combined with the quantity and unit its power is expressed in, and — for
/// electricity — the phase.
///
/// The unit is **not** always watts: [`unit`](Self::unit) is what a diagnostic should use.
///
/// Note that `HEAT.TEMPERATURE` is in this list although a temperature is not a power.
/// The standard puts it here; the model follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CommodityQuantity {
    /// Electric power in watts on phase 1. A single-phase device always uses L1.
    #[serde(rename = "ELECTRIC.POWER.L1")]
    ElectricPowerL1,
    /// Electric power in watts on phase 2. Three-phase devices only.
    #[serde(rename = "ELECTRIC.POWER.L2")]
    ElectricPowerL2,
    /// Electric power in watts on phase 3. Three-phase devices only.
    #[serde(rename = "ELECTRIC.POWER.L3")]
    ElectricPowerL3,
    /// Electric power in watts, shared equally across three phases.
    #[serde(rename = "ELECTRIC.POWER.3_PHASE_SYMMETRIC")]
    ElectricPower3PhaseSymmetric,
    /// Gas flow rate in litres per second.
    #[serde(rename = "NATURAL_GAS.FLOW_RATE")]
    NaturalGasFlowRate,
    /// Hydrogen flow rate in grams per second.
    #[serde(rename = "HYDROGEN.FLOW_RATE")]
    HydrogenFlowRate,
    /// Heat in degrees Celsius.
    #[serde(rename = "HEAT.TEMPERATURE")]
    HeatTemperature,
    /// Flow rate of a heat-carrying gas or liquid, in litres per second.
    #[serde(rename = "HEAT.FLOW_RATE")]
    HeatFlowRate,
    /// Thermal power in watts.
    #[serde(rename = "HEAT.THERMAL_POWER")]
    HeatThermalPower,
    /// Oil flow rate in litres per hour.
    #[serde(rename = "OIL.FLOW_RATE")]
    OilFlowRate,
}

impl CommodityQuantity {
    /// The commodity this quantity measures.
    #[must_use]
    pub const fn commodity(self) -> Commodity {
        match self {
            Self::ElectricPowerL1
            | Self::ElectricPowerL2
            | Self::ElectricPowerL3
            | Self::ElectricPower3PhaseSymmetric => Commodity::Electricity,
            Self::NaturalGasFlowRate | Self::HydrogenFlowRate => Commodity::Gas,
            Self::HeatTemperature | Self::HeatFlowRate | Self::HeatThermalPower => Commodity::Heat,
            Self::OilFlowRate => Commodity::Oil,
        }
    }

    /// Whether this is one of the per-phase electric quantities.
    #[must_use]
    pub const fn is_per_phase_electric(self) -> bool {
        matches!(
            self,
            Self::ElectricPowerL1 | Self::ElectricPowerL2 | Self::ElectricPowerL3
        )
    }

    /// Whether this quantity is a power at all.
    ///
    /// `HEAT.TEMPERATURE` is the one that is not, and treating it as one is a mistake
    /// the enum cannot otherwise stop.
    #[must_use]
    pub const fn is_power(self) -> bool {
        !matches!(self, Self::HeatTemperature)
    }

    /// The unit a value of this quantity is expressed in.
    ///
    /// Four of the ten are not watts (`S2J schemas/CommodityQuantity`: litres per second,
    /// grams per second, degrees Celsius, litres per hour), so any log line, diagnostic or
    /// user interface that appends `"W"` to a `PowerValue` is wrong four times out of ten
    /// — including for the one quantity that is not a power at all.
    ///
    /// ```
    /// use s2_kit::types::common::CommodityQuantity;
    ///
    /// assert_eq!(CommodityQuantity::ElectricPowerL1.unit(), "W");
    /// assert_eq!(CommodityQuantity::HeatTemperature.unit(), "°C");
    /// assert_eq!(CommodityQuantity::OilFlowRate.unit(), "l/h");
    /// ```
    #[must_use]
    pub const fn unit(self) -> &'static str {
        match self {
            Self::ElectricPowerL1
            | Self::ElectricPowerL2
            | Self::ElectricPowerL3
            | Self::ElectricPower3PhaseSymmetric
            | Self::HeatThermalPower => "W",
            Self::NaturalGasFlowRate | Self::HeatFlowRate => "l/s",
            Self::HydrogenFlowRate => "g/s",
            Self::HeatTemperature => "°C",
            Self::OilFlowRate => "l/h",
        }
    }

    /// The wire spelling, for a diagnostic that should say what the peer said.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ElectricPowerL1 => "ELECTRIC.POWER.L1",
            Self::ElectricPowerL2 => "ELECTRIC.POWER.L2",
            Self::ElectricPowerL3 => "ELECTRIC.POWER.L3",
            Self::ElectricPower3PhaseSymmetric => "ELECTRIC.POWER.3_PHASE_SYMMETRIC",
            Self::NaturalGasFlowRate => "NATURAL_GAS.FLOW_RATE",
            Self::HydrogenFlowRate => "HYDROGEN.FLOW_RATE",
            Self::HeatTemperature => "HEAT.TEMPERATURE",
            Self::HeatFlowRate => "HEAT.FLOW_RATE",
            Self::HeatThermalPower => "HEAT.THERMAL_POWER",
            Self::OilFlowRate => "OIL.FLOW_RATE",
        }
    }
}

/// The way a Resource Manager expresses its flexibility.
///
/// Five control types cover the eight flexibility patterns the standard identifies, plus
/// two values that are not control types at all: `NOT_CONTROLABLE`, a legal *selection*
/// after which only measurements and forecasts flow, and `NO_SELECTION`, the deactivated
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ControlType {
    /// Power Envelope Based Control — a device that cannot be controlled but can be
    /// bounded. Curtailable PV, a wallbox that only curtails.
    PowerEnvelopeBasedControl,
    /// Power Profile Based Control — a device with a task to perform that is flexible
    /// about *when*. A washing machine, a dishwasher.
    PowerProfileBasedControl,
    /// Operation Mode Based Control — a device that can adjust its power with no
    /// constraint on how long the adjustment lasts. A generator.
    OperationModeBasedControl,
    /// Fill Rate Based Control — anything that stores or buffers energy. A battery, an
    /// EV, a hot-water tank, a fridge.
    FillRateBasedControl,
    /// Demand Driven Based Control — a device that must meet a demand but is flexible in
    /// how. A hybrid heat pump.
    DemandDrivenBasedControl,
    /// No control is possible. Measurements and forecasts may still be sent.
    ///
    /// The wire spelling has one `l`. The Rust name does not inherit the typo; the
    /// `serde` rename carries the bytes (erratum E15).
    #[serde(rename = "NOT_CONTROLABLE")]
    NotControllable,
    /// No control type is, or has been, selected.
    NoSelection,
}

impl ControlType {
    /// Whether this is one of the five real control types, as opposed to
    /// `NOT_CONTROLABLE` or `NO_SELECTION`.
    #[must_use]
    pub const fn is_controllable(self) -> bool {
        matches!(
            self,
            Self::PowerEnvelopeBasedControl
                | Self::PowerProfileBasedControl
                | Self::OperationModeBasedControl
                | Self::FillRateBasedControl
                | Self::DemandDrivenBasedControl
        )
    }

    /// Whether the control type is built on operation modes, transitions and timers.
    ///
    /// the S2 documentation, *Operation modes*: OMBC, FRBC and DDBC share that machinery; PEBC
    /// and PPBC "each have their own approach".
    #[must_use]
    pub const fn uses_operation_modes(self) -> bool {
        matches!(
            self,
            Self::OperationModeBasedControl
                | Self::FillRateBasedControl
                | Self::DemandDrivenBasedControl
        )
    }

    /// The short name used in message types: `"FRBC"`, `"PEBC"`, and so on.
    #[must_use]
    pub const fn abbreviation(self) -> Option<&'static str> {
        match self {
            Self::PowerEnvelopeBasedControl => Some("PEBC"),
            Self::PowerProfileBasedControl => Some("PPBC"),
            Self::OperationModeBasedControl => Some("OMBC"),
            Self::FillRateBasedControl => Some("FRBC"),
            Self::DemandDrivenBasedControl => Some("DDBC"),
            Self::NotControllable | Self::NoSelection => None,
        }
    }
}

/// The currency cost information is expressed in.
///
/// `S2J messages/ResourceManagerDetails.currency` is "Mandatory if cost information is
/// published" — running costs and transition costs are in this currency per second, and
/// never include the price of the commodity itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[non_exhaustive]
#[allow(missing_docs)] // ISO 4217 codes; the name is the documentation
pub enum Currency {
    Aed,
    Ang,
    Aud,
    Che,
    Chf,
    Chw,
    Eur,
    Gbp,
    Lbp,
    Lkr,
    Lrd,
    Lsl,
    Lyd,
    Mad,
    Mdl,
    Mga,
    Mkd,
    Mmk,
    Mnt,
    Mop,
    Mro,
    Mur,
    Mvr,
    Mwk,
    Mxn,
    Mxv,
    Myr,
    Mzn,
    Nad,
    Ngn,
    Nio,
    Nok,
    Npr,
    Nzd,
    Omr,
    Pab,
    Pen,
    Pgk,
    Php,
    Pkr,
    Pln,
    Pyg,
    Qar,
    Ron,
    Rsd,
    Rub,
    Rwf,
    Sar,
    Sbd,
    Scr,
    Sdg,
    Sek,
    Sgd,
    Shp,
    Sll,
    Sos,
    Srd,
    Ssp,
    Std,
    Syp,
    Szl,
    Thb,
    Tjs,
    Tmt,
    Tnd,
    Top,
    Try,
    Ttd,
    Twd,
    Tzs,
    Uah,
    Ugx,
    Usd,
    Usn,
    Uyi,
    Uyu,
    Uzs,
    Vef,
    Vnd,
    Vuv,
    Wst,
    Xag,
    Xau,
    Xba,
    Xbb,
    Xbc,
    Xbd,
    Xcd,
    Xof,
    Xpd,
    Xpf,
    Xpt,
    Xsu,
    Xts,
    Xua,
    Xxx,
    Yer,
    Zar,
    Zmw,
    Zwl,
}

/// Which side of an S2 conversation a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnergyManagementRole {
    /// Customer Energy Manager — decides *why* a device should behave a certain way.
    Cem,
    /// Resource Manager — describes *how* a device can behave.
    Rm,
}

impl EnergyManagementRole {
    /// The other side.
    #[must_use]
    pub const fn peer(self) -> Self {
        match self {
            Self::Cem => Self::Rm,
            Self::Rm => Self::Cem,
        }
    }
}

/// What became of an instruction.
///
/// The order in which these must be sent is not documented (`[s2-json #26]`,
/// erratum E11), so the session ledger accepts any order and warns
/// only on a regression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum InstructionStatus {
    /// Newly created.
    New,
    /// Accepted.
    Accepted,
    /// Rejected.
    Rejected,
    /// Revoked.
    Revoked,
    /// Execution started.
    Started,
    /// Finished successfully.
    Succeeded,
    /// Aborted.
    Aborted,
}

impl InstructionStatus {
    /// Whether no further status can follow.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Rejected | Self::Revoked | Self::Succeeded | Self::Aborted
        )
    }

    /// How far through the lifecycle this status is, for detecting a regression.
    #[must_use]
    pub const fn progress_rank(self) -> u8 {
        match self {
            Self::New => 0,
            Self::Accepted => 1,
            Self::Started => 2,
            Self::Succeeded | Self::Rejected | Self::Revoked | Self::Aborted => 3,
        }
    }
}

/// How a received message was processed, and what the sender should do about it.
///
/// The consequences are normative (`S2J schemas/ReceptionStatusValues`); see
/// [`ReceptionStatusValues::consequence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ReceptionStatusValues {
    /// "Message not understood (e.g. not valid JSON, no message_id found)."
    InvalidData,
    /// "Message was not according to schema."
    InvalidMessage,
    /// "Message contents is invalid (e.g. contains a non-existing ID). Somewhat
    /// equivalent to BAD_REQUEST in HTTP."
    InvalidContent,
    /// "Receiver encountered an error." The sender should try again.
    TemporaryError,
    /// "Receiver encountered an error which it cannot recover from." Disconnect.
    PermanentError,
    /// "Message processed normally."
    Ok,
}

/// What the standard says the *sender* should do about a reception status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Consequence {
    /// Proceed normally.
    Proceed,
    /// The message was ignored; proceed if possible.
    Ignored,
    /// Try to send the message again.
    Retry,
    /// Disconnect.
    Disconnect,
}

impl ReceptionStatusValues {
    /// The normative consequence for the sender.
    #[must_use]
    pub const fn consequence(self) -> Consequence {
        match self {
            Self::Ok => Consequence::Proceed,
            Self::InvalidData | Self::InvalidMessage | Self::InvalidContent => Consequence::Ignored,
            Self::TemporaryError => Consequence::Retry,
            Self::PermanentError => Consequence::Disconnect,
        }
    }

    /// Whether this status reports a problem.
    #[must_use]
    pub const fn is_error(self) -> bool {
        !matches!(self, Self::Ok)
    }
}

/// The kinds of object a [`RevokeObject`] message can withdraw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RevokableObjects {
    /// A `PEBC.PowerConstraints`.
    #[serde(rename = "PEBC.PowerConstraints")]
    PebcPowerConstraints,
    /// A `PEBC.EnergyConstraint`.
    #[serde(rename = "PEBC.EnergyConstraint")]
    PebcEnergyConstraint,
    /// A `PEBC.Instruction`.
    #[serde(rename = "PEBC.Instruction")]
    PebcInstruction,
    /// A `PPBC.PowerProfileDefinition`.
    #[serde(rename = "PPBC.PowerProfileDefinition")]
    PpbcPowerProfileDefinition,
    /// A `PPBC.ScheduleInstruction`.
    #[serde(rename = "PPBC.ScheduleInstruction")]
    PpbcScheduleInstruction,
    /// A `PPBC.StartInterruptionInstruction`.
    #[serde(rename = "PPBC.StartInterruptionInstruction")]
    PpbcStartInterruptionInstruction,
    /// A `PPBC.EndInterruptionInstruction`.
    #[serde(rename = "PPBC.EndInterruptionInstruction")]
    PpbcEndInterruptionInstruction,
    /// An `OMBC.SystemDescription`.
    #[serde(rename = "OMBC.SystemDescription")]
    OmbcSystemDescription,
    /// An `OMBC.Instruction`.
    #[serde(rename = "OMBC.Instruction")]
    OmbcInstruction,
    /// An `FRBC.SystemDescription`.
    #[serde(rename = "FRBC.SystemDescription")]
    FrbcSystemDescription,
    /// An `FRBC.Instruction`.
    #[serde(rename = "FRBC.Instruction")]
    FrbcInstruction,
    /// A `DDBC.SystemDescription`.
    #[serde(rename = "DDBC.SystemDescription")]
    DdbcSystemDescription,
    /// A `DDBC.Instruction`.
    #[serde(rename = "DDBC.Instruction")]
    DdbcInstruction,
}

impl RevokableObjects {
    /// Which role owns this kind of object, and may therefore revoke it.
    ///
    /// The S2 Connect state table lists `RevokeObject` only in the RM's column, but this
    /// enum contains every instruction type, which only a CEM sends. The mismatch is
    /// erratum E6; both roles are accepted and a mismatch is a warning.
    #[must_use]
    pub const fn owner(self) -> EnergyManagementRole {
        match self {
            Self::PebcInstruction
            | Self::PpbcScheduleInstruction
            | Self::PpbcStartInterruptionInstruction
            | Self::PpbcEndInterruptionInstruction
            | Self::OmbcInstruction
            | Self::FrbcInstruction
            | Self::DdbcInstruction => EnergyManagementRole::Cem,
            Self::PebcPowerConstraints
            | Self::PebcEnergyConstraint
            | Self::PpbcPowerProfileDefinition
            | Self::OmbcSystemDescription
            | Self::FrbcSystemDescription
            | Self::DdbcSystemDescription => EnergyManagementRole::Rm,
        }
    }

    /// The control type this object belongs to.
    #[must_use]
    pub const fn control_type(self) -> ControlType {
        match self {
            Self::PebcPowerConstraints | Self::PebcEnergyConstraint | Self::PebcInstruction => {
                ControlType::PowerEnvelopeBasedControl
            }
            Self::PpbcPowerProfileDefinition
            | Self::PpbcScheduleInstruction
            | Self::PpbcStartInterruptionInstruction
            | Self::PpbcEndInterruptionInstruction => ControlType::PowerProfileBasedControl,
            Self::OmbcSystemDescription | Self::OmbcInstruction => {
                ControlType::OperationModeBasedControl
            }
            Self::FrbcSystemDescription | Self::FrbcInstruction => {
                ControlType::FillRateBasedControl
            }
            Self::DdbcSystemDescription | Self::DdbcInstruction => {
                ControlType::DemandDrivenBasedControl
            }
        }
    }
}

/// What a resource does with a commodity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RoleType {
    /// Produces energy.
    EnergyProducer,
    /// Consumes energy.
    EnergyConsumer,
    /// Stores energy.
    EnergyStorage,
}

/// What a sender wants to happen to the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SessionRequestType {
    /// "Please reconnect the WebSocket session. Once reconnected, it starts from scratch
    /// with a handshake."
    Reconnect,
    /// "Disconnect the session (client can try to reconnecting with exponential
    /// backoff)."
    Terminate,
}

// ---------------------------------------------------------------------------
// Shared components
// ---------------------------------------------------------------------------

/// A range of numbers.
///
/// What "start" and "end" mean depends on the field: for a `power_range` or a
/// `fill_rate` the start is the value at an operation-mode factor of 0 and the end the
/// value at 1, so the range is *interpolated*; for `running_costs` the range expresses
/// **uncertainty** and is not linked to the factor at all.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberRange {
    /// The start of the range.
    pub start_of_range: f64,
    /// The end of the range.
    pub end_of_range: f64,
}

impl NumberRange {
    /// A range from `start` to `end`.
    #[must_use]
    pub const fn new(start: f64, end: f64) -> Self {
        Self {
            start_of_range: start,
            end_of_range: end,
        }
    }

    /// A range with a single value.
    #[must_use]
    pub const fn exactly(value: f64) -> Self {
        Self::new(value, value)
    }

    /// Whether `value` lies within the range, inclusive of both ends.
    #[must_use]
    pub fn contains(self, value: f64) -> bool {
        let (lo, hi) = self.ordered();
        value >= lo && value <= hi
    }

    /// The range as `(low, high)`, whichever way round it was written.
    #[must_use]
    pub fn ordered(self) -> (f64, f64) {
        if self.start_of_range <= self.end_of_range {
            (self.start_of_range, self.end_of_range)
        } else {
            (self.end_of_range, self.start_of_range)
        }
    }

    /// The width of the range.
    #[must_use]
    pub fn span(self) -> f64 {
        self.end_of_range - self.start_of_range
    }

    /// Linear interpolation: the value at operation-mode `factor`.
    ///
    /// the S2 documentation, *Operation modes*: `power = (end - start) * factor + start`.
    ///
    /// Factors 0 and 1 return `start` and `end` **exactly**: in floating point
    /// `start + (end - start) * 1.0` is not always `end`, and both roles compare against
    /// the number the description published (D30).
    #[must_use]
    #[allow(
        clippy::float_cmp,
        reason = "0 and 1 are the endpoints, not approximations"
    )]
    pub fn at_factor(self, factor: f64) -> f64 {
        if factor == 0.0 {
            self.start_of_range
        } else if factor == 1.0 {
            self.end_of_range
        } else {
            self.start_of_range + self.span() * factor
        }
    }

    /// The inverse of [`at_factor`](Self::at_factor): which factor produces `value`.
    ///
    /// `None` when the range is a single point, where every factor gives that value and
    /// the question has no answer.
    #[must_use]
    pub fn factor_of(self, value: f64) -> Option<f64> {
        let span = self.span();
        if span == 0.0 {
            None
        } else {
            Some((value - self.start_of_range) / span)
        }
    }

    /// Whether both ends are finite.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.start_of_range.is_finite() && self.end_of_range.is_finite()
    }

    /// Whether the two ranges overlap in more than a single point.
    #[must_use]
    pub fn overlaps(self, other: Self) -> bool {
        let (a_lo, a_hi) = self.ordered();
        let (b_lo, b_hi) = other.ordered();
        a_lo < b_hi && b_lo < a_hi
    }
}

impl From<core::ops::Range<f64>> for NumberRange {
    fn from(r: core::ops::Range<f64>) -> Self {
        Self::new(r.start, r.end)
    }
}

impl From<NumberRange> for core::ops::Range<f64> {
    fn from(r: NumberRange) -> Self {
        r.start_of_range..r.end_of_range
    }
}

/// A range of power values for one commodity quantity.
///
/// The start is the power at an operation-mode factor of 0, the end the power at 1.
/// Consumption is positive and production negative, always as seen at the grid.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerRange {
    /// The power at factor 0.
    pub start_of_range: f64,
    /// The power at factor 1.
    pub end_of_range: f64,
    /// The quantity the values refer to.
    pub commodity_quantity: CommodityQuantity,
}

impl PowerRange {
    /// A power range.
    #[must_use]
    pub const fn new(start: f64, end: f64, commodity_quantity: CommodityQuantity) -> Self {
        Self {
            start_of_range: start,
            end_of_range: end,
            commodity_quantity,
        }
    }

    /// A power range with a single value.
    #[must_use]
    pub const fn exactly(value: f64, commodity_quantity: CommodityQuantity) -> Self {
        Self::new(value, value, commodity_quantity)
    }

    /// The range without its commodity quantity.
    #[must_use]
    pub const fn range(self) -> NumberRange {
        NumberRange::new(self.start_of_range, self.end_of_range)
    }

    /// The power at operation-mode `factor`.
    #[must_use]
    pub fn at_factor(self, factor: f64) -> f64 {
        self.range().at_factor(factor)
    }
}

/// A measured power for one commodity quantity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerValue {
    /// The quantity the value refers to.
    pub commodity_quantity: CommodityQuantity,
    /// The power, in the unit the quantity implies. Consumption positive.
    pub value: f64,
}

impl PowerValue {
    /// A power value.
    #[must_use]
    pub const fn new(commodity_quantity: CommodityQuantity, value: f64) -> Self {
        Self {
            commodity_quantity,
            value,
        }
    }
}

/// A forecast power for one commodity quantity, with optional confidence bands.
///
/// The bands are nested percentile ranges: the 68 % band lies inside the 95 % band,
/// which lies inside the 100 % limits. Only `value_expected` and `commodity_quantity`
/// are required.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PowerForecastValue {
    /// The upper bound with 100 % certainty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_upper_limit: Option<f64>,
    /// The upper bound with 95 % certainty.
    #[serde(rename = "value_upper_95PPR", skip_serializing_if = "Option::is_none")]
    pub value_upper_95ppr: Option<f64>,
    /// The upper bound with 68 % certainty.
    #[serde(rename = "value_upper_68PPR", skip_serializing_if = "Option::is_none")]
    pub value_upper_68ppr: Option<f64>,
    /// The expected power.
    pub value_expected: f64,
    /// The lower bound with 68 % certainty.
    #[serde(rename = "value_lower_68PPR", skip_serializing_if = "Option::is_none")]
    pub value_lower_68ppr: Option<f64>,
    /// The lower bound with 95 % certainty.
    #[serde(rename = "value_lower_95PPR", skip_serializing_if = "Option::is_none")]
    pub value_lower_95ppr: Option<f64>,
    /// The lower bound with 100 % certainty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_lower_limit: Option<f64>,
    /// The quantity the values refer to.
    pub commodity_quantity: CommodityQuantity,
}

impl PowerForecastValue {
    /// A forecast with no uncertainty bands.
    #[must_use]
    pub const fn expected(value: f64, commodity_quantity: CommodityQuantity) -> Self {
        Self {
            value_upper_limit: None,
            value_upper_95ppr: None,
            value_upper_68ppr: None,
            value_expected: value,
            value_lower_68ppr: None,
            value_lower_95ppr: None,
            value_lower_limit: None,
            commodity_quantity,
        }
    }

    /// The bands from the lowest to the highest, skipping those that are absent.
    ///
    /// Used by the validator to check that they are ordered.
    #[must_use]
    pub fn bands_ascending(&self) -> [(&'static str, Option<f64>); 7] {
        [
            ("value_lower_limit", self.value_lower_limit),
            ("value_lower_95PPR", self.value_lower_95ppr),
            ("value_lower_68PPR", self.value_lower_68ppr),
            ("value_expected", Some(self.value_expected)),
            ("value_upper_68PPR", self.value_upper_68ppr),
            ("value_upper_95PPR", self.value_upper_95ppr),
            ("value_upper_limit", self.value_upper_limit),
        ]
    }
}

/// One time slice of a [`PowerForecast`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerForecastElement {
    /// How long this slice lasts.
    pub duration: Duration,
    /// The expected powers, at most one per commodity quantity.
    pub power_values: Vec<PowerForecastValue>,
}

/// What a resource does with one commodity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    /// Producer, consumer or storage.
    pub role: RoleType,
    /// The commodity the role refers to.
    pub commodity: Commodity,
}

impl Role {
    /// A role.
    #[must_use]
    pub const fn new(role: RoleType, commodity: Commodity) -> Self {
        Self { role, commodity }
    }
}

/// A timer that blocks or is started by a [`Transition`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
#[builder(on(String, into))]
pub struct Timer {
    /// Unique within the system or actuator description that contains it.
    pub id: Id,
    /// Human-readable, for debugging only — never for a user interface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_label: Option<String>,
    /// How long the timer runs once started.
    pub duration: Duration,
}

/// A permitted move from one operation mode to another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// Unique within the system or actuator description that contains it.
    pub id: Id,
    /// The operation mode being left.
    pub from: Id,
    /// The operation mode being entered.
    pub to: Id,
    /// Timers that are (re)started when this transition is initiated.
    pub start_timers: Vec<Id>,
    /// Timers that block this transition while any of them is unfinished.
    pub blocking_timers: Vec<Id>,
    /// Absolute cost of making the transition, in the resource's currency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition_costs: Option<f64>,
    /// Time between initiating the transition and the device behaving according to the
    /// mode it moves to. Absent means negligible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition_duration: Option<Duration>,
    /// Whether this transition may only be used during an abnormal condition.
    pub abnormal_condition_only: bool,
}

impl Transition {
    /// A transition with no timers, no costs and no duration.
    #[must_use]
    pub fn simple(id: Id, from: Id, to: Id) -> Self {
        Self {
            id,
            from,
            to,
            start_timers: Vec::new(),
            blocking_timers: Vec::new(),
            transition_costs: None,
            transition_duration: None,
            abnormal_condition_only: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Announces a node's role and the S2 versions it speaks.
///
/// Not used under S2 Connect, where the version is negotiated during session initiation
/// and `S2C §Communication - JSON messages` says the handshake messages "can not be
/// sent".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct Handshake {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The role of the sender.
    pub role: EnergyManagementRole,
    /// "Protocol versions supported by the sender of this message. This field is
    /// mandatory for the RM, but optional for the CEM."
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_protocol_versions: Option<Vec<ProtocolVersion>>,
}

/// The CEM's answer to a [`Handshake`]: the version it selected for this session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct HandshakeResponse {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The version the CEM chose, which must be one the RM offered.
    pub selected_protocol_version: ProtocolVersion,
}

/// Everything static about a resource, and which control types it can be driven in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
#[builder(on(String, into))]
pub struct ResourceManagerDetails {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// Identifies the resource. Unique within the scope of the CEM.
    pub resource_id: Id,
    /// A human-readable name, for user interfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What the resource does with each commodity. One to three roles.
    pub roles: Vec<Role>,
    /// The manufacturer's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// The model name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The serial number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    /// The firmware version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware_version: Option<String>,
    /// How long the resource takes, on average, to process and execute an instruction.
    pub instruction_processing_delay: Duration,
    /// The control types this Resource Manager supports. The CEM selects one of these.
    pub available_control_types: Vec<ControlType>,
    /// The currency every cost is expressed in. Mandatory if any cost is published.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<Currency>,
    /// Whether this Resource Manager can send a [`PowerForecast`].
    pub provides_forecast: bool,
    /// Which quantities it can measure. A [`PowerMeasurement`] may only use these.
    pub provides_power_measurement_types: Vec<CommodityQuantity>,
}

/// The CEM activating a control type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct SelectControlType {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The control type to activate. Must be one the resource offered.
    pub control_type: ControlType,
}

/// Either side asking for the session to end or restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
#[builder(on(String, into))]
pub struct SessionRequest {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// Reconnect, or terminate.
    pub request: SessionRequestType,
    /// Human-readable, for debugging only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_label: Option<String>,
}

/// The answer every message except this one gets.
///
/// Note that `ReceptionStatus` has **no** `message_id` of its own — it names the message
/// it is about, and is never itself acknowledged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
#[builder(on(String, into))]
pub struct ReceptionStatus {
    /// The message this status is about.
    pub subject_message_id: Id,
    /// How it was processed.
    pub status: ReceptionStatusValues,
    /// Human-readable diagnostics. This crate puts the failing rule's identifier here —
    /// `S2-STATE-001: FRBC.Instruction not allowed in WebSocketConnected` — so that
    /// errors are greppable across a fleet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_label: Option<String>,
}

impl ReceptionStatus {
    /// An `OK` status for `subject`.
    #[must_use]
    pub fn ok(subject: Id) -> Self {
        Self {
            subject_message_id: subject,
            status: ReceptionStatusValues::Ok,
            diagnostic_label: None,
        }
    }

    /// A failing status for `subject`, with a diagnostic.
    #[must_use]
    pub fn error(
        subject: Id,
        status: ReceptionStatusValues,
        diagnostic: impl Into<String>,
    ) -> Self {
        Self {
            subject_message_id: subject,
            status,
            diagnostic_label: Some(diagnostic.into()),
        }
    }
}

/// What the Resource Manager did with an instruction.
///
/// The second of the two answers an instruction gets: the [`ReceptionStatus`] says the
/// message was read, this says what became of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct InstructionStatusUpdate {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The instruction this is about, as the CEM identified it.
    pub instruction_id: Id,
    /// The present status.
    pub status_type: InstructionStatus,
    /// When the status last changed.
    pub timestamp: Timestamp,
}

/// A measurement of the power a resource is exchanging with the grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PowerMeasurement {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// When the values were measured.
    pub measurement_timestamp: Timestamp,
    /// The measured powers, at most one per commodity quantity.
    pub values: Vec<PowerValue>,
}

/// What a resource expects to consume or produce over a period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct PowerForecast {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// When the forecast starts.
    pub start_time: Timestamp,
    /// The slices, in chronological order.
    pub elements: Vec<PowerForecastElement>,
}

/// Withdraws an object the sender published earlier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Builder)]
#[serde(deny_unknown_fields)]
pub struct RevokeObject {
    /// This message's identifier.
    #[cfg_attr(feature = "uuid", builder(default = Id::generate()))]
    pub message_id: Id,
    /// The kind of object being withdrawn.
    pub object_type: RevokableObjects,
    /// Which one.
    pub object_id: Id,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_type_wire_spellings_are_the_standards_own() {
        // Including the one with a missing `l`, which the Rust name does not inherit.
        for (value, wire) in [
            (
                ControlType::PowerEnvelopeBasedControl,
                "\"POWER_ENVELOPE_BASED_CONTROL\"",
            ),
            (
                ControlType::PowerProfileBasedControl,
                "\"POWER_PROFILE_BASED_CONTROL\"",
            ),
            (
                ControlType::OperationModeBasedControl,
                "\"OPERATION_MODE_BASED_CONTROL\"",
            ),
            (
                ControlType::FillRateBasedControl,
                "\"FILL_RATE_BASED_CONTROL\"",
            ),
            (
                ControlType::DemandDrivenBasedControl,
                "\"DEMAND_DRIVEN_BASED_CONTROL\"",
            ),
            (ControlType::NotControllable, "\"NOT_CONTROLABLE\""),
            (ControlType::NoSelection, "\"NO_SELECTION\""),
        ] {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(serde_json::from_str::<ControlType>(wire).unwrap(), value);
        }
        // The correctly spelled string is not the wire value and must not be accepted.
        assert!(serde_json::from_str::<ControlType>("\"NOT_CONTROLLABLE\"").is_err());
    }

    #[test]
    fn commodity_quantity_wire_spellings() {
        for (value, wire) in [
            (CommodityQuantity::ElectricPowerL1, "\"ELECTRIC.POWER.L1\""),
            (
                CommodityQuantity::ElectricPower3PhaseSymmetric,
                "\"ELECTRIC.POWER.3_PHASE_SYMMETRIC\"",
            ),
            (
                CommodityQuantity::NaturalGasFlowRate,
                "\"NATURAL_GAS.FLOW_RATE\"",
            ),
            (
                CommodityQuantity::HeatThermalPower,
                "\"HEAT.THERMAL_POWER\"",
            ),
            (CommodityQuantity::OilFlowRate, "\"OIL.FLOW_RATE\""),
        ] {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            assert_eq!(
                serde_json::from_str::<CommodityQuantity>(wire).unwrap(),
                value
            );
        }
    }

    #[test]
    fn revokable_objects_keep_their_dots() {
        assert_eq!(
            serde_json::to_string(&RevokableObjects::FrbcInstruction).unwrap(),
            "\"FRBC.Instruction\""
        );
        // And the owner table is what makes E6 answerable.
        assert_eq!(
            RevokableObjects::FrbcInstruction.owner(),
            EnergyManagementRole::Cem
        );
        assert_eq!(
            RevokableObjects::FrbcSystemDescription.owner(),
            EnergyManagementRole::Rm
        );
    }

    #[test]
    fn currency_codes_are_uppercase() {
        assert_eq!(serde_json::to_string(&Currency::Eur).unwrap(), "\"EUR\"");
        assert_eq!(
            serde_json::from_str::<Currency>("\"XXX\"").unwrap(),
            Currency::Xxx
        );
    }

    #[test]
    fn reception_status_consequences_are_the_normative_ones() {
        use Consequence as C;
        use ReceptionStatusValues as R;
        assert_eq!(R::Ok.consequence(), C::Proceed);
        assert_eq!(R::InvalidData.consequence(), C::Ignored);
        assert_eq!(R::InvalidMessage.consequence(), C::Ignored);
        assert_eq!(R::InvalidContent.consequence(), C::Ignored);
        assert_eq!(R::TemporaryError.consequence(), C::Retry);
        assert_eq!(R::PermanentError.consequence(), C::Disconnect);
    }

    #[test]
    fn factor_interpolation_matches_the_documented_formula() {
        // the S2 documentation, *Operation modes*: a mode from 1000 W to 2500 W at factor 0.5
        // is 1750 W.
        let r = NumberRange::new(1000.0, 2500.0);
        assert!((r.at_factor(0.5) - 1750.0).abs() < 1e-9);
        assert!((r.at_factor(0.0) - 1000.0).abs() < 1e-9);
        assert!((r.at_factor(1.0) - 2500.0).abs() < 1e-9);
        assert!((r.factor_of(1750.0).unwrap() - 0.5).abs() < 1e-9);
        // A single-valued range gives the same value for every factor, so the inverse
        // has no answer.
        assert_eq!(NumberRange::exactly(0.0).factor_of(0.0), None);
        assert!((NumberRange::exactly(7.0).at_factor(0.3) - 7.0).abs() < 1e-12);
    }

    #[test]
    fn unknown_fields_are_refused() {
        let json = r#"{"start_of_range":0,"end_of_range":1,"extra":true}"#;
        assert!(serde_json::from_str::<NumberRange>(json).is_err());
    }

    #[test]
    fn absent_optionals_are_absent_rather_than_null() {
        let v = PowerForecastValue::expected(-3450.1, CommodityQuantity::ElectricPowerL1);
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(
            json,
            r#"{"value_expected":-3450.1,"commodity_quantity":"ELECTRIC.POWER.L1"}"#
        );
    }

    #[test]
    fn percentile_bands_keep_their_capitalised_wire_names() {
        let json = r#"{
            "value_upper_limit": -3400.0, "value_upper_95PPR": -3440.0,
            "value_upper_68PPR": -3445.0, "value_expected": -3450.1,
            "value_lower_68PPR": -3455.0, "value_lower_95PPR": -3460.0,
            "value_lower_limit": -3500.0, "commodity_quantity": "ELECTRIC.POWER.L1"
        }"#;
        let v: PowerForecastValue = serde_json::from_str(json).unwrap();
        assert_eq!(v.value_upper_95ppr, Some(-3440.0));
        assert_eq!(v.value_lower_68ppr, Some(-3455.0));
        let back = serde_json::to_string(&v).unwrap();
        assert!(back.contains("\"value_upper_95PPR\":-3440.0"));
    }
}
