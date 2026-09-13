+++
title = "Transition"
description = "The Transition type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `abnormal_condition_only` | `boolean` | **yes** | Indicates if this Transition may only be used during an abnormal condition (see Clause ) |
| `blocking_timers` | `ID[]` | **yes** | List of IDs of Timers that block this Transition from initiating while at least one of these Timers is not yet finished |
| `from` | `ID` | **yes** | ID of the OperationMode (exact type differs per ControlType) that should be switched from. |
| `id` | `ID` | **yes** | ID of the Transition. Must be unique in the scope of the `OMBC.SystemDescription`, `FRBC.ActuatorDescription` or `DDBC.ActuatorDescription` in which it is used. |
| `start_timers` | `ID[]` | **yes** | List of IDs of Timers that will be (re)started when this transition is initiated |
| `to` | `ID` | **yes** | ID of the OperationMode (exact type differs per ControlType) that will be switched to. |
| `transition_costs` | `float` | no | Absolute costs for going through this Transition in the currency as described in the ResourceManagerDetails. |
| `transition_duration` | `Duration` | no | Indicates the time between the initiation of this Transition, and the time at which the device behaves according to the Operation Mode which is defined in the ‘to’ data element. When no value is provided it is assumed the transition duration is negligible. |
