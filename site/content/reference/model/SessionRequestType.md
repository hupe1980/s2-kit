+++
title = "SessionRequestType"
description = "The SessionRequestType type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++


| Value | Description |
|---|---|
| `RECONNECT` | Please reconnect the WebSocket session. Once reconnected, it starts from scratch with a handshake. |
| `TERMINATE` | Disconnect the session (client can try to reconnecting with exponential backoff) |
