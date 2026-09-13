+++
title = "FRBC.OperationModeElement"
description = "The FRBC.OperationModeElement type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `fill_level_range` | `NumberRange` | **yes** | The range of the fill level for which this `FRBC.OperationModeElement` applies. The start of the NumberRange shall be smaller than the end of the NumberRange. |
| `fill_rate` | `NumberRange` | **yes** | Indicates the change in fill_level per second. The lower_boundary of the NumberRange is associated with an operation_mode_factor of 0, the upper_boundary is associated with an operation_mode_factor of 1. |
| `power_ranges` | `PowerRange[]` | **yes** | The power produced or consumed by this operation mode. The start of each PowerRange is associated with an operation_mode_factor of 0, the end is associated with an operation_mode_factor of 1. In the array there must be at least one PowerRange, and at most one PowerRange per CommodityQuantity. |
| `running_costs` | `NumberRange` | no | Additional costs per second (e.g. wear, services) associated with this operation mode in the currency defined by the ResourceManagerDetails, excluding the commodity cost. The range is expressing uncertainty and is not linked to the operation_mode_factor. |
