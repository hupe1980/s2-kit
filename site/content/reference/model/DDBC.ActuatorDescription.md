+++
title = "DDBC.ActuatorDescription"
description = "The DDBC.ActuatorDescription type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `diagnostic_label` | `string` | no | Human readable name/description of the actuator. This element is only intended for diagnostic purposes and not for HMI applications. |
| `id` | `ID` | **yes** | ID of this `DDBC.ActuatorDescription`. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `operation_modes` | `DDBC.OperationMode[]` | **yes** | List of all Operation Modes that are available for this actuator. There shall be at least one `DDBC.OperationMode`. |
| `supported_commodites` | `Commodity[]` | **yes** | Commodities supported by the operation modes of this actuator. There shall be at least one commodity |
| `timers` | `Timer[]` | **yes** | List of Timers associated with Transitions for this Actuator. Can be empty. |
| `transitions` | `Transition[]` | **yes** | List of Transitions between Operation Modes. Shall contain at least one Transition. |
