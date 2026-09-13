+++
title = "PEBC.PowerEnvelopeElement"
description = "The PEBC.PowerEnvelopeElement type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `duration` | `Duration` | **yes** | The duration of the element |
| `lower_limit` | `float` | **yes** | Lower power limit according to the commodity_quantity of the containing `PEBC.PowerEnvelope`. The lower_limit must be smaller or equal to the upper_limit. The Resource Manager is requested to keep the power values for the given commodity quantity equal to or above the lower_limit. The lower_limit shall be in accordance with the constraints provided by the Resource Manager through any `PEBC.AllowedLimitRange` with limit_type LOWER_LIMIT. |
| `upper_limit` | `float` | **yes** | Upper power limit according to the commodity_quantity of the containing `PEBC.PowerEnvelope`. The lower_limit must be smaller or equal to the upper_limit. The Resource Manager is requested to keep the power values for the given commodity quantity equal to or below the upper_limit. The upper_limit shall be in accordance with the constraints provided by the Resource Manager through any `PEBC.AllowedLimitRange` with limit_type UPPER_LIMIT. |
