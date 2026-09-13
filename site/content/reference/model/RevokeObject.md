+++
title = "RevokeObject"
description = "The RevokeObject type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `object_id` | `ID` | **yes** | The ID of object that needs to be revoked |
| `object_type` | `RevokableObjects` | **yes** | The type of object that needs to be revoked |
