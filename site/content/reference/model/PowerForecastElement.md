+++
title = "PowerForecastElement"
description = "Specifies power forecast values valid for a duration."
[extra]
generated = true
+++

Specifies power forecast values valid for a duration.

| Field | Type | Required | Description |
|---|---|---|---|
| `duration` | `Duration` | **yes** | Duration of this `PowerForecastElement`. |
| `power_values` | `PowerForecastValue[]` | **yes** | The values of power that are expected for the given period of time. There must be at least one `PowerForecastValue`, and at most one `PowerForecastValue` per `CommodityQuantity`. |
