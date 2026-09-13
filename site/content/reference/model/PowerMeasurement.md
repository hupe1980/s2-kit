+++
title = "PowerMeasurement"
description = "Message containing one or more power measurements."
[extra]
generated = true
+++

Message containing one or more power measurements.

This message informs the CEM about power values measured by the RM's device. A well-behaved RM should send `PowerMeasurement`s periodically if possible.

| Field | Type | Required | Description |
|---|---|---|---|
| `measurement_timestamp` | `string` | **yes** | Timestamp when the power values were measured. |
| `message_id` | `ID` | **yes** | ID of this message. |
| `message_type` | `string` | **yes** | The string `"PowerMeasurement"`. |
| `values` | `PowerValue[]` | **yes** | The measured `PowerValue`s. Must contain at least one item, and at most one item per `CommodityQuantity`. |
