+++
title = "PEBC.AllowedLimitRange"
description = "The PEBC.AllowedLimitRange type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `abnormal_condition_only` | `boolean` | **yes** | Indicates if this `PEBC.AllowedLimitRange` may only be used during an abnormal condition |
| `commodity_quantity` | `CommodityQuantity` | **yes** | Type of power quantity this `PEBC.AllowedLimitRange` applies to |
| `limit_type` | `PEBC.PowerEnvelopeLimitType` | **yes** | Indicates if this ranges applies to the upper limit or the lower limit |
| `range_boundary` | `NumberRange` | **yes** | Boundaries of the power range of this `PEBC.AllowedLimitRange`. The CEM is allowed to choose values within this range for the power envelope for the limit as described in limit_type. The start of the range shall be smaller or equal than the end of the range. |
