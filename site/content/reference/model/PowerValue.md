+++
title = "PowerValue"
description = "The PowerValue type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `commodity_quantity` | `CommodityQuantity` | **yes** | The power quantity the value refers to. |
| `value` | `float` | **yes** | The power value, expressed in the unit associated with this object's `commodity_quantity`. |
