+++
title = "SessionRequest"
description = "The SessionRequest type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `diagnostic_label` | `string` | no | Optional field for a human readible descirption for debugging purposes |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `request` | `SessionRequestType` | **yes** | The type of request |
