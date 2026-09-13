+++
title = "Timer"
description = "The Timer type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `diagnostic_label` | `string` | no | Human readable name/description of the Timer. This element is only intended for diagnostic purposes and not for HMI applications. |
| `duration` | `Duration` | **yes** | The time it takes for the Timer to finish after it has been started |
| `id` | `ID` | **yes** | ID of the Timer. Must be unique in the scope of the `OMBC.SystemDescription`, `FRBC.ActuatorDescription` or `DDBC.ActuatorDescription` in which it is used. |
