+++
title = "PEBC.PowerConstraints"
description = "The PEBC.PowerConstraints type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `allowed_limit_ranges` | `PEBC.AllowedLimitRange[]` | **yes** | The actual constraints. There shall be at least one `PEBC.AllowedLimitRange` for the UPPER_LIMIT and at least one AllowedLimitRange for the LOWER_LIMIT. It is allowed to have multiple `PEBC.AllowedLimitRange` objects with identical CommodityQuantities and LimitTypes. |
| `consequence_type` | `PEBC.PowerEnvelopeConsequenceType` | **yes** | Type of consequence of limiting power |
| `id` | `ID` | **yes** | Identifier of this `PEBC.PowerConstraints`. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `valid_from` | `string` | **yes** | Moment this `PEBC.PowerConstraints` start to be valid |
| `valid_until` | `string` | no | Moment until this `PEBC.PowerConstraints` is valid. If valid_until is not present, there is no determined end time of this `PEBC.PowerConstraints`. |
