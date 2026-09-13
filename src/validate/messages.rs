//! [`Validate`] for every message and component.
//!
//! One function per message, each checking the rules
//! §"Semantic rules" that apply to it. Rules that need nothing but the message run with
//! an empty [`Context`]; rules that need the session run only when it supplies one, so
//! the same code validates a lone JSON file and a live conversation.

use alloc::format;
use alloc::vec::Vec;

use super::{
    Context, ModeCheck, Report, Validate, check_array, check_band, check_envelope, check_factor,
    check_finite, check_instruction_context, check_interpolated, check_observed, check_percentiles,
    check_range, check_scheduled, check_transitions, check_unique_ids, check_unique_quantities,
    child, index, rules,
};
use crate::message::Message;
use crate::types::common::{
    Commodity, CommodityQuantity, ControlType, EnergyManagementRole, Handshake, HandshakeResponse,
    InstructionStatusUpdate, NumberRange, PowerForecast, PowerForecastValue, PowerMeasurement,
    PowerRange, ReceptionStatus, ResourceManagerDetails, RevokeObject, SelectControlType,
    SessionRequest,
};
use crate::types::{Id, ddbc, frbc, ombc, pebc, ppbc};

impl Validate for Message {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_envelope(self.kind(), self.id(), ctx, out);
        match self {
            Message::Handshake(m) => m.validate_at(path, ctx, out),
            Message::HandshakeResponse(m) => m.validate_at(path, ctx, out),
            Message::ResourceManagerDetails(m) => m.validate_at(path, ctx, out),
            Message::SelectControlType(m) => m.validate_at(path, ctx, out),
            Message::SessionRequest(m) => m.validate_at(path, ctx, out),
            Message::ReceptionStatus(m) => m.validate_at(path, ctx, out),
            Message::InstructionStatusUpdate(m) => m.validate_at(path, ctx, out),
            Message::PowerMeasurement(m) => m.validate_at(path, ctx, out),
            Message::PowerForecast(m) => m.validate_at(path, ctx, out),
            Message::RevokeObject(m) => m.validate_at(path, ctx, out),
            Message::PebcPowerConstraints(m) => m.validate_at(path, ctx, out),
            Message::PebcEnergyConstraint(m) => m.validate_at(path, ctx, out),
            Message::PebcInstruction(m) => m.validate_at(path, ctx, out),
            Message::PpbcPowerProfileDefinition(m) => m.validate_at(path, ctx, out),
            Message::PpbcPowerProfileStatus(m) => m.validate_at(path, ctx, out),
            Message::PpbcScheduleInstruction(m) => m.validate_at(path, ctx, out),
            Message::PpbcStartInterruptionInstruction(m) => m.validate_at(path, ctx, out),
            Message::PpbcEndInterruptionInstruction(m) => m.validate_at(path, ctx, out),
            Message::OmbcSystemDescription(m) => m.validate_at(path, ctx, out),
            Message::OmbcStatus(m) => m.validate_at(path, ctx, out),
            Message::OmbcTimerStatus(m) => m.validate_at(path, ctx, out),
            Message::OmbcInstruction(m) => m.validate_at(path, ctx, out),
            Message::FrbcSystemDescription(m) => m.validate_at(path, ctx, out),
            Message::FrbcStorageStatus(m) => m.validate_at(path, ctx, out),
            Message::FrbcActuatorStatus(m) => m.validate_at(path, ctx, out),
            Message::FrbcTimerStatus(m) => m.validate_at(path, ctx, out),
            Message::FrbcLeakageBehaviour(m) => m.validate_at(path, ctx, out),
            Message::FrbcUsageForecast(m) => m.validate_at(path, ctx, out),
            Message::FrbcFillLevelTargetProfile(m) => m.validate_at(path, ctx, out),
            Message::FrbcInstruction(m) => m.validate_at(path, ctx, out),
            Message::DdbcSystemDescription(m) => m.validate_at(path, ctx, out),
            Message::DdbcActuatorStatus(m) => m.validate_at(path, ctx, out),
            Message::DdbcTimerStatus(m) => m.validate_at(path, ctx, out),
            Message::DdbcAverageDemandRateForecast(m) => m.validate_at(path, ctx, out),
            Message::DdbcPresentDemandStatus(m) => m.validate_at(path, ctx, out),
            Message::DdbcInstruction(m) => m.validate_at(path, ctx, out),
        }
    }
}

// ---------------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------------

fn check_power_ranges(ranges: &[PowerRange], owner: &str, path: &str, out: &mut Report) {
    let at = child(path, "power_ranges");
    check_array(owner, "power_ranges", ranges.len(), path, out);
    check_unique_quantities(ranges.iter().map(|r| r.commodity_quantity), &at, out);
    for (i, range) in ranges.iter().enumerate() {
        let element = index(&at, i);
        check_finite(
            range.start_of_range,
            &child(&element, "start_of_range"),
            out,
        );
        check_finite(range.end_of_range, &child(&element, "end_of_range"), out);
    }
}

fn check_forecast_values(values: &[PowerForecastValue], owner: &str, path: &str, out: &mut Report) {
    let at = child(path, "power_values");
    check_array(owner, "power_values", values.len(), path, out);
    check_unique_quantities(values.iter().map(|v| v.commodity_quantity), &at, out);
    for (i, value) in values.iter().enumerate() {
        check_percentiles(&value.bands_ascending(), &index(&at, i), out);
    }
}

/// `S2J messages/ResourceManagerDetails.currency`: "Mandatory if cost information is
/// published."
fn check_currency(publishes_costs: bool, path: &str, ctx: &Context<'_>, out: &mut Report) {
    if publishes_costs
        && let Some(details) = ctx.details
        && details.currency.is_none()
    {
        out.push(
            rules::CURRENCY_REQUIRED,
            path,
            "costs are published but the ResourceManagerDetails declared no currency",
        );
    }
}

fn running_costs_present(ranges: &[Option<NumberRange>]) -> bool {
    ranges.iter().any(Option::is_some)
}

/// An actuator should not name the same commodity twice.
fn check_supported_commodities(
    commodities: &[Commodity],
    property: &str,
    path: &str,
    out: &mut Report,
) {
    let at = child(path, property);
    for (i, commodity) in commodities.iter().enumerate() {
        if commodities
            .get(..i)
            .is_some_and(|seen| seen.contains(commodity))
        {
            out.push(
                rules::DUPLICATE_SUPPORTED_COMMODITY,
                &index(&at, i),
                format!("{commodity:?} is already in this actuator's supported commodities"),
            );
        }
    }
}

/// Every commodity the actuator supports should appear in the power ranges of every
/// operation mode, or a CEM has no way to work out that commodity's power.
///
/// Not a schema requirement, but `s2-python` refuses a description without it — see
/// [`rules::COMMODITY_WITHOUT_POWER_RANGE`]. The converse check `s2-python` also makes,
/// that a commodity appears at *most* once, is deliberately **not** implemented here: a
/// three-phase asymmetric actuator legitimately publishes `ELECTRIC.POWER.L1`, `L2` and
/// `L3`, which are three quantities of one commodity. `S2-PM-001` already enforces the
/// rule the schema does state — at most one range per *quantity*.
fn check_commodity_coverage(
    supported: &[Commodity],
    ranges: &[PowerRange],
    path: &str,
    out: &mut Report,
) {
    for commodity in supported {
        if !ranges
            .iter()
            .any(|r| r.commodity_quantity.commodity() == *commodity)
        {
            out.push(
                rules::COMMODITY_WITHOUT_POWER_RANGE,
                &child(path, "power_ranges"),
                format!(
                    "the actuator supports {commodity:?} but this operation mode \
                     publishes no power range for it"
                ),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Common messages
// ---------------------------------------------------------------------------

impl Validate for Handshake {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        if let Some(versions) = &self.supported_protocol_versions {
            check_array(
                "Handshake",
                "supported_protocol_versions",
                versions.len(),
                path,
                out,
            );
        } else if self.role == EnergyManagementRole::Rm {
            // "This field is mandatory for the RM, but optional for the CEM."
            out.push(
                rules::ROLE_REQUIRED_FIELD,
                &child(path, "supported_protocol_versions"),
                "a Resource Manager must list the protocol versions it supports; \
                 the field is optional only for a CEM",
            );
        }
        let _ = ctx;
    }
}

impl Validate for HandshakeResponse {
    fn validate_at(&self, _path: &str, _ctx: &Context<'_>, _out: &mut Report) {}
}

impl Validate for SessionRequest {
    fn validate_at(&self, _path: &str, _ctx: &Context<'_>, _out: &mut Report) {}
}

impl Validate for ReceptionStatus {
    fn validate_at(&self, _path: &str, _ctx: &Context<'_>, _out: &mut Report) {}
}

impl Validate for ResourceManagerDetails {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "ResourceManagerDetails",
            "roles",
            self.roles.len(),
            path,
            out,
        );
        check_array(
            "ResourceManagerDetails",
            "available_control_types",
            self.available_control_types.len(),
            path,
            out,
        );
        check_array(
            "ResourceManagerDetails",
            "provides_power_measurement_types",
            self.provides_power_measurement_types.len(),
            path,
            out,
        );

        let mut commodities = Vec::new();
        for (i, role) in self.roles.iter().enumerate() {
            if commodities.contains(&role.commodity) {
                out.push(
                    rules::DUPLICATE_ROLE,
                    &index(&child(path, "roles"), i),
                    format!("{:?} already has a role in this resource", role.commodity),
                );
            } else {
                commodities.push(role.commodity);
            }
        }

        if self
            .available_control_types
            .contains(&ControlType::NoSelection)
        {
            out.push(
                rules::NO_SELECTION_OFFERED,
                &child(path, "available_control_types"),
                "NO_SELECTION is the absence of a control type, not one a resource can offer",
            );
        }

        check_unique_quantities(
            self.provides_power_measurement_types.iter().copied(),
            &child(path, "provides_power_measurement_types"),
            out,
        );
        let _ = ctx;
    }
}

impl Validate for SelectControlType {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        if let Some(details) = ctx.details
            && self.control_type != ControlType::NoSelection
            && !details.available_control_types.contains(&self.control_type)
        {
            out.push(
                rules::CONTROL_TYPE_NOT_OFFERED,
                &child(path, "control_type"),
                format!(
                    "{:?} is not one of the control types this resource offers",
                    self.control_type
                ),
            );
        }
    }
}

impl Validate for PowerMeasurement {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array("PowerMeasurement", "values", self.values.len(), path, out);
        let at = child(path, "values");
        check_unique_quantities(self.values.iter().map(|v| v.commodity_quantity), &at, out);
        for (i, value) in self.values.iter().enumerate() {
            check_finite(value.value, &child(&index(&at, i), "value"), out);
        }
        check_observed(
            self.measurement_timestamp,
            &child(path, "measurement_timestamp"),
            ctx,
            out,
        );

        if let Some(details) = ctx.details {
            for (i, value) in self.values.iter().enumerate() {
                if !details
                    .provides_power_measurement_types
                    .contains(&value.commodity_quantity)
                {
                    out.push(
                        rules::MEASUREMENT_NOT_OFFERED,
                        &child(&index(&at, i), "commodity_quantity"),
                        format!(
                            "{:?} is not among the quantities this resource said it measures",
                            value.commodity_quantity
                        ),
                    );
                }
            }
        }
    }
}

impl Validate for PowerForecast {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array("PowerForecast", "elements", self.elements.len(), path, out);
        let at = child(path, "elements");
        for (i, element) in self.elements.iter().enumerate() {
            check_forecast_values(
                &element.power_values,
                "PowerForecastElement",
                &index(&at, i),
                out,
            );
        }
        check_scheduled(self.start_time, &child(path, "start_time"), ctx, out);

        if let Some(details) = ctx.details
            && !details.provides_forecast
        {
            out.push(
                rules::FORECAST_NOT_OFFERED,
                path,
                "this resource said provides_forecast is false",
            );
        }
    }
}

impl Validate for InstructionStatusUpdate {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_observed(self.timestamp, &child(path, "timestamp"), ctx, out);

        if !ctx.instructions.is_empty() && !ctx.instructions.contains(&self.instruction_id) {
            out.push_related(
                rules::UNKNOWN_INSTRUCTION,
                &child(path, "instruction_id"),
                format!("{} was never sent on this session", self.instruction_id),
                [self.instruction_id],
            );
        }

        if let Some((_, previous)) = ctx
            .instruction_statuses
            .iter()
            .find(|(id, _)| *id == self.instruction_id)
            && self.status_type.progress_rank() < previous.progress_rank()
        {
            out.push_related(
                rules::STATUS_REGRESSION,
                &child(path, "status_type"),
                format!(
                    "{:?} comes after {:?}, which is already terminal or further along",
                    self.status_type, previous
                ),
                [self.instruction_id],
            );
        }
    }
}

impl Validate for RevokeObject {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        if let Some(sender) = ctx.sender
            && self.object_type.owner() != sender
        {
            out.push(
                rules::REVOKE_ROLE_MISMATCH,
                &child(path, "object_type"),
                format!(
                    "{:?} is published by the {:?}, not by the {sender:?}",
                    self.object_type,
                    self.object_type.owner()
                ),
            );
        }

        if !ctx.published.is_empty()
            && !ctx
                .published
                .iter()
                .any(|(t, id)| *t == self.object_type && *id == self.object_id)
        {
            out.push_related(
                rules::REVOKE_UNKNOWN,
                path,
                format!(
                    "no {:?} with identifier {} was published on this session",
                    self.object_type, self.object_id
                ),
                [self.object_id],
            );
        }
    }
}

// ---------------------------------------------------------------------------
// FRBC
// ---------------------------------------------------------------------------

impl Validate for frbc::SystemDescription {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "FRBC.SystemDescription",
            "actuators",
            self.actuators.len(),
            path,
            out,
        );
        check_scheduled(self.valid_from, &child(path, "valid_from"), ctx, out);
        check_unique_ids(
            self.actuators.iter().map(|a| &a.id),
            "the Resource Manager",
            &child(path, "actuators"),
            out,
        );

        let at = child(path, "actuators");
        let mut publishes_costs = false;
        for (i, actuator) in self.actuators.iter().enumerate() {
            let actuator_path = index(&at, i);
            check_array(
                "FRBC.ActuatorDescription",
                "operation_modes",
                actuator.operation_modes.len(),
                &actuator_path,
                out,
            );
            check_array(
                "FRBC.ActuatorDescription",
                "supported_commodities",
                actuator.supported_commodities.len(),
                &actuator_path,
                out,
            );
            check_supported_commodities(
                &actuator.supported_commodities,
                "supported_commodities",
                &actuator_path,
                out,
            );
            check_unique_ids(
                actuator.operation_modes.iter().map(|m| &m.id),
                "this actuator",
                &child(&actuator_path, "operation_modes"),
                out,
            );
            check_unique_ids(
                actuator.timers.iter().map(|t| &t.id),
                "this actuator",
                &child(&actuator_path, "timers"),
                out,
            );
            check_unique_ids(
                actuator.transitions.iter().map(|t| &t.id),
                "this actuator",
                &child(&actuator_path, "transitions"),
                out,
            );

            let modes = child(&actuator_path, "operation_modes");
            for (j, mode) in actuator.operation_modes.iter().enumerate() {
                let mode_path = index(&modes, j);
                check_array(
                    "FRBC.OperationMode",
                    "elements",
                    mode.elements.len(),
                    &mode_path,
                    out,
                );
                let elements = child(&mode_path, "elements");
                for (k, element) in mode.elements.iter().enumerate() {
                    let element_path = index(&elements, k);
                    check_band(
                        element.fill_level_range,
                        &child(&element_path, "fill_level_range"),
                        out,
                    );
                    check_interpolated(element.fill_rate, &child(&element_path, "fill_rate"), out);
                    check_power_ranges(
                        &element.power_ranges,
                        "FRBC.OperationModeElement",
                        &element_path,
                        out,
                    );
                    if let Some(costs) = element.running_costs {
                        check_interpolated(costs, &child(&element_path, "running_costs"), out);
                    }
                    check_commodity_coverage(
                        &actuator.supported_commodities,
                        &element.power_ranges,
                        &element_path,
                        out,
                    );
                    publishes_costs |= element.running_costs.is_some();
                }
                let bands: Vec<NumberRange> =
                    mode.elements.iter().map(|e| e.fill_level_range).collect();
                super::check_contiguous(
                    &bands,
                    rules::FRBC_ELEMENTS_CONTIGUOUS,
                    "operation mode fill level bands",
                    &elements,
                    out,
                );
            }

            let mode_ids: Vec<Id> = actuator.operation_modes.iter().map(|m| m.id).collect();
            let timer_ids: Vec<Id> = actuator.timers.iter().map(|t| t.id).collect();
            check_transitions(
                &actuator.transitions,
                &mode_ids,
                &timer_ids,
                rules::FRBC_TRANSITION_REFERENCES,
                &child(&actuator_path, "transitions"),
                out,
            );
            publishes_costs |= actuator
                .transitions
                .iter()
                .any(|t| t.transition_costs.is_some());
        }

        check_band(
            self.storage.fill_level_range,
            &child(&child(path, "storage"), "fill_level_range"),
            out,
        );
        check_currency(publishes_costs, path, ctx, out);
    }
}

impl Validate for frbc::StorageStatus {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        let at = child(path, "present_fill_level");
        check_finite(self.present_fill_level, &at, out);
        if let Some(system) = ctx.frbc
            && self.present_fill_level.is_finite()
            && !system
                .storage
                .fill_level_range
                .contains(self.present_fill_level)
        {
            out.push(
                rules::FRBC_FILL_LEVEL_OUT_OF_RANGE,
                &at,
                format!(
                    "{} is outside the storage's range {}..{}; \
                     the Resource Manager may ignore instructions until it is back inside",
                    self.present_fill_level,
                    system.storage.fill_level_range.start_of_range,
                    system.storage.fill_level_range.end_of_range
                ),
            );
        }
    }
}

impl Validate for frbc::ActuatorStatus {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_factor(
            self.operation_mode_factor,
            &child(path, "operation_mode_factor"),
            out,
        );
        if let Some(at) = self.transition_timestamp {
            check_observed(at, &child(path, "transition_timestamp"), ctx, out);
        }
        // Per actuator, not per session: an FRBC system may describe up to ten actuators,
        // and the first status of the second one is still a first status.
        if ctx.active_mode_of(&self.actuator_id).is_some()
            && self.previous_operation_mode_id.is_none()
        {
            out.push_related(
                rules::PREVIOUS_MODE_MISSING,
                path,
                format!(
                    "this is not the first status for actuator {}, so the previous \
                     operation mode and the transition timestamp should be present",
                    self.actuator_id
                ),
                [self.actuator_id],
            );
        }
        if let Some(system) = ctx.frbc {
            match system.actuator(&self.actuator_id) {
                None => out.push_related(
                    rules::FRBC_UNKNOWN_ACTUATOR,
                    &child(path, "actuator_id"),
                    format!("{} is not an actuator of this system", self.actuator_id),
                    [self.actuator_id],
                ),
                Some(actuator) => {
                    if actuator
                        .operation_mode(&self.active_operation_mode_id)
                        .is_none()
                    {
                        out.push_related(
                            rules::FRBC_UNKNOWN_MODE,
                            &child(path, "active_operation_mode_id"),
                            format!(
                                "{} is not an operation mode of actuator {}",
                                self.active_operation_mode_id, self.actuator_id
                            ),
                            [self.active_operation_mode_id],
                        );
                    }
                }
            }
        }
    }
}

impl Validate for frbc::TimerStatus {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        if let Some(system) = ctx.frbc {
            match system.actuator(&self.actuator_id) {
                None => out.push_related(
                    rules::FRBC_UNKNOWN_ACTUATOR,
                    &child(path, "actuator_id"),
                    format!("{} is not an actuator of this system", self.actuator_id),
                    [self.actuator_id],
                ),
                Some(actuator) if actuator.timer(&self.timer_id).is_none() => out.push_related(
                    rules::FRBC_TRANSITION_REFERENCES,
                    &child(path, "timer_id"),
                    format!(
                        "{} is not a timer of actuator {}",
                        self.timer_id, self.actuator_id
                    ),
                    [self.timer_id],
                ),
                Some(_) => {}
            }
        }
    }
}

impl Validate for frbc::LeakageBehaviour {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "FRBC.LeakageBehaviour",
            "elements",
            self.elements.len(),
            path,
            out,
        );
        check_scheduled(self.valid_from, &child(path, "valid_from"), ctx, out);
        let at = child(path, "elements");
        for (i, element) in self.elements.iter().enumerate() {
            let element_path = index(&at, i);
            check_band(
                element.fill_level_range,
                &child(&element_path, "fill_level_range"),
                out,
            );
            check_finite(
                element.leakage_rate,
                &child(&element_path, "leakage_rate"),
                out,
            );
        }
        let bands: Vec<NumberRange> = self.elements.iter().map(|e| e.fill_level_range).collect();
        super::check_contiguous(
            &bands,
            rules::FRBC_LEAKAGE_CONTIGUOUS,
            "leakage bands",
            &at,
            out,
        );

        if let Some(system) = ctx.frbc
            && !system.storage.provides_leakage_behaviour
        {
            out.push(
                rules::FRBC_NOT_OFFERED,
                path,
                "the storage description said provides_leakage_behaviour is false",
            );
        }
    }
}

impl Validate for frbc::UsageForecast {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "FRBC.UsageForecast",
            "elements",
            self.elements.len(),
            path,
            out,
        );
        check_scheduled(self.start_time, &child(path, "start_time"), ctx, out);
        let at = child(path, "elements");
        for (i, element) in self.elements.iter().enumerate() {
            check_percentiles(&element.bands_ascending(), &index(&at, i), out);
        }
        if let Some(system) = ctx.frbc
            && !system.storage.provides_usage_forecast
        {
            out.push(
                rules::FRBC_NOT_OFFERED,
                path,
                "the storage description said provides_usage_forecast is false",
            );
        }
    }
}

impl Validate for frbc::FillLevelTargetProfile {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "FRBC.FillLevelTargetProfile",
            "elements",
            self.elements.len(),
            path,
            out,
        );
        check_scheduled(self.start_time, &child(path, "start_time"), ctx, out);
        let at = child(path, "elements");
        for (i, element) in self.elements.iter().enumerate() {
            // A target band may be a single point — "The start of the range must be
            // smaller or equal to the end of the range" — unlike an operation mode's.
            check_range(
                element.fill_level_range,
                &child(&index(&at, i), "fill_level_range"),
                out,
            );
        }
        if let Some(system) = ctx.frbc
            && !system.storage.provides_fill_level_target_profile
        {
            out.push(
                rules::FRBC_NOT_OFFERED,
                path,
                "the storage description said provides_fill_level_target_profile is false",
            );
        }
    }
}

impl Validate for frbc::Instruction {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_factor(
            self.operation_mode_factor,
            &child(path, "operation_mode_factor"),
            out,
        );
        check_scheduled(
            self.execution_time,
            &child(path, "execution_time"),
            ctx,
            out,
        );
        check_duplicate_instruction(self.id, path, ctx, out);

        let Some(system) = ctx.frbc else { return };
        let Some(actuator) = system.actuator(&self.actuator_id) else {
            out.push_related(
                rules::FRBC_UNKNOWN_ACTUATOR,
                &child(path, "actuator_id"),
                format!("{} is not an actuator of this system", self.actuator_id),
                [self.actuator_id],
            );
            return;
        };
        let Some(mode) = actuator.operation_mode(&self.operation_mode) else {
            out.push_related(
                rules::FRBC_UNKNOWN_MODE,
                &child(path, "operation_mode"),
                format!(
                    "{} is not an operation mode of actuator {}",
                    self.operation_mode, self.actuator_id
                ),
                [self.operation_mode],
            );
            return;
        };
        check_instruction_context(
            &ModeCheck {
                actuator: Some(&self.actuator_id),
                mode: &self.operation_mode,
                abnormal_condition: self.abnormal_condition,
                abnormal_only: mode.abnormal_condition_only,
                transitions: &actuator.transitions,
                timers: &actuator.timers,
            },
            path,
            ctx,
            out,
        );
    }
}

// ---------------------------------------------------------------------------
// OMBC
// ---------------------------------------------------------------------------

impl Validate for ombc::SystemDescription {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "OMBC.SystemDescription",
            "operation_modes",
            self.operation_modes.len(),
            path,
            out,
        );
        check_scheduled(self.valid_from, &child(path, "valid_from"), ctx, out);
        check_unique_ids(
            self.operation_modes.iter().map(|m| &m.id),
            "the Resource Manager",
            &child(path, "operation_modes"),
            out,
        );
        check_unique_ids(
            self.timers.iter().map(|t| &t.id),
            "this description",
            &child(path, "timers"),
            out,
        );
        check_unique_ids(
            self.transitions.iter().map(|t| &t.id),
            "this description",
            &child(path, "transitions"),
            out,
        );

        let modes = child(path, "operation_modes");
        let mut costs = Vec::new();
        for (i, mode) in self.operation_modes.iter().enumerate() {
            let mode_path = index(&modes, i);
            check_power_ranges(&mode.power_ranges, "OMBC.OperationMode", &mode_path, out);
            if let Some(running) = mode.running_costs {
                check_interpolated(running, &child(&mode_path, "running_costs"), out);
            }
            costs.push(mode.running_costs);
        }

        let mode_ids: Vec<Id> = self.operation_modes.iter().map(|m| m.id).collect();
        let timer_ids: Vec<Id> = self.timers.iter().map(|t| t.id).collect();
        check_transitions(
            &self.transitions,
            &mode_ids,
            &timer_ids,
            rules::OMBC_TRANSITION_REFERENCES,
            &child(path, "transitions"),
            out,
        );

        let publishes_costs = running_costs_present(&costs)
            || self
                .transitions
                .iter()
                .any(|t| t.transition_costs.is_some());
        check_currency(publishes_costs, path, ctx, out);
    }
}

impl Validate for ombc::Status {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_factor(
            self.operation_mode_factor,
            &child(path, "operation_mode_factor"),
            out,
        );
        if let Some(at) = self.transition_timestamp {
            check_observed(at, &child(path, "transition_timestamp"), ctx, out);
        }
        // OMBC has one state machine rather than actuators, so `Id::NIL` is its key in
        // `active_modes` — the same stand-in the registry uses.
        if ctx.active_mode_of(&Id::NIL).is_some() && self.previous_operation_mode_id.is_none() {
            out.push(
                rules::PREVIOUS_MODE_MISSING,
                path,
                "this is not the first status, so the previous operation mode and the \
                 transition timestamp should be present",
            );
        }
        if let Some(system) = ctx.ombc
            && system
                .operation_mode(&self.active_operation_mode_id)
                .is_none()
        {
            out.push_related(
                rules::OMBC_UNKNOWN_MODE,
                &child(path, "active_operation_mode_id"),
                format!(
                    "{} is not an operation mode of this system",
                    self.active_operation_mode_id
                ),
                [self.active_operation_mode_id],
            );
        }
    }
}

impl Validate for ombc::TimerStatus {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        if let Some(system) = ctx.ombc
            && system.timer(&self.timer_id).is_none()
        {
            out.push_related(
                rules::OMBC_TRANSITION_REFERENCES,
                &child(path, "timer_id"),
                format!("{} is not a timer of this system", self.timer_id),
                [self.timer_id],
            );
        }
    }
}

impl Validate for ombc::Instruction {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_factor(
            self.operation_mode_factor,
            &child(path, "operation_mode_factor"),
            out,
        );
        check_scheduled(
            self.execution_time,
            &child(path, "execution_time"),
            ctx,
            out,
        );
        check_duplicate_instruction(self.id, path, ctx, out);

        let Some(system) = ctx.ombc else { return };
        let Some(mode) = system.operation_mode(&self.operation_mode_id) else {
            out.push_related(
                rules::OMBC_UNKNOWN_MODE,
                &child(path, "operation_mode_id"),
                format!(
                    "{} is not an operation mode of this system",
                    self.operation_mode_id
                ),
                [self.operation_mode_id],
            );
            return;
        };
        check_instruction_context(
            &ModeCheck {
                actuator: None,
                mode: &self.operation_mode_id,
                abnormal_condition: self.abnormal_condition,
                abnormal_only: mode.abnormal_condition_only,
                transitions: &system.transitions,
                timers: &system.timers,
            },
            path,
            ctx,
            out,
        );
    }
}

// ---------------------------------------------------------------------------
// DDBC
// ---------------------------------------------------------------------------

impl Validate for ddbc::SystemDescription {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "DDBC.SystemDescription",
            "actuators",
            self.actuators.len(),
            path,
            out,
        );
        check_scheduled(self.valid_from, &child(path, "valid_from"), ctx, out);
        check_unique_ids(
            self.actuators.iter().map(|a| &a.id),
            "the Resource Manager",
            &child(path, "actuators"),
            out,
        );
        if let Some(rate) = self.present_demand_rate {
            check_range(rate, &child(path, "present_demand_rate"), out);
        }

        let at = child(path, "actuators");
        let mut publishes_costs = false;
        for (i, actuator) in self.actuators.iter().enumerate() {
            let actuator_path = index(&at, i);
            check_array(
                "DDBC.ActuatorDescription",
                "operation_modes",
                actuator.operation_modes.len(),
                &actuator_path,
                out,
            );
            check_array(
                "DDBC.ActuatorDescription",
                "supported_commodites",
                actuator.supported_commodities.len(),
                &actuator_path,
                out,
            );
            check_supported_commodities(
                &actuator.supported_commodities,
                // The wire spells it without the second `i` (erratum E5), and a JSON
                // pointer names the wire.
                "supported_commodites",
                &actuator_path,
                out,
            );
            check_unique_ids(
                actuator.operation_modes.iter().map(|m| &m.id),
                "this actuator",
                &child(&actuator_path, "operation_modes"),
                out,
            );
            // FRBC checks these; DDBC declares the same `timers` and `transitions` under
            // the same uniqueness sentence, so it checks them too.
            check_unique_ids(
                actuator.timers.iter().map(|t| &t.id),
                "this actuator",
                &child(&actuator_path, "timers"),
                out,
            );
            check_unique_ids(
                actuator.transitions.iter().map(|t| &t.id),
                "this actuator",
                &child(&actuator_path, "transitions"),
                out,
            );

            let modes = child(&actuator_path, "operation_modes");
            for (j, mode) in actuator.operation_modes.iter().enumerate() {
                let mode_path = index(&modes, j);
                check_power_ranges(&mode.power_ranges, "DDBC.OperationMode", &mode_path, out);
                check_commodity_coverage(
                    &actuator.supported_commodities,
                    &mode.power_ranges,
                    &mode_path,
                    out,
                );
                check_interpolated(mode.supply_range, &child(&mode_path, "supply_range"), out);
                if let Some(running) = mode.running_costs {
                    check_interpolated(running, &child(&mode_path, "running_costs"), out);
                }
                publishes_costs |= mode.running_costs.is_some();
            }

            let mode_ids: Vec<Id> = actuator.operation_modes.iter().map(|m| m.id).collect();
            let timer_ids: Vec<Id> = actuator.timers.iter().map(|t| t.id).collect();
            check_transitions(
                &actuator.transitions,
                &mode_ids,
                &timer_ids,
                rules::DDBC_TRANSITION_REFERENCES,
                &child(&actuator_path, "transitions"),
                out,
            );
            publishes_costs |= actuator
                .transitions
                .iter()
                .any(|t| t.transition_costs.is_some());
        }
        check_currency(publishes_costs, path, ctx, out);
    }
}

impl Validate for ddbc::PresentDemandStatus {
    fn validate_at(&self, path: &str, _ctx: &Context<'_>, out: &mut Report) {
        check_range(
            self.present_demand_rate,
            &child(path, "present_demand_rate"),
            out,
        );
    }
}

impl Validate for ddbc::ActuatorStatus {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_factor(
            self.operation_mode_factor,
            &child(path, "operation_mode_factor"),
            out,
        );
        if let Some(at) = self.transition_timestamp {
            check_observed(at, &child(path, "transition_timestamp"), ctx, out);
        }
        // Per actuator, not per session: an FRBC system may describe up to ten actuators,
        // and the first status of the second one is still a first status.
        if ctx.active_mode_of(&self.actuator_id).is_some()
            && self.previous_operation_mode_id.is_none()
        {
            out.push_related(
                rules::PREVIOUS_MODE_MISSING,
                path,
                format!(
                    "this is not the first status for actuator {}, so the previous \
                     operation mode and the transition timestamp should be present",
                    self.actuator_id
                ),
                [self.actuator_id],
            );
        }
        if let Some(system) = ctx.ddbc {
            match system.actuator(&self.actuator_id) {
                None => out.push_related(
                    rules::DDBC_UNKNOWN_ACTUATOR,
                    &child(path, "actuator_id"),
                    format!("{} is not an actuator of this system", self.actuator_id),
                    [self.actuator_id],
                ),
                Some(actuator)
                    if actuator
                        .operation_mode(&self.active_operation_mode_id)
                        .is_none() =>
                {
                    out.push_related(
                        rules::DDBC_UNKNOWN_MODE,
                        &child(path, "active_operation_mode_id"),
                        format!(
                            "{} is not an operation mode of actuator {}",
                            self.active_operation_mode_id, self.actuator_id
                        ),
                        [self.active_operation_mode_id],
                    );
                }
                Some(_) => {}
            }
        }
    }
}

impl Validate for ddbc::TimerStatus {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        if let Some(system) = ctx.ddbc {
            match system.actuator(&self.actuator_id) {
                None => out.push_related(
                    rules::DDBC_UNKNOWN_ACTUATOR,
                    &child(path, "actuator_id"),
                    format!("{} is not an actuator of this system", self.actuator_id),
                    [self.actuator_id],
                ),
                Some(actuator) if actuator.timer(&self.timer_id).is_none() => out.push_related(
                    rules::DDBC_TRANSITION_REFERENCES,
                    &child(path, "timer_id"),
                    format!(
                        "{} is not a timer of actuator {}",
                        self.timer_id, self.actuator_id
                    ),
                    [self.timer_id],
                ),
                Some(_) => {}
            }
        }
    }
}

impl Validate for ddbc::AverageDemandRateForecast {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "DDBC.AverageDemandRateForecast",
            "elements",
            self.elements.len(),
            path,
            out,
        );
        check_scheduled(self.start_time, &child(path, "start_time"), ctx, out);
        let at = child(path, "elements");
        for (i, element) in self.elements.iter().enumerate() {
            check_percentiles(&element.bands_ascending(), &index(&at, i), out);
        }
        if let Some(system) = ctx.ddbc
            && !system.provides_average_demand_rate_forecast
        {
            out.push(
                rules::DDBC_FORECAST_NOT_OFFERED,
                path,
                "the system description said provides_average_demand_rate_forecast is false",
            );
        }
    }
}

impl Validate for ddbc::Instruction {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_factor(
            self.operation_mode_factor,
            &child(path, "operation_mode_factor"),
            out,
        );
        check_scheduled(
            self.execution_time,
            &child(path, "execution_time"),
            ctx,
            out,
        );
        check_duplicate_instruction(self.id, path, ctx, out);

        let Some(system) = ctx.ddbc else { return };
        let Some(actuator) = system.actuator(&self.actuator_id) else {
            out.push_related(
                rules::DDBC_UNKNOWN_ACTUATOR,
                &child(path, "actuator_id"),
                format!("{} is not an actuator of this system", self.actuator_id),
                [self.actuator_id],
            );
            return;
        };
        let Some(mode) = actuator.operation_mode(&self.operation_mode_id) else {
            out.push_related(
                rules::DDBC_UNKNOWN_MODE,
                &child(path, "operation_mode_id"),
                format!(
                    "{} is not an operation mode of actuator {}",
                    self.operation_mode_id, self.actuator_id
                ),
                [self.operation_mode_id],
            );
            return;
        };
        check_instruction_context(
            &ModeCheck {
                actuator: Some(&self.actuator_id),
                mode: &self.operation_mode_id,
                abnormal_condition: self.abnormal_condition,
                abnormal_only: mode.abnormal_condition_only,
                transitions: &actuator.transitions,
                timers: &actuator.timers,
            },
            path,
            ctx,
            out,
        );
    }
}

// ---------------------------------------------------------------------------
// PEBC
// ---------------------------------------------------------------------------

impl Validate for pebc::PowerConstraints {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "PEBC.PowerConstraints",
            "allowed_limit_ranges",
            self.allowed_limit_ranges.len(),
            path,
            out,
        );
        check_scheduled(self.valid_from, &child(path, "valid_from"), ctx, out);

        if let Some(until) = self.valid_until
            && until < self.valid_from
        {
            out.push(
                rules::PEBC_VALIDITY_WINDOW,
                &child(path, "valid_until"),
                format!("{until} is before valid_from {}", self.valid_from),
            );
        }

        let at = child(path, "allowed_limit_ranges");
        for (i, range) in self.allowed_limit_ranges.iter().enumerate() {
            check_range(
                range.range_boundary,
                &child(&index(&at, i), "range_boundary"),
                out,
            );
        }

        let has_upper = self
            .allowed_limit_ranges
            .iter()
            .any(|r| r.limit_type == pebc::PowerEnvelopeLimitType::UpperLimit);
        let has_lower = self
            .allowed_limit_ranges
            .iter()
            .any(|r| r.limit_type == pebc::PowerEnvelopeLimitType::LowerLimit);
        if !has_upper || !has_lower {
            out.push(
                rules::PEBC_BOTH_LIMITS,
                &at,
                "there must be at least one UPPER_LIMIT range and at least one LOWER_LIMIT range",
            );
        }
    }
}

impl Validate for pebc::EnergyConstraint {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_finite(
            self.upper_average_power,
            &child(path, "upper_average_power"),
            out,
        );
        check_finite(
            self.lower_average_power,
            &child(path, "lower_average_power"),
            out,
        );
        check_scheduled(self.valid_from, &child(path, "valid_from"), ctx, out);

        if self.valid_until < self.valid_from {
            out.push(
                rules::PEBC_VALIDITY_WINDOW,
                &child(path, "valid_until"),
                format!(
                    "{} is before valid_from {}",
                    self.valid_until, self.valid_from
                ),
            );
        }
        if self.lower_average_power.is_finite()
            && self.upper_average_power.is_finite()
            && self.lower_average_power > self.upper_average_power
        {
            out.push(
                rules::PEBC_ENERGY_ORDER,
                path,
                format!(
                    "lower_average_power {} is above upper_average_power {}",
                    self.lower_average_power, self.upper_average_power
                ),
            );
        }
    }
}

impl Validate for pebc::Instruction {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "PEBC.Instruction",
            "power_envelopes",
            self.power_envelopes.len(),
            path,
            out,
        );
        check_scheduled(
            self.execution_time,
            &child(path, "execution_time"),
            ctx,
            out,
        );
        check_duplicate_instruction(self.id, path, ctx, out);

        let at = child(path, "power_envelopes");
        let mut seen: Vec<CommodityQuantity> = Vec::new();
        for (i, envelope) in self.power_envelopes.iter().enumerate() {
            let envelope_path = index(&at, i);
            if seen.contains(&envelope.commodity_quantity) {
                out.push(
                    rules::PEBC_ONE_ENVELOPE_PER_QUANTITY,
                    &child(&envelope_path, "commodity_quantity"),
                    format!(
                        "{:?} already has an envelope in this instruction",
                        envelope.commodity_quantity
                    ),
                );
            } else {
                seen.push(envelope.commodity_quantity);
            }
            check_array(
                "PEBC.PowerEnvelope",
                "power_envelope_elements",
                envelope.power_envelope_elements.len(),
                &envelope_path,
                out,
            );
            let elements = child(&envelope_path, "power_envelope_elements");
            for (j, element) in envelope.power_envelope_elements.iter().enumerate() {
                let element_path = index(&elements, j);
                check_finite(
                    element.lower_limit,
                    &child(&element_path, "lower_limit"),
                    out,
                );
                check_finite(
                    element.upper_limit,
                    &child(&element_path, "upper_limit"),
                    out,
                );
                if element.lower_limit.is_finite()
                    && element.upper_limit.is_finite()
                    && element.lower_limit > element.upper_limit
                {
                    out.push(
                        rules::PEBC_ENVELOPE_LIMIT_ORDER,
                        &element_path,
                        format!(
                            "lower_limit {} is above upper_limit {}",
                            element.lower_limit, element.upper_limit
                        ),
                    );
                }
            }
        }

        let Some(constraints) = ctx
            .pebc_constraints
            .iter()
            .find(|c| c.id == self.power_constraints_id)
        else {
            if !ctx.pebc_constraints.is_empty() {
                out.push_related(
                    rules::PEBC_UNKNOWN_CONSTRAINTS,
                    &child(path, "power_constraints_id"),
                    format!(
                        "{} names no PEBC.PowerConstraints published on this session",
                        self.power_constraints_id
                    ),
                    [self.power_constraints_id],
                );
            }
            return;
        };

        if let Some(now) = ctx.now
            && !constraints.is_valid_at(now)
        {
            out.push_related(
                rules::PEBC_UNKNOWN_CONSTRAINTS,
                &child(path, "power_constraints_id"),
                format!("{} is not in force at {now}", constraints.id),
                [constraints.id],
            );
        }

        for (i, envelope) in self.power_envelopes.iter().enumerate() {
            let elements = child(&index(&at, i), "power_envelope_elements");
            for (j, element) in envelope.power_envelope_elements.iter().enumerate() {
                let element_path = index(&elements, j);
                check_within_allowed(
                    constraints,
                    envelope.commodity_quantity,
                    pebc::PowerEnvelopeLimitType::UpperLimit,
                    element.upper_limit,
                    self.abnormal_condition,
                    &child(&element_path, "upper_limit"),
                    out,
                );
                check_within_allowed(
                    constraints,
                    envelope.commodity_quantity,
                    pebc::PowerEnvelopeLimitType::LowerLimit,
                    element.lower_limit,
                    self.abnormal_condition,
                    &child(&element_path, "lower_limit"),
                    out,
                );
            }
        }
    }
}

fn check_within_allowed(
    constraints: &pebc::PowerConstraints,
    quantity: CommodityQuantity,
    limit_type: pebc::PowerEnvelopeLimitType,
    value: f64,
    abnormal_condition: bool,
    path: &str,
    out: &mut Report,
) {
    if !value.is_finite() {
        return;
    }
    let mut any = false;
    let mut fits = false;
    let mut needs_abnormal = false;
    for range in constraints.ranges_for(quantity, limit_type) {
        any = true;
        if range.range_boundary.contains(value) {
            if range.abnormal_condition_only && !abnormal_condition {
                needs_abnormal = true;
            } else {
                fits = true;
            }
        }
    }
    if !any {
        out.push(
            rules::PEBC_OUTSIDE_ALLOWED,
            path,
            format!("the constraints allow no {limit_type:?} for {quantity:?}"),
        );
    } else if !fits {
        if needs_abnormal {
            out.push(
                rules::ABNORMAL_ONLY,
                path,
                format!(
                    "{value} is only allowed by a range marked abnormal_condition_only, \
                     and this instruction is not one"
                ),
            );
        } else {
            out.push(
                rules::PEBC_OUTSIDE_ALLOWED,
                path,
                format!("{value} is outside every allowed {limit_type:?} range for {quantity:?}"),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// PPBC
// ---------------------------------------------------------------------------

impl Validate for ppbc::PowerProfileDefinition {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "PPBC.PowerProfileDefinition",
            "power_sequences_containers",
            self.power_sequence_containers.len(),
            path,
            out,
        );
        check_scheduled(self.start_time, &child(path, "start_time"), ctx, out);

        if self.end_time < self.start_time {
            out.push(
                rules::PPBC_WINDOW,
                &child(path, "end_time"),
                format!("{} is before start_time {}", self.end_time, self.start_time),
            );
        }

        check_unique_ids(
            self.power_sequence_containers.iter().map(|c| &c.id),
            "this profile",
            &child(path, "power_sequences_containers"),
            out,
        );

        let at = child(path, "power_sequences_containers");
        let window = self
            .end_time
            .checked_duration_since(self.start_time)
            .unwrap_or(crate::types::Duration::ZERO);
        for (i, container) in self.power_sequence_containers.iter().enumerate() {
            let container_path = index(&at, i);
            check_array(
                "PPBC.PowerSequenceContainer",
                "power_sequences",
                container.power_sequences.len(),
                &container_path,
                out,
            );
            check_unique_ids(
                container.power_sequences.iter().map(|s| &s.id),
                "this container",
                &child(&container_path, "power_sequences"),
                out,
            );

            let sequences = child(&container_path, "power_sequences");
            let mut shortest: Option<crate::types::Duration> = None;
            for (j, sequence) in container.power_sequences.iter().enumerate() {
                let sequence_path = index(&sequences, j);
                check_array(
                    "PPBC.PowerSequence",
                    "elements",
                    sequence.elements.len(),
                    &sequence_path,
                    out,
                );
                let elements = child(&sequence_path, "elements");
                for (k, element) in sequence.elements.iter().enumerate() {
                    check_forecast_values(
                        &element.power_values,
                        "PPBC.PowerSequenceElement",
                        &index(&elements, k),
                        out,
                    );
                }
                let total = sequence.total_duration();
                shortest = Some(shortest.map_or(total, |s| if total < s { total } else { s }));
            }
            if let Some(shortest) = shortest
                && shortest.as_millis() > window.as_millis()
            {
                out.push(
                    rules::PPBC_WINDOW_TOO_SHORT,
                    &container_path,
                    format!(
                        "the shortest sequence takes {shortest} but the window is only {window}"
                    ),
                );
            }
        }
    }
}

impl Validate for ppbc::PowerProfileStatus {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_array(
            "PPBC.PowerProfileStatus",
            "sequence_container_status",
            self.sequence_container_status.len(),
            path,
            out,
        );
        let at = child(path, "sequence_container_status");
        for (i, status) in self.sequence_container_status.iter().enumerate() {
            let status_path = index(&at, i);
            let started = status.status.has_started();
            match (started, status.progress) {
                (true, None) => out.push(
                    rules::PPBC_PROGRESS,
                    &status_path,
                    format!(
                        "{:?} means the sequence has started, so progress must be present",
                        status.status
                    ),
                ),
                (false, Some(_)) => out.push(
                    rules::PPBC_PROGRESS,
                    &child(&status_path, "progress"),
                    format!(
                        "{:?} means nothing has started, so progress must be absent",
                        status.status
                    ),
                ),
                _ => {}
            }
            if status.status.has_selection() && status.selected_sequence_id.is_none() {
                out.push(
                    rules::PPBC_PROGRESS,
                    &status_path,
                    format!(
                        "{:?} means a sequence was chosen, so selected_sequence_id must be present",
                        status.status
                    ),
                );
            }
        }

        // "Array with status information for all PPBC.PowerSequenceContainers".
        for profile in ctx.ppbc_profiles {
            if !self
                .sequence_container_status
                .iter()
                .any(|s| s.power_profile_id == profile.id)
            {
                continue;
            }
            for container in &profile.power_sequence_containers {
                if !self.sequence_container_status.iter().any(|s| {
                    s.power_profile_id == profile.id && s.sequence_container_id == container.id
                }) {
                    out.push_related(
                        rules::PPBC_STATUS_COVERAGE,
                        &at,
                        format!(
                            "container {} of profile {} has no status",
                            container.id, profile.id
                        ),
                        [container.id],
                    );
                }
            }
        }
    }
}

/// The three PPBC instructions differ only in what they do to the sequence they name.
#[allow(clippy::too_many_arguments)] // three identifiers, a flag, a path, a context and a report
fn check_ppbc_instruction(
    id: Id,
    profile_id: Id,
    container_id: Id,
    sequence_id: Id,
    needs_interruptible: bool,
    path: &str,
    ctx: &Context<'_>,
    out: &mut Report,
) {
    check_duplicate_instruction(id, path, ctx, out);

    if ctx.ppbc_profiles.is_empty() {
        return;
    }
    let Some(profile) = ctx.ppbc_profiles.iter().find(|p| p.id == profile_id) else {
        out.push_related(
            rules::PPBC_UNKNOWN_SEQUENCE,
            &child(path, "power_profile_id"),
            format!("{profile_id} names no profile published on this session"),
            [profile_id],
        );
        return;
    };
    let Some((_, sequence)) = profile.resolve(&container_id, &sequence_id) else {
        out.push_related(
            rules::PPBC_UNKNOWN_SEQUENCE,
            path,
            format!(
                "profile {profile_id} has no sequence {sequence_id} in container {container_id}"
            ),
            [container_id, sequence_id],
        );
        return;
    };
    if needs_interruptible && !sequence.is_interruptible {
        out.push_related(
            rules::PPBC_NOT_INTERRUPTIBLE,
            path,
            format!("sequence {sequence_id} says is_interruptible is false"),
            [sequence_id],
        );
    }
}

impl Validate for ppbc::ScheduleInstruction {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_scheduled(
            self.execution_time,
            &child(path, "execution_time"),
            ctx,
            out,
        );
        check_ppbc_instruction(
            self.id,
            self.power_profile_id,
            self.sequence_container_id,
            self.power_sequence_id,
            false,
            path,
            ctx,
            out,
        );
    }
}

impl Validate for ppbc::StartInterruptionInstruction {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_scheduled(
            self.execution_time,
            &child(path, "execution_time"),
            ctx,
            out,
        );
        check_ppbc_instruction(
            self.id,
            self.power_profile_id,
            self.sequence_container_id,
            self.power_sequence_id,
            true,
            path,
            ctx,
            out,
        );
    }
}

impl Validate for ppbc::EndInterruptionInstruction {
    fn validate_at(&self, path: &str, ctx: &Context<'_>, out: &mut Report) {
        check_scheduled(
            self.execution_time,
            &child(path, "execution_time"),
            ctx,
            out,
        );
        check_ppbc_instruction(
            self.id,
            self.power_profile_id,
            self.sequence_container_id,
            self.power_sequence_id,
            true,
            path,
            ctx,
            out,
        );
    }
}

fn check_duplicate_instruction(id: Id, path: &str, ctx: &Context<'_>, out: &mut Report) {
    if ctx.instructions.contains(&id) {
        out.push_related(
            rules::DUPLICATE_INSTRUCTION_ID,
            &child(path, "id"),
            format!("instruction {id} has already been sent on this session"),
            [id],
        );
    }
}
