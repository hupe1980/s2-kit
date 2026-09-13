+++
title = "DDBC.OperationMode"
description = "The DDBC.OperationMode type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `Id` | `ID` | **yes** | ID of this operation mode. Must be unique in the scope of the `DDBC.ActuatorDescription` in which it is used. |
| `abnormal_condition_only` | `boolean` | **yes** | Indicates if this `DDBC.OperationMode` may only be used during an abnormal condition. |
| `diagnostic_label` | `string` | no | Human readable name/description of the `DDBC.OperationMode`. This element is only intended for diagnostic purposes and not for HMI applications. |
| `power_ranges` | `PowerRange[]` | **yes** | The power produced or consumed by this operation mode. The start of each PowerRange is associated with an operation_mode_factor of 0, the end is associated with an operation_mode_factor of 1. In the array there must be at least one PowerRange, and at most one PowerRange per CommodityQuantity. |
| `running_costs` | `NumberRange` | no | Additional costs per second (e.g. wear, services) associated with this operation mode in the currency defined by the ResourceManagerDetails, excluding the commodity cost. The range is expressing uncertainty and is not linked to the operation_mode_factor. |
| `supply_range` | `NumberRange` | **yes** | The supply rate this `DDBC.OperationMode` can deliver for the CEM to match the demand rate. The start of the NumberRange is associated with an operation_mode_factor of 0, the end is associated with an operation_mode_factor of 1. |
