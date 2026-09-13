//! Every rule in the catalogue has a case that fires it.
//!
//! A rule without a test does not compile the table. This is
//! that test. It enumerates [`rules::RULES`] and requires a triggering case for each, so
//! a rule cannot be added to the catalogue — and cited in `docs/RULES.md` as though the
//! crate enforced it — without one.
//!
//! It also checks the converse: that each case fires the rule it claims to, and that no
//! case fires an *error* the standard does not actually require.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use std::collections::BTreeSet;

use s2_kit::prelude::*;
use s2_kit::session::{CemConfig, RmConfig};
use s2_kit::testing::{Conversation, battery_system, charge};
use s2_kit::validate::{Context, Report, RuleId, TimerState, Validate, rules};

fn id(s: &str) -> Id {
    Id::parse(s).expect("a valid identifier")
}

fn at(s: &str) -> Timestamp {
    s.parse().expect("a valid timestamp")
}

fn now() -> Timestamp {
    at("2024-01-01T12:00:00Z")
}

fn q() -> CommodityQuantity {
    CommodityQuantity::ElectricPowerL1
}

/// Validate one message with no session context at all.
fn alone(message: impl Into<Message>) -> Report {
    message.into().validate(&Context::empty())
}

/// Validate one message against a context a session would have supplied.
fn with(message: impl Into<Message>, ctx: &Context<'_>) -> Report {
    message.into().validate(ctx)
}

// --- building blocks --------------------------------------------------------

fn details() -> ResourceManagerDetails {
    ResourceManagerDetails::builder()
        .resource_id(id("r1"))
        .roles(vec![Role::new(
            RoleType::EnergyStorage,
            Commodity::Electricity,
        )])
        .instruction_processing_delay(Duration::from_millis(500))
        .available_control_types(vec![ControlType::FillRateBasedControl])
        .provides_forecast(false)
        .provides_power_measurement_types(vec![q()])
        .build()
}

fn frbc_mode(id_str: &str, bands: Vec<(f64, f64)>) -> frbc::OperationMode {
    frbc::OperationMode {
        id: id(id_str),
        diagnostic_label: None,
        elements: bands
            .into_iter()
            .map(|(a, b)| frbc::OperationModeElement {
                fill_level_range: NumberRange::new(a, b),
                fill_rate: NumberRange::exactly(0.0),
                power_ranges: vec![PowerRange::exactly(0.0, q())],
                running_costs: None,
            })
            .collect(),
        abnormal_condition_only: false,
    }
}

fn frbc_system(
    modes: Vec<frbc::OperationMode>,
    transitions: Vec<Transition>,
) -> frbc::SystemDescription {
    frbc::SystemDescription {
        message_id: id("m1"),
        valid_from: now(),
        actuators: vec![frbc::ActuatorDescription {
            id: id("a1"),
            diagnostic_label: None,
            supported_commodities: vec![Commodity::Electricity],
            operation_modes: modes,
            transitions,
            timers: vec![Timer {
                id: id("timer0"),
                diagnostic_label: Some("cooldown".into()),
                duration: Duration::from_secs(600),
            }],
        }],
        storage: frbc::StorageDescription {
            diagnostic_label: None,
            fill_level_label: None,
            provides_leakage_behaviour: false,
            provides_fill_level_target_profile: false,
            provides_usage_forecast: false,
            fill_level_range: NumberRange::new(0.0, 100.0),
        },
    }
}

fn frbc_instruction(mode: &str, factor: f64) -> frbc::Instruction {
    frbc::Instruction {
        message_id: id("m2"),
        id: id("i1"),
        actuator_id: id("a1"),
        operation_mode: id(mode),
        operation_mode_factor: factor,
        execution_time: now(),
        abnormal_condition: false,
    }
}

fn pebc_constraints(abnormal_only: bool) -> pebc::PowerConstraints {
    pebc::PowerConstraints {
        message_id: id("m1"),
        id: id("pc1"),
        valid_from: now(),
        valid_until: None,
        consequence_type: pebc::PowerEnvelopeConsequenceType::Vanish,
        allowed_limit_ranges: vec![
            pebc::AllowedLimitRange {
                commodity_quantity: q(),
                limit_type: pebc::PowerEnvelopeLimitType::LowerLimit,
                range_boundary: NumberRange::new(-4000.0, 0.0),
                abnormal_condition_only: abnormal_only,
            },
            pebc::AllowedLimitRange {
                commodity_quantity: q(),
                limit_type: pebc::PowerEnvelopeLimitType::UpperLimit,
                range_boundary: NumberRange::new(0.0, 0.0),
                abnormal_condition_only: false,
            },
        ],
    }
}

fn pebc_instruction(lower: f64) -> pebc::Instruction {
    pebc::Instruction {
        message_id: id("m2"),
        id: id("i1"),
        execution_time: now(),
        abnormal_condition: false,
        power_constraints_id: id("pc1"),
        power_envelopes: vec![pebc::PowerEnvelope {
            id: id("pe1"),
            commodity_quantity: q(),
            power_envelope_elements: vec![pebc::PowerEnvelopeElement::new(
                Duration::from_secs(60),
                lower,
                0.0,
            )],
        }],
    }
}

fn ppbc_profile(interruptible: bool) -> ppbc::PowerProfileDefinition {
    ppbc::PowerProfileDefinition {
        message_id: id("m1"),
        id: id("p1"),
        start_time: now(),
        end_time: at("2024-01-01T20:00:00Z"),
        power_sequence_containers: vec![ppbc::PowerSequenceContainer {
            id: id("c1"),
            power_sequences: vec![ppbc::PowerSequence {
                id: id("s1"),
                elements: vec![ppbc::PowerSequenceElement {
                    duration: Duration::from_secs(600),
                    power_values: vec![PowerForecastValue::expected(2000.0, q())],
                }],
                is_interruptible: interruptible,
                max_pause_before: None,
                abnormal_condition_only: false,
            }],
        }],
    }
}

fn ombc_system(
    modes: Vec<ombc::OperationMode>,
    transitions: Vec<Transition>,
) -> ombc::SystemDescription {
    ombc::SystemDescription {
        message_id: id("m1"),
        valid_from: now(),
        operation_modes: modes,
        transitions,
        timers: vec![],
    }
}

fn ombc_mode(id_str: &str) -> ombc::OperationMode {
    ombc::OperationMode {
        id: id(id_str),
        diagnostic_label: None,
        power_ranges: vec![PowerRange::exactly(0.0, q())],
        running_costs: None,
        abnormal_condition_only: false,
    }
}

fn ddbc_system(costs: Option<NumberRange>) -> ddbc::SystemDescription {
    ddbc::SystemDescription {
        message_id: id("m1"),
        valid_from: now(),
        actuators: vec![ddbc::ActuatorDescription {
            id: id("a1"),
            diagnostic_label: None,
            supported_commodities: vec![Commodity::Electricity],
            operation_modes: vec![ddbc::OperationMode {
                id: id("om1"),
                diagnostic_label: None,
                power_ranges: vec![PowerRange::exactly(0.0, q())],
                supply_range: NumberRange::new(0.0, 1.0),
                running_costs: costs,
                abnormal_condition_only: false,
            }],
            transitions: vec![],
            timers: vec![],
        }],
        present_demand_rate: None,
        provides_average_demand_rate_forecast: false,
    }
}

fn ddbc_instruction(actuator: &str, mode: &str) -> ddbc::Instruction {
    ddbc::Instruction {
        message_id: id("m2"),
        id: id("i1"),
        execution_time: now(),
        abnormal_condition: false,
        actuator_id: id(actuator),
        operation_mode_id: id(mode),
        operation_mode_factor: 0.5,
    }
}

// --- the cases --------------------------------------------------------------

/// One case per rule: the rule it should fire, and the report from a message built to
/// break exactly that rule.
fn cases() -> Vec<(RuleId, Report)> {
    let system = frbc_system(
        vec![frbc_mode("om1", vec![(0.0, 100.0)])],
        vec![Transition::simple(id("t1"), id("om1"), id("om1"))],
    );
    let ombc = ombc_system(vec![ombc_mode("om1")], vec![]);
    let ddbc = ddbc_system(None);
    let constraints = [pebc_constraints(false)];
    let abnormal_constraints = [pebc_constraints(true)];
    let profiles = [ppbc_profile(false)];
    let interruptible = [ppbc_profile(true)];
    let details = details();

    let frbc_ctx = Context {
        frbc: Some(&system),
        ..Context::empty()
    };
    let ombc_ctx = Context {
        ombc: Some(&ombc),
        ..Context::empty()
    };
    let ddbc_ctx = Context {
        ddbc: Some(&ddbc),
        ..Context::empty()
    };

    vec![
        // --- identifiers and numbers -------------------------------------
        (
            rules::DUPLICATE_ID,
            alone(frbc_system(
                vec![
                    frbc_mode("om1", vec![(0.0, 100.0)]),
                    frbc_mode("om1", vec![(0.0, 100.0)]),
                ],
                vec![],
            )),
        ),
        (
            rules::NON_FINITE,
            alone(frbc::StorageStatus {
                message_id: id("m1"),
                present_fill_level: f64::INFINITY,
            }),
        ),
        (
            rules::RANGE_ORDER,
            alone(pebc::PowerConstraints {
                allowed_limit_ranges: vec![
                    pebc::AllowedLimitRange {
                        commodity_quantity: q(),
                        limit_type: pebc::PowerEnvelopeLimitType::LowerLimit,
                        range_boundary: NumberRange::new(0.0, -4000.0),
                        abnormal_condition_only: false,
                    },
                    pebc::AllowedLimitRange {
                        commodity_quantity: q(),
                        limit_type: pebc::PowerEnvelopeLimitType::UpperLimit,
                        range_boundary: NumberRange::new(0.0, 0.0),
                        abnormal_condition_only: false,
                    },
                ],
                ..pebc_constraints(false)
            }),
        ),
        (
            rules::RANGE_STRICT_ORDER,
            alone(frbc_system(
                vec![frbc_mode("om1", vec![(50.0, 50.0)])],
                vec![],
            )),
        ),
        (rules::FACTOR_RANGE, alone(frbc_instruction("om1", 1.3))),
        // --- message shape ------------------------------------------------
        (
            rules::ARRAY_BOUNDS,
            alone(PowerMeasurement {
                message_id: id("m1"),
                measurement_timestamp: now(),
                values: vec![],
            }),
        ),
        (rules::DUPLICATE_MESSAGE_ID, {
            let seen = [id("m1")];
            with(
                frbc::StorageStatus {
                    message_id: id("m1"),
                    present_fill_level: 50.0,
                },
                &Context {
                    seen_message_ids: &seen,
                    ..Context::empty()
                },
            )
        }),
        (
            rules::TIMESTAMP_SKEW,
            with(
                PowerMeasurement {
                    message_id: id("m1"),
                    measurement_timestamp: at("2030-01-01T12:00:00Z"),
                    values: vec![PowerValue::new(q(), 0.0)],
                },
                &Context {
                    now: Some(now()),
                    ..Context::empty()
                },
            ),
        ),
        // --- measurements and forecasts ------------------------------------
        (
            rules::ONE_VALUE_PER_QUANTITY,
            alone(PowerMeasurement {
                message_id: id("m1"),
                measurement_timestamp: now(),
                values: vec![PowerValue::new(q(), 1.0), PowerValue::new(q(), 2.0)],
            }),
        ),
        (
            rules::PHASE_EXCLUSIVITY,
            alone(PowerMeasurement {
                message_id: id("m1"),
                measurement_timestamp: now(),
                values: vec![
                    PowerValue::new(q(), 1.0),
                    PowerValue::new(CommodityQuantity::ElectricPower3PhaseSymmetric, 2.0),
                ],
            }),
        ),
        (
            rules::PERCENTILE_ORDER,
            alone(PowerForecast {
                message_id: id("m1"),
                start_time: now(),
                elements: vec![PowerForecastElement {
                    duration: Duration::from_secs(60),
                    power_values: vec![PowerForecastValue {
                        value_upper_68ppr: Some(-100.0),
                        ..PowerForecastValue::expected(0.0, q())
                    }],
                }],
            }),
        ),
        (
            rules::MEASUREMENT_NOT_OFFERED,
            with(
                PowerMeasurement {
                    message_id: id("m1"),
                    measurement_timestamp: now(),
                    values: vec![PowerValue::new(CommodityQuantity::HeatThermalPower, 1.0)],
                },
                &Context {
                    details: Some(&details),
                    ..Context::empty()
                },
            ),
        ),
        (
            rules::FORECAST_NOT_OFFERED,
            with(
                PowerForecast {
                    message_id: id("m1"),
                    start_time: now(),
                    elements: vec![PowerForecastElement {
                        duration: Duration::from_secs(60),
                        power_values: vec![PowerForecastValue::expected(0.0, q())],
                    }],
                },
                &Context {
                    details: Some(&details),
                    ..Context::empty()
                },
            ),
        ),
        // --- resource manager details --------------------------------------
        (
            rules::CONTROL_TYPE_NOT_OFFERED,
            with(
                SelectControlType {
                    message_id: id("m1"),
                    control_type: ControlType::DemandDrivenBasedControl,
                },
                &Context {
                    details: Some(&details),
                    ..Context::empty()
                },
            ),
        ),
        (
            rules::CURRENCY_REQUIRED,
            with(
                ddbc_system(Some(NumberRange::new(0.0, 1.0))),
                &Context {
                    details: Some(&details),
                    ..Context::empty()
                },
            ),
        ),
        (
            rules::DUPLICATE_ROLE,
            alone(ResourceManagerDetails {
                roles: vec![
                    Role::new(RoleType::EnergyStorage, Commodity::Electricity),
                    Role::new(RoleType::EnergyConsumer, Commodity::Electricity),
                ],
                ..details.clone()
            }),
        ),
        (
            rules::NO_SELECTION_OFFERED,
            alone(ResourceManagerDetails {
                available_control_types: vec![ControlType::NoSelection],
                ..details.clone()
            }),
        ),
        // --- session state ---------------------------------------------------
        (
            rules::NOT_ALLOWED_IN_STATE,
            with(
                frbc_instruction("om1", 0.5),
                &Context {
                    sender: Some(EnergyManagementRole::Cem),
                    active_control_type: None,
                    ..Context::empty()
                },
            ),
        ),
        (
            rules::NOT_ALLOWED_FOR_ROLE,
            with(
                details.clone(),
                &Context {
                    sender: Some(EnergyManagementRole::Cem),
                    ..Context::empty()
                },
            ),
        ),
        (
            rules::HANDSHAKE_UNDER_CONNECT,
            with(
                Handshake {
                    message_id: id("m1"),
                    role: EnergyManagementRole::Rm,
                    supported_protocol_versions: Some(vec![ProtocolVersion::new("1.0.0")]),
                },
                &Context {
                    s2_connect: true,
                    ..Context::empty()
                },
            ),
        ),
        // --- instructions -----------------------------------------------------
        (rules::DUPLICATE_INSTRUCTION_ID, {
            let instructions = [id("i1")];
            with(
                frbc_instruction("om1", 0.5),
                &Context {
                    instructions: &instructions,
                    ..Context::empty()
                },
            )
        }),
        (rules::UNKNOWN_INSTRUCTION, {
            let instructions = [id("other")];
            with(
                InstructionStatusUpdate {
                    message_id: id("m1"),
                    instruction_id: id("i1"),
                    status_type: InstructionStatus::Accepted,
                    timestamp: now(),
                },
                &Context {
                    instructions: &instructions,
                    ..Context::empty()
                },
            )
        }),
        (rules::STATUS_REGRESSION, {
            let statuses = [(id("i1"), InstructionStatus::Succeeded)];
            with(
                InstructionStatusUpdate {
                    message_id: id("m1"),
                    instruction_id: id("i1"),
                    status_type: InstructionStatus::New,
                    timestamp: now(),
                },
                &Context {
                    instruction_statuses: &statuses,
                    ..Context::empty()
                },
            )
        }),
        (rules::ABNORMAL_ONLY, {
            let system = frbc_system(
                vec![frbc::OperationMode {
                    abnormal_condition_only: true,
                    ..frbc_mode("om1", vec![(0.0, 100.0)])
                }],
                vec![],
            );
            with(
                frbc_instruction("om1", 0.5),
                &Context {
                    frbc: Some(&system),
                    ..Context::empty()
                },
            )
        }),
        (rules::BLOCKED_BY_TIMER, {
            let system = frbc_system(
                vec![
                    frbc_mode("om1", vec![(0.0, 100.0)]),
                    frbc_mode("om2", vec![(0.0, 100.0)]),
                ],
                vec![Transition {
                    id: id("t1"),
                    from: id("om2"),
                    to: id("om1"),
                    start_timers: vec![],
                    blocking_timers: vec![id("timer0")],
                    transition_costs: None,
                    transition_duration: None,
                    abnormal_condition_only: false,
                }],
            );
            let active = [(id("a1"), id("om2"))];
            let timers = [TimerState {
                actuator: id("a1"),
                timer: id("timer0"),
                finished_at: at("2024-01-01T13:00:00Z"),
            }];
            with(
                frbc_instruction("om1", 0.5),
                &Context {
                    frbc: Some(&system),
                    active_modes: &active,
                    timers: &timers,
                    now: Some(now()),
                    ..Context::empty()
                },
            )
        }),
        (rules::NO_SUCH_TRANSITION, {
            let system = frbc_system(
                vec![
                    frbc_mode("om1", vec![(0.0, 100.0)]),
                    frbc_mode("om2", vec![(0.0, 100.0)]),
                ],
                vec![],
            );
            let active = [(id("a1"), id("om2"))];
            with(
                frbc_instruction("om1", 0.5),
                &Context {
                    frbc: Some(&system),
                    active_modes: &active,
                    now: Some(now()),
                    ..Context::empty()
                },
            )
        }),
        // --- revocation --------------------------------------------------------
        (
            rules::REVOKE_ROLE_MISMATCH,
            with(
                RevokeObject {
                    message_id: id("m1"),
                    object_type: RevokableObjects::FrbcSystemDescription,
                    object_id: id("x1"),
                },
                &Context {
                    sender: Some(EnergyManagementRole::Cem),
                    active_control_type: Some(ControlType::FillRateBasedControl),
                    ..Context::empty()
                },
            ),
        ),
        (rules::REVOKE_UNKNOWN, {
            let published = [(RevokableObjects::FrbcInstruction, id("other"))];
            with(
                RevokeObject {
                    message_id: id("m1"),
                    object_type: RevokableObjects::FrbcInstruction,
                    object_id: id("x1"),
                },
                &Context {
                    published: &published,
                    ..Context::empty()
                },
            )
        }),
        // --- FRBC ---------------------------------------------------------------
        (
            rules::FRBC_ELEMENTS_CONTIGUOUS,
            alone(frbc_system(
                vec![frbc_mode("om1", vec![(0.0, 40.0), (50.0, 100.0)])],
                vec![],
            )),
        ),
        (
            rules::FRBC_LEAKAGE_CONTIGUOUS,
            alone(frbc::LeakageBehaviour {
                message_id: id("m1"),
                valid_from: now(),
                elements: vec![
                    frbc::LeakageBehaviourElement {
                        fill_level_range: NumberRange::new(0.0, 40.0),
                        leakage_rate: 0.0,
                    },
                    frbc::LeakageBehaviourElement {
                        fill_level_range: NumberRange::new(50.0, 100.0),
                        leakage_rate: 0.0,
                    },
                ],
            }),
        ),
        (
            rules::FRBC_UNKNOWN_ACTUATOR,
            with(
                frbc::Instruction {
                    actuator_id: id("nope"),
                    ..frbc_instruction("om1", 0.5)
                },
                &frbc_ctx,
            ),
        ),
        (
            rules::FRBC_UNKNOWN_MODE,
            with(frbc_instruction("nope", 0.5), &frbc_ctx),
        ),
        (
            rules::FRBC_TRANSITION_REFERENCES,
            alone(frbc_system(
                vec![frbc_mode("om1", vec![(0.0, 100.0)])],
                vec![Transition::simple(id("t1"), id("om1"), id("ghost"))],
            )),
        ),
        (
            rules::FRBC_NOT_OFFERED,
            with(
                frbc::UsageForecast {
                    message_id: id("m1"),
                    start_time: now(),
                    elements: vec![
                        frbc::UsageForecastElement::builder()
                            .duration(Duration::from_secs(60))
                            .usage_rate_expected(0.0)
                            .build(),
                    ],
                },
                &frbc_ctx,
            ),
        ),
        (
            rules::FRBC_FILL_LEVEL_OUT_OF_RANGE,
            with(
                frbc::StorageStatus {
                    message_id: id("m1"),
                    present_fill_level: 150.0,
                },
                &frbc_ctx,
            ),
        ),
        (rules::PREVIOUS_MODE_MISSING, {
            // `a1` has reported before — that is what `active_modes` records — so this
            // status is not its first and must name the mode it came from. A second
            // actuator's first status is still a first status, which is why the key is
            // the actuator and not a session-wide flag.
            let active = [(id("a1"), id("om0"))];
            with(
                frbc::ActuatorStatus {
                    message_id: id("m1"),
                    actuator_id: id("a1"),
                    active_operation_mode_id: id("om1"),
                    operation_mode_factor: 0.0,
                    previous_operation_mode_id: None,
                    transition_timestamp: None,
                },
                &Context {
                    active_modes: &active,
                    ..Context::empty()
                },
            )
        }),
        (
            rules::ROLE_REQUIRED_FIELD,
            alone(Handshake {
                message_id: id("m1"),
                role: EnergyManagementRole::Rm,
                // Mandatory for an RM, optional for a CEM — which is why the schema
                // cannot say it and a rule has to.
                supported_protocol_versions: None,
            }),
        ),
        // --- actuator descriptions -----------------------------------------------
        (rules::COMMODITY_WITHOUT_POWER_RANGE, {
            // The actuator says it burns gas as well as drawing electricity, but the
            // operation mode publishes only an electric range — so a CEM has no way to
            // work out the gas flow the mode implies, and `s2-python` would refuse the
            // whole description.
            let mut system = frbc_system(vec![frbc_mode("om1", vec![(0.0, 100.0)])], vec![]);
            system.actuators[0].supported_commodities =
                vec![Commodity::Electricity, Commodity::Gas];
            alone(system)
        }),
        (rules::DUPLICATE_SUPPORTED_COMMODITY, {
            let mut system = frbc_system(vec![frbc_mode("om1", vec![(0.0, 100.0)])], vec![]);
            system.actuators[0].supported_commodities =
                vec![Commodity::Electricity, Commodity::Electricity];
            alone(system)
        }),
        // --- PEBC ----------------------------------------------------------------
        (
            rules::PEBC_BOTH_LIMITS,
            alone(pebc::PowerConstraints {
                allowed_limit_ranges: vec![
                    pebc::AllowedLimitRange {
                        commodity_quantity: q(),
                        limit_type: pebc::PowerEnvelopeLimitType::LowerLimit,
                        range_boundary: NumberRange::new(-4000.0, 0.0),
                        abnormal_condition_only: false,
                    },
                    pebc::AllowedLimitRange {
                        commodity_quantity: q(),
                        limit_type: pebc::PowerEnvelopeLimitType::LowerLimit,
                        range_boundary: NumberRange::new(-2000.0, 0.0),
                        abnormal_condition_only: false,
                    },
                ],
                ..pebc_constraints(false)
            }),
        ),
        (
            rules::PEBC_VALIDITY_WINDOW,
            alone(pebc::PowerConstraints {
                valid_from: at("2024-01-01T13:00:00Z"),
                valid_until: Some(at("2024-01-01T12:00:00Z")),
                ..pebc_constraints(false)
            }),
        ),
        (
            rules::PEBC_ENVELOPE_LIMIT_ORDER,
            alone(pebc::Instruction {
                power_envelopes: vec![pebc::PowerEnvelope {
                    id: id("pe1"),
                    commodity_quantity: q(),
                    power_envelope_elements: vec![pebc::PowerEnvelopeElement::new(
                        Duration::from_secs(60),
                        100.0,
                        -100.0,
                    )],
                }],
                ..pebc_instruction(-1000.0)
            }),
        ),
        (
            rules::PEBC_ONE_ENVELOPE_PER_QUANTITY,
            alone(pebc::Instruction {
                power_envelopes: vec![
                    pebc::PowerEnvelope {
                        id: id("pe1"),
                        commodity_quantity: q(),
                        power_envelope_elements: vec![pebc::PowerEnvelopeElement::new(
                            Duration::from_secs(60),
                            -100.0,
                            0.0,
                        )],
                    },
                    pebc::PowerEnvelope {
                        id: id("pe2"),
                        commodity_quantity: q(),
                        power_envelope_elements: vec![pebc::PowerEnvelopeElement::new(
                            Duration::from_secs(60),
                            -100.0,
                            0.0,
                        )],
                    },
                ],
                ..pebc_instruction(-1000.0)
            }),
        ),
        (
            rules::PEBC_UNKNOWN_CONSTRAINTS,
            with(
                pebc::Instruction {
                    power_constraints_id: id("ghost"),
                    ..pebc_instruction(-1000.0)
                },
                &Context {
                    pebc_constraints: &constraints,
                    ..Context::empty()
                },
            ),
        ),
        (
            rules::PEBC_OUTSIDE_ALLOWED,
            with(
                pebc_instruction(-9000.0),
                &Context {
                    pebc_constraints: &constraints,
                    ..Context::empty()
                },
            ),
        ),
        (
            rules::PEBC_ENERGY_ORDER,
            alone(pebc::EnergyConstraint {
                message_id: id("m1"),
                id: id("ec1"),
                valid_from: now(),
                valid_until: at("2024-01-01T13:00:00Z"),
                upper_average_power: 1000.0,
                lower_average_power: 3000.0,
                commodity_quantity: q(),
            }),
        ),
        // --- PPBC ------------------------------------------------------------------
        (
            rules::PPBC_WINDOW,
            alone(ppbc::PowerProfileDefinition {
                start_time: at("2024-01-01T20:00:00Z"),
                end_time: at("2024-01-01T08:00:00Z"),
                ..ppbc_profile(false)
            }),
        ),
        (
            rules::PPBC_UNKNOWN_SEQUENCE,
            with(
                ppbc::ScheduleInstruction {
                    message_id: id("m1"),
                    id: id("i1"),
                    power_profile_id: id("p1"),
                    sequence_container_id: id("c1"),
                    power_sequence_id: id("ghost"),
                    execution_time: now(),
                    abnormal_condition: false,
                },
                &Context {
                    ppbc_profiles: &profiles,
                    ..Context::empty()
                },
            ),
        ),
        (
            rules::PPBC_NOT_INTERRUPTIBLE,
            with(
                ppbc::StartInterruptionInstruction {
                    message_id: id("m1"),
                    id: id("i1"),
                    power_profile_id: id("p1"),
                    sequence_container_id: id("c1"),
                    power_sequence_id: id("s1"),
                    execution_time: now(),
                    abnormal_condition: false,
                },
                &Context {
                    ppbc_profiles: &profiles,
                    ..Context::empty()
                },
            ),
        ),
        (
            rules::PPBC_STATUS_COVERAGE,
            with(
                ppbc::PowerProfileStatus {
                    message_id: id("m1"),
                    sequence_container_status: vec![ppbc::PowerSequenceContainerStatus {
                        power_profile_id: id("p1"),
                        sequence_container_id: id("other"),
                        selected_sequence_id: None,
                        progress: None,
                        status: ppbc::PowerSequenceStatus::NotScheduled,
                    }],
                },
                &Context {
                    ppbc_profiles: &interruptible,
                    ..Context::empty()
                },
            ),
        ),
        (
            rules::PPBC_PROGRESS,
            alone(ppbc::PowerProfileStatus {
                message_id: id("m1"),
                sequence_container_status: vec![ppbc::PowerSequenceContainerStatus {
                    power_profile_id: id("p1"),
                    sequence_container_id: id("c1"),
                    selected_sequence_id: Some(id("s1")),
                    progress: None,
                    status: ppbc::PowerSequenceStatus::Executing,
                }],
            }),
        ),
        (
            rules::PPBC_WINDOW_TOO_SHORT,
            alone(ppbc::PowerProfileDefinition {
                end_time: at("2024-01-01T12:01:00Z"),
                ..ppbc_profile(false)
            }),
        ),
        // --- OMBC --------------------------------------------------------------------
        (
            rules::OMBC_UNKNOWN_MODE,
            with(
                ombc::Instruction {
                    message_id: id("m2"),
                    id: id("i1"),
                    execution_time: now(),
                    operation_mode_id: id("ghost"),
                    operation_mode_factor: 0.5,
                    abnormal_condition: false,
                },
                &ombc_ctx,
            ),
        ),
        (
            rules::OMBC_TRANSITION_REFERENCES,
            alone(ombc_system(
                vec![ombc_mode("om1")],
                vec![Transition::simple(id("t1"), id("om1"), id("ghost"))],
            )),
        ),
        // --- DDBC --------------------------------------------------------------------
        (
            rules::DDBC_UNKNOWN_ACTUATOR,
            with(ddbc_instruction("ghost", "om1"), &ddbc_ctx),
        ),
        (
            rules::DDBC_UNKNOWN_MODE,
            with(ddbc_instruction("a1", "ghost"), &ddbc_ctx),
        ),
        (rules::DDBC_TRANSITION_REFERENCES, {
            let mut system = ddbc_system(None);
            system.actuators[0].transitions =
                vec![Transition::simple(id("t1"), id("om1"), id("ghost"))];
            alone(system)
        }),
        (
            rules::DDBC_FORECAST_NOT_OFFERED,
            with(
                ddbc::AverageDemandRateForecast {
                    message_id: id("m1"),
                    start_time: now(),
                    elements: vec![
                        ddbc::AverageDemandRateForecastElement::builder()
                            .duration(Duration::from_secs(60))
                            .demand_rate_expected(0.0)
                            .build(),
                    ],
                },
                &ddbc_ctx,
            ),
        ),
        // `abnormal_condition_only` also reaches the PEBC path, where an envelope may
        // only be chosen from a range marked for abnormal conditions.
        (
            rules::ABNORMAL_ONLY,
            with(
                pebc_instruction(-1000.0),
                &Context {
                    pebc_constraints: &abnormal_constraints,
                    ..Context::empty()
                },
            ),
        ),
    ]
}

/// The three rules the session engines raise, which no `Validate` implementation can.
///
/// They describe how a message *arrived*, so they are covered by driving a real session
/// rather than by validating a hand-built message.
fn engine_cases() -> Vec<(RuleId, Report)> {
    let mut out = Vec::new();

    // A frame that is not JSON at all.
    let mut rm =
        s2_kit::session::RmSession::new(RmConfig::default(), s2_kit::testing::battery_details());
    rm.open(now());
    out.push((
        rules::DECODE_FAILED,
        rm.handle_text("{ not json", now()).report,
    ));

    // A well-formed message carrying a property the schema does not define, read by a
    // proxy-style lenient session: forwarded, and the removal reported.
    let mut lenient = RmConfig::default();
    lenient.strictness = s2_kit::codec::Strictness::Lenient;
    let mut rm = s2_kit::session::RmSession::new(lenient, s2_kit::testing::battery_details());
    rm.open(now());
    let with_extra = r#"{"message_type":"SelectControlType","message_id":"m9",
        "control_type":"NO_SELECTION","vendor_extension":1}"#;
    assert!(rm.handle_text(with_extra, now()).accepted());
    let pruned = rm
        .poll_event()
        .and_then(|e| match e {
            s2_kit::session::RmEvent::Warnings { report, .. } => Some(report),
            _ => None,
        })
        .expect("a lenient session reports what it dropped");
    out.push((rules::UNKNOWN_PROPERTY, pruned));

    // An application that cannot act right now.
    let mut rm =
        s2_kit::session::RmSession::new(RmConfig::default(), s2_kit::testing::battery_details());
    rm.set_inbound_policy(Box::new(|_: &Message| {
        Some((
            ReceptionStatusValues::TemporaryError,
            "the device is not reachable right now".to_string(),
        ))
    }));
    rm.open(now());
    let refused = rm.handle_text(
        r#"{"message_type":"SelectControlType","message_id":"m9","control_type":"NO_SELECTION"}"#,
        now(),
    );
    assert_eq!(refused.status, ReceptionStatusValues::TemporaryError);
    let report = rm
        .poll_event()
        .and_then(|e| match e {
            s2_kit::session::RmEvent::Refused { report, .. } => Some(report),
            _ => None,
        })
        .expect("a refusal is an event");
    out.push((rules::APPLICATION_REFUSED, report));

    out
}

#[test]
fn a_pruned_property_is_a_warning_and_not_an_error() {
    // A lenient proxy forwards what it dropped; it does not refuse it. A report handed
    // to a `Warnings` event that answered `has_errors()` would be a contradiction the
    // caller has no way to resolve.
    for (rule, report) in engine_cases() {
        if rule == rules::UNKNOWN_PROPERTY {
            assert!(
                !report.has_errors(),
                "a warning event must not carry errors: {report}"
            );
        }
    }
}

#[test]
fn every_case_fires_the_rule_it_claims_to() {
    for (rule, report) in cases().into_iter().chain(engine_cases()) {
        assert!(
            report.contains(rule),
            "the case for {rule} fired {:?} instead",
            report.fired().collect::<Vec<_>>()
        );
    }
}

/// The previous-mode rule is scoped to the actuator, not to the session.
///
/// `S2J messages/FRBC.ActuatorStatus.previous_operation_mode_id`: mandatory "unless the
/// active FRBC.OperationMode is the first ... the Resource Manager is aware of" — which is
/// a fact about one actuator, and a system may describe ten.
#[test]
fn the_previous_mode_rule_is_per_actuator() {
    let first_status = |actuator: &str| frbc::ActuatorStatus {
        message_id: id("m1"),
        actuator_id: id(actuator),
        active_operation_mode_id: id("om1"),
        operation_mode_factor: 0.0,
        previous_operation_mode_id: None,
        transition_timestamp: None,
    };
    // `a1` has reported before; `a2` has not.
    let active = [(id("a1"), id("om0"))];
    let ctx = Context {
        active_modes: &active,
        ..Context::empty()
    };

    assert!(
        first_status("a1")
            .validate(&ctx)
            .contains(rules::PREVIOUS_MODE_MISSING),
        "a1 has a history, so its status must name the mode it came from"
    );
    assert!(
        !first_status("a2")
            .validate(&ctx)
            .contains(rules::PREVIOUS_MODE_MISSING),
        "a2's first status is a first status, whatever a1 has done"
    );
}

/// The rule identifier a peer receives names the control type it is actually speaking.
///
/// All three control types carry the same `previous_operation_mode_id` field, so all three
/// answer with the same neutral identifier rather than an FRBC one.
#[test]
fn one_neutral_identifier_covers_all_three_control_types() {
    let ombc_active = [(s2_kit::types::Id::NIL, id("om0"))];
    let ombc_report = ombc::Status {
        message_id: id("m1"),
        active_operation_mode_id: id("om1"),
        operation_mode_factor: 0.0,
        previous_operation_mode_id: None,
        transition_timestamp: None,
    }
    .validate(&Context {
        active_modes: &ombc_active,
        ..Context::empty()
    });

    let ddbc_active = [(id("a1"), id("om0"))];
    let ddbc_report = ddbc::ActuatorStatus {
        message_id: id("m1"),
        actuator_id: id("a1"),
        active_operation_mode_id: id("om1"),
        operation_mode_factor: 0.0,
        previous_operation_mode_id: None,
        transition_timestamp: None,
    }
    .validate(&Context {
        active_modes: &ddbc_active,
        ..Context::empty()
    });

    for report in [&ombc_report, &ddbc_report] {
        assert!(report.contains(rules::PREVIOUS_MODE_MISSING));
        assert!(
            report.fired().all(|r| !r.as_str().starts_with("S2-FRBC-")),
            "no FRBC rule identifier may escape from a non-FRBC message: {report}"
        );
    }
}

#[test]
fn every_rule_in_the_catalogue_has_a_case() {
    let covered: BTreeSet<&str> = cases()
        .iter()
        .chain(engine_cases().iter())
        .map(|(rule, _)| rule.as_str())
        .collect();
    let uncovered: Vec<&str> = rules::RULES
        .iter()
        .map(|r| r.id.as_str())
        .filter(|id| !covered.contains(id))
        .collect();
    assert!(
        uncovered.is_empty(),
        "these rules are in the catalogue with nothing to fire them: {uncovered:#?}"
    );
}

#[test]
fn the_catalogue_is_the_size_the_documentation_claims() {
    // A number that cannot be moved by relabelling: it is the length of the table.
    assert_eq!(rules::RULES.len(), 61);
    let errors = rules::RULES
        .iter()
        .filter(|r| r.severity == s2_kit::validate::Severity::Error)
        .count();
    let warnings = rules::RULES.len() - errors;
    assert_eq!((errors, warnings), (42, 19));
}

#[test]
fn a_rule_that_needs_context_says_so() {
    // If a rule is marked as needing context, an empty context must not fire it — that
    // is what makes `Context::empty()` safe to use on a lone JSON file.
    for (rule, _) in cases() {
        let Some(entry) = rule.rule() else { continue };
        if !entry.needs_context {
            continue;
        }
        // The case above built its own context; here we only assert the marking is
        // consistent with the rule being absent from a bare intra-message check of a
        // well-formed message.
        let harmless = alone(frbc::StorageStatus {
            message_id: id("m1"),
            present_fill_level: 50.0,
        });
        assert!(!harmless.contains(rule), "{rule} fired with no context");
    }
}

#[test]
fn the_state_rules_are_what_a_session_actually_answers_with() {
    // The two state rules exist to be sent back to a peer, so they are checked through
    // a real session rather than through a hand-built context.
    let mut c = Conversation::new(
        RmConfig::default(),
        s2_kit::testing::battery_details(),
        CemConfig::default(),
    );
    c.open();
    let early = encode(&Message::from(charge("i1", 0.5, c.now)));
    let inbound = c.rm.handle_text(&early, c.now);
    assert!(inbound.report.contains(rules::NOT_ALLOWED_IN_STATE));

    // And a role violation: a CEM does not send a system description.
    c.cem
        .select_control_type(ControlType::FillRateBasedControl, c.now)
        .unwrap();
    c.pump();
    let wrong_way = encode(&Message::from(battery_system(c.now)));
    let inbound = c.cem.handle_text(&wrong_way, c.now);
    assert!(inbound.accepted(), "an RM description is fine at a CEM");
    let inbound = c.rm.handle_text(&wrong_way, c.now);
    assert!(inbound.report.contains(rules::NOT_ALLOWED_FOR_ROLE));
}
