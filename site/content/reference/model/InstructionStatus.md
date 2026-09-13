+++
title = "InstructionStatus"
description = "Indicates the current status of an instruction."
[extra]
generated = true
+++

Indicates the current status of an instruction.

Used by the RM to indicate the status of instructions it receives to the CEM in an `InstructionStatusUpdate`. RMs don't necessarily need to inform the CEM of all the intermediary steps (i.e. `NEW` -> `ACCEPTED` -> `STARTED` -> `SUCCEEDED`), but they should always at least send `SUCCEEDED` when an instruction has been successfully executed.


| Value | Description |
|---|---|
| `ABORTED` | Instruction was aborted. |
| `ACCEPTED` | Instruction has been accepted. |
| `NEW` | Instruction was newly created. |
| `REJECTED` | Instruction was rejected. |
| `REVOKED` | Instruction was revoked. |
| `STARTED` | Instruction was executed. |
| `SUCCEEDED` | Instruction finished successfully. |
