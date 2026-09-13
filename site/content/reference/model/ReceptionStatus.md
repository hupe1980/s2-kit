+++
title = "ReceptionStatus"
description = "Indicates whether a message was received successfully."
[extra]
generated = true
+++

Indicates whether a message was received successfully.

When an RM or CEM receives a message, it should send back a `ReceptionStatus` indication whether the message was received successfully. The exception is that `ReceptionStatus` messages should not be answered themselves by a `ReceptionStatus`. The RM/CEM should send this as soon as the status is known.

`ReceptionStatus` is only meant to indicate successful receipt of a message, and an `OK` status does not necessarily indicate that, for example, an instruction was fully carried out successfully. See `InstructionStatus` for that.

| Field | Type | Required | Description |
|---|---|---|---|
| `diagnostic_label` | `string` | no | Diagnostic label that can be used to provide additional information for debugging. |
| `message_type` | `string` | **yes** | The string `"ReceptionStatus"`. |
| `status` | `ReceptionStatusValues` | **yes** | The reception status of the referred message. |
| `subject_message_id` | `ID` | **yes** | The message this `ReceptionStatus` refers to. |
