+++
title = "FRBC.OperationMode"
description = "The FRBC.OperationMode type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `abnormal_condition_only` | `boolean` | **yes** | Indicates if this `FRBC.OperationMode` may only be used during an abnormal condition |
| `diagnostic_label` | `string` | no | Human readable name/description of the `FRBC.OperationMode`. This element is only intended for diagnostic purposes and not for HMI applications. |
| `elements` | `FRBC.OperationModeElement[]` | **yes** | List of `FRBC.OperationModeElements`, which describe the properties of this `FRBC.OperationMode` depending on the fill_level. The fill_level_ranges of the items in the Array must be contiguous. |
| `id` | `ID` | **yes** | ID of the `FRBC.OperationMode`. Must be unique in the scope of the `FRBC.ActuatorDescription` in which it is used. |
