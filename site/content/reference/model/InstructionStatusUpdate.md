+++
title = "InstructionStatusUpdate"
description = "Message sent to indicate the status of an instruction."
[extra]
generated = true
+++

Message sent to indicate the status of an instruction.

Used by the RM to indicate the status of instructions it receives. RMs should send these for every instruction they receive until the instruction has either succeeded (indicated by `SUCCEEDED`) or revoked/failed (indicated by `REVOKED`, `REJECTED` or `ABORTED`).

| Field | Type | Required | Description |
|---|---|---|---|
| `instruction_id` | `ID` | **yes** | ID of this instruction (as provided by the CEM). |
| `message_id` | `ID` | **yes** | ID of this message. |
| `message_type` | `string` | **yes** | The string `"InstructionStatusUpdate"`. |
| `status_type` | `InstructionStatus` | **yes** | Present status of this instruction. |
| `timestamp` | `string` | **yes** | Timestamp when status_type has changed the last time. |
