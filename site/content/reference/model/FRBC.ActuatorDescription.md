+++
title = "FRBC.ActuatorDescription"
description = "The FRBC.ActuatorDescription type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `diagnostic_label` | `string` | no | Human readable name/description for the actuator. This element is only intended for diagnostic purposes and not for HMI applications. |
| `id` | `ID` | **yes** | ID of the Actuator. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `operation_modes` | `FRBC.OperationMode[]` | **yes** | Provided `FRBC.OperationModes` associated with this actuator |
| `supported_commodities` | `Commodity[]` | **yes** | List of all supported Commodities. |
| `timers` | `Timer[]` | **yes** | List of Timers associated with this actuator |
| `transitions` | `Transition[]` | **yes** | Possible transitions between `FRBC.OperationModes` associated with this actuator. |
