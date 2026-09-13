+++
title = "PEBC.PowerEnvelope"
description = "The PEBC.PowerEnvelope type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `commodity_quantity` | `CommodityQuantity` | **yes** | Type of power quantity this `PEBC.PowerEnvelope` applies to |
| `id` | `ID` | **yes** | Identifier of this `PEBC.PowerEnvelope`. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `power_envelope_elements` | `PEBC.PowerEnvelopeElement[]` | **yes** | The elements of this `PEBC.PowerEnvelope`. Shall contain at least one element. Elements must be placed in chronological order. |
