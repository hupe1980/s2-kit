+++
title = "PowerForecastValue"
description = "Specifies the expected power value for a specific commodity/quantitiy."
[extra]
generated = true
+++

Specifies the expected power value for a specific commodity/quantitiy.

This object describes the expected power value with limits and, if you want, confidence intervals. The duration for which it is valid is specified in the containing `PowerForecastElement`.

| Field | Type | Required | Description |
|---|---|---|---|
| `commodity_quantity` | `CommodityQuantity` | **yes** | The power quantity the value refers to. |
| `value_expected` | `float` | **yes** | The expected power value. |
| `value_lower_68PPR` | `float` | no | The lower boundary of the range with 68% certainty the power value is in it. |
| `value_lower_95PPR` | `float` | no | The lower boundary of the range with 95% certainty the power value is in it. |
| `value_lower_limit` | `float` | no | The lower boundary of the range with 100% certainty the power value is in it. |
| `value_upper_68PPR` | `float` | no | The upper boundary of the range with 68% certainty the power value is in it. |
| `value_upper_95PPR` | `float` | no | The upper boundary of the range with 95% certainty the power value is in it. |
| `value_upper_limit` | `float` | no | The upper boundary of the range with 100% certainty the power value is in it. |
