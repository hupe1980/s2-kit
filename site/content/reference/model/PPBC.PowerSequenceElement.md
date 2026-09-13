+++
title = "PPBC.PowerSequenceElement"
description = "The PPBC.PowerSequenceElement type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `duration` | `Duration` | **yes** | Duration of the `PPBC.PowerSequenceElement`. |
| `power_values` | `PowerForecastValue[]` | **yes** | The value of power and deviations for the given duration. The array should contain at least one PowerForecastValue and at most one PowerForecastValue per CommodityQuantity. |
