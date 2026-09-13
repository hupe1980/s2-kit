+++
title = "PowerRange"
description = "The PowerRange type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `commodity_quantity` | `CommodityQuantity` | **yes** | The power quantity the values refer to |
| `end_of_range` | `float` | **yes** | Power value that defines the end of the range. |
| `start_of_range` | `float` | **yes** | Power value that defines the start of the range. |
