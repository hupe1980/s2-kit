+++
title = "OMBC.OperationMode"
description = "The OMBC.OperationMode type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `abnormal_condition_only` | `boolean` | **yes** | Indicates if this `OMBC.OperationMode` may only be used during an abnormal condition. |
| `diagnostic_label` | `string` | no | Human readable name/description of the `OMBC.OperationMode`. This element is only intended for diagnostic purposes and not for HMI applications. |
| `id` | `ID` | **yes** | ID of the `OBMC.OperationMode`. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `power_ranges` | `PowerRange[]` | **yes** | The power produced or consumed by this operation mode. The start of each PowerRange is associated with an operation_mode_factor of 0, the end is associated with an operation_mode_factor of 1. In the array there must be at least one PowerRange, and at most one PowerRange per CommodityQuantity. |
| `running_costs` | `NumberRange` | no | Additional costs per second (e.g. wear, services) associated with this operation mode in the currency defined by the ResourceManagerDetails , excluding the commodity cost. The range is expressing uncertainty and is not linked to the operation_mode_factor. |
