+++
title = "S2 Connect"
description = "Discovery, pairing and session initiation in s2-kit: the mutual HMAC challenge-response, why LAN trust runs backwards, the per-node rate limit, and the two-phase access token commit."
weight = 40
+++

S2 itself says nothing about how its messages travel — the standard is deliberately a
*semantic* protocol, and Victron's Venus OS carries the same JSON over D-Bus.
**S2 Connect** is the specification that makes it work over IP: how two nodes find each
other, how a person's trust in both of them becomes a shared secret, and how that secret
becomes a WebSocket.

```text
 discovery          pairing                    session initiation        S2
 ─────────          ───────                    ──────────────────        ──
 _s2connect._tcp    requestPairing             initiateSession           wss://
 or a typed URL     → challenge/response       → replacement token       Authorization:
                    requestConnectionDetails   confirmAccessToken          Bearer …
                    or postConnectionDetails   → websocketUrl + token
                    finalizePairing
```

## Roles are not what you expect

Three independent pairs of roles are in play, and conflating them is the usual source of
confusion:

- **CEM or RM** — what the device *is*.
- **HTTPS client or server** — who dials during pairing.
- **Communication client or server** — who accepts the WebSocket afterwards.

A WAN node is always the communication server; when both are in the LAN, the CEM is. "A
device developed solely for use as an RM in a LAN setup will never function as a
communication server."

## Trust runs backwards in a LAN, on purpose

In a LAN there is no certificate authority, and there never will be one for
`EVSE1038.local`. A heat pump discovered over DNS-SD answers with a certificate it minted
itself, and no public PKI will ever say anything useful about it.

S2 Connect's answer is not to weaken TLS but to move the authentication elsewhere: the
**pairing token** the user carried from one device to the other, mixed with the server's
**leaf certificate fingerprint** to produce the challenge response.

That inverts the usual order. The certificate is not verified and then used; it is used,
and then — if the HMAC comes out right — trusted:

> in case of a local server, the TLS certificate fingerprint is part of the challenge. So
> if the challenge succeeds, the certificate fingerprint is correct, and the certificate
> can be trusted. The client **must** pin the self-signed CA (root) certificate.

So exactly one operation in this crate talks to a server it has not authenticated — LAN
pairing, whose entire purpose is finding out whether that server is the right one — and
`TlsPolicy` makes that structural:

| Policy | Authenticates? | For |
|---|---|---|
| `Web` | yes, the platform's trust store | a WAN endpoint |
| `PinnedCa` | yes, one certificate | every LAN connection *after* pairing |
| `LanPairingOnly` | **no** | the four pairing requests, and nothing else |

`Session::new` refuses the third outright.

## The exchange is mutual, and a round trip apart

This is the part implementations get wrong. The client challenges first and the server
answers in the same round trip; the server's challenge is answered one request *later*.

| Step | Who | What |
|---|---|---|
| 1 | client | `requestPairing` carrying `clientHmacChallenge` **only** — no response |
| 2–3 | server | computes `R_c`, generates `C_s` and an attempt id; answers after a mandatory one-second delay |
| 4 | client | checks `R_c` — where a man in the middle dies, and where the client learns it may pin the CA |
| 5 | client | computes `R_s` |
| 6 | client | `requestConnectionDetails` **or** `postConnectionDetails`, carrying `R_s` |
| 7 | server | checks `R_s`; a mismatch fails the attempt outright |
| 8 | client | `finalizePairing` |

An implementation where the client answers its own challenge proves nothing at all.

## Brute force, and what the rate limit does not cover

The server must delay its answer by at least a second and handle attempts for a given node
sequentially. s2-kit puts both in the **state machine** rather than a request handler:
`poll_response` returns `None` until the second has elapsed, and a *refused* attempt
consumes the rate-limit slot exactly as a successful one does — which is the only thing
that makes the limit worth having.

Worth knowing, though: those rules bound **online** guessing only. `requestPairing`
answers with `HMAC(C_c, T ‖ F)` for a challenge the caller chose, and `F` is public — so
one reachable request is enough to attack the token offline. A four-character dynamic
token is 14.7 million candidates, which is seconds of GPU time. Prefer longer tokens than
the minimum; this is recorded as [erratum E18](@/docs/errata.md).

## Session initiation is a two-phase commit

An access token is valid **one time**. A naive exchange — present it, server burns it and
returns a new one — loses the pairing outright if that reply never arrives.

The trick is which token authenticates which request:

- `initiateSession` is authenticated with the token you hold, and returns a replacement.
- `confirmAccessToken` is authenticated with the **replacement**. Presenting it *is* the
  proof you stored it, so the server needs no separate assertion.

Until that confirmation, **both** tokens open the door. A client that crashed in between
retries with whichever it has, and the store keeps the ordered list the specification asks
for. Nobody has to re-pair the devices by hand.

The confirmation has its own deadline of fifteen seconds. A late one is refused and
changes nothing, so the token the client still holds keeps working: a slow network is a
retry, not an unpairing.

## The last mile: a token the server keeps

`confirmAccessToken` hands back a `websocketUrl` and a `websocketToken`, and it is easy to
stop there. A server that returns a bearer and keeps no copy of it has authorised nothing:
the upgrade that follows has nothing to compare the `Authorization` header against.

So `s2-kit` keeps it and spends it. The token is valid for **one** connection and thirty
seconds; a second attempt with the same one is a `401`.

```rust,ignore
use s2_kit::connect::server::websocket_router;
use s2_kit::io::Driver;
use s2_kit::prelude::*;

let ws = websocket_router(endpoint.clone(), "/s2", |node, socket| async move {
    // `node` is the paired peer the token belonged to: which resource this is.
    let mut cem = CemSession::new(CemConfig::default().pre_negotiated(WireProfile::V1_0_0));
    cem.open(Timestamp::now());
    let _ = Driver::new(socket).run(&mut cem, |_event, _session| {}).await;
});
let app = lan_router(endpoint, subnets).merge(ws);
```

`ServerSocket` is an `io::TextTransport`, the same trait the client's `io::WebSocket`
implements, so the driver does not care which end of the connection it is on.

On the client side, dial with the policy pairing produced:

```rust,ignore
let socket = WebSocket::connect_with_policy(
    &credentials.websocket_url,
    Some(credentials.websocket_token.as_str()),
    &paired.policy(),          // the pinned CA, not the public PKI
).await?;
```

`WebSocket::connect` verifies against the platform trust store, which is right for a WAN
endpoint and useless for a LAN one: no public CA has heard of the certificate a hub minted
for itself. Dialling a LAN endpoint with it fails rather than quietly succeeding against
the wrong server.

## Three routers, not one with flags

`GET /v1/nodes` tells whoever asks the brand, model and role of every flexible device in
the building, and `/preparePairing` makes a device display its pairing code. Neither
carries a bearer: the subnet *is* the access control. And an RM in a LAN never accepts a
WebSocket at all, so it should not have to opt *out* of serving one.

| Builder | Serves | Needs |
|---|---|---|
| `router(endpoint)` | the authenticated pairing and session-initiation operations | nothing |
| `lan_router(endpoint, subnets)` | those **plus** the LAN-only ones, behind the subnet check | `LocalSubnets` |
| `websocket_router(endpoint, path, handler)` | the S2 WebSocket, behind the communication token | a handler |

A WAN endpoint mounts `router` and the absent routes answer `404`, which is what the
specification recommends. A LAN endpoint mounts `lan_router` and cannot do so without
supplying the subnets. Choosing which you are is your decision; forgetting the check is
not one of the available mistakes.

## The Resource Manager nobody can dial

S2 Connect expects some RMs to be "purely an HTTPS client" — no listening socket, nothing
to connect to. Pairing cannot start by somebody dialling them, so they dial out and wait:
`POST /v1/waitForPairing` is held open by the other endpoint for up to twenty-five seconds
and answered the moment there is something to say.

```rust,ignore
// On the device with no server:
loop {
    for instruction in client.wait_for_pairing(&mine).await? {
        if instruction.action == WaitAction::RequestPairing {
            // start `Pairing::run` now
        }
    }
}

// On the endpoint, when a person presses the button:
endpoint.instruct_waiting_node(node, WaitAction::RequestPairing);
```

An empty answer is "nothing yet, ask again", not a failure, and a queued instruction wakes
a waiting request rather than letting it run out the hold. `Endpoint::waiting_nodes` is
what a user interface lists — the counterpart of `GET /v1/nodes` for clients with no
server to browse.

## Which cryptography does the TLS use?

Yours, if you have already chosen one. `rustls` takes its algorithms from a
`CryptoProvider`, and a library that installs one over the application's choice leaves that
application with two crypto stacks and whichever won a race. `s2-kit` reads
`CryptoProvider::get_default()` first and only falls back to a bundled provider.

It also never lets `rustls` pick for itself. `ClientConfig::builder()` resolves the
provider implicitly from crate features and **panics** when that is empty or ambiguous —
an application that has already installed `aws-lc-rs` would panic against a build carrying
`tls-ring`. Every configuration this crate builds names its provider instead, including
the one behind `WebSocket::connect`. With no provider at all you get an `Error` naming the
two features and `install_default`.

| Feature | Backend | Pick it for |
|---|---|---|
| `tls-ring` *(default)* | `ring` 0.17 | Cross-compiling without `cmake` or a C toolchain — what a gateway build needs. Maintained by the `rustls` team; the unmaintained advisory covers only versions before 0.17. |
| `tls-aws-lc-rs` | `aws-lc-rs` | `rustls`'s own default: FIPS-certifiable, faster, post-quantum key exchange. Costs a vendored BoringSSL and a `cmake` build. |

Only one is ever linked, and CI asserts it — running the whole end-to-end pairing suite on
each. Before reaching for post-quantum here: S2 Connect rotates the access token on
**every** session initiation, so a handshake recorded today and broken in a decade yields a
credential that was retired the same afternoon.

## Trying it

```console
$ cargo run --example connect_pair --features connect-client,connect-server
```

That pairs a charger with a hub over a real TLS connection on loopback, then initiates two
sessions — including the mandatory second, the certificate pinning, and the token
rotation.
