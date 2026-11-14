+++
title = "S2 Connect"
description = "Discovery, pairing and session initiation in s2-kit: version discovery, the mutual HMAC challenge-response, why LAN trust runs backwards, per-pairing secrets, the two-phase access token commit, and what unpairing ends."
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

## URLs carry no version; the index does

Every URL one endpoint gives another — a DNS-SD `pairingUrl`, a typed URL, the
`initiateSessionUrl` from pairing — is **unversioned**: `https://EVSE1038.local/`, not
`…/v1/`. The server answers `GET /` with the API versions it serves, and the client
"**must** always first determine which version of the API it will use":

```console
$ curl -k https://EVSE1038.local/
["v1"]
```

`router` and `lan_router` serve that index; the clients read it before their first
operation and join everything below the version they chose. A URL that already ends in
`v1/` — what older peers hand out — is used as given.

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
| `Pinned(fingerprint)` | yes, one certificate | every LAN connection *after* pairing |
| `LanPairingOnly` | **no** | the four pairing requests, and nothing else |

`Session::new` refuses the third outright.

A pin is the SHA-256 fingerprint of the certificate, 32 bytes to persist beside the access
token: `Paired::pin()` after a LAN pairing, or the `certificateFingerprint` a client posted
when it is to be the communication server (6B). A handshake passes when the leaf *is* the
pinned certificate, or when a certificate in the presented chain is and the leaf validates
against it — so a server that leaves its root out of the chain still connects.

Fingerprints go on the wire as the specification encodes them, plain lowercase
hexadecimal; one written with colons is still read. And they are the **CA's**: the HMAC
binds the leaf, `certificateFingerprint` names the CA ([erratum E19](@/docs/errata.md)).

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

## One secret per pairing

The access token pairing hands out is generated **per pairing**, from the operating
system's CSPRNG — `S2C §8A` says it "**must**". So the host gives `PairingService` only
where to connect and which CA it uses; the token is minted for each client:

```rust,ignore
let pairing = PairingService::lan(store, endpoint, identity.leaf)
    .as_communication_server(CommunicationServer {
        initiate_session_url: "https://hub.local/".into(), // no version
        ca: Fingerprint::of(&identity.ca),
    });
```

Pairings are stored under a `PairingKey { local, remote }`. Both halves, because "a CEM
can be paired with multiple RMs at the same time", and a WAN endpoint hosting a household's
battery and heat pump holds two pairings with one CEM: keyed by either half alone, one of
them silently overwrites the other.

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
retry, not an unpairing. Neither is any other failure: `Session::initiate` is ready for
the next attempt after every error, with `Session::backoff` saying how long to wait —
except `NoLongerPaired`, which means a person has to pair the devices again.

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

let ws = websocket_router(endpoint.clone(), "/s2", |pairing, socket| async move {
    // `pairing.remote` is the paired peer the token belonged to: which resource this is.
    let mut cem = CemSession::new(CemConfig::default().pre_negotiated(WireProfile::V1_0_0));
    cem.open(Timestamp::now());
    let _ = Driver::new(socket).run(&mut cem, |_event, _session| {}).await;
});
let app = lan_router(endpoint, subnets).merge(ws);
```

`ServerSocket` is an `io::TextTransport`, the same trait the client's `io::WebSocket`
implements, so the driver does not care which end of the connection it is on.

The token travels only over `wss://`: the client refuses a `ws://` `websocketUrl`, and the
dialler refuses to put a bearer on a plaintext socket — "S2 **MUST NOT** be sent over
unencrypted channels". A loopback test opts in explicitly
(`WebSocketOptions::allow_insecure_bearer`).

## Unpairing ends the session, now

"After two nodes have unpaired, the S2 WebSocket connection **MUST** be terminated
immediately." `websocket_router` registers every socket it accepts, and every way a
pairing ends closes it:

- the client calls `unpair`;
- the host calls `SessionService::unpair_remote(key)` — which also answers the next
  `initiateSession` with `NoLongerPaired`, once, so the peer knows to stop retrying;
- a pairing is replaced: the same two nodes pair again, or a Resource Manager pairs with a
  new CEM ("A RM can only be paired with one CEM at a time").

A connection can also simply die. The driver pings every thirty seconds and closes a
connection it has heard nothing from — no message, no pong — for ninety, so a half-open
TCP link becomes a `Closed` event with a reconnection hint, drawn as the specification's
`random(0, min(600 s, 2 s × 2ⁿ))` so a fleet does not come back in lockstep.

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
| `lan_router(endpoint, subnets)` | those **plus** the LAN-only ones, behind the subnet check | `LocalSubnets` — loopback excluded unless `allowing_loopback()`, since behind a reverse proxy every request comes from `127.0.0.1` |
| `websocket_router(endpoint, path, handler)` | the S2 WebSocket, behind the communication token | a handler |

A WAN endpoint mounts `router` and the absent routes answer `404`, which is what the
specification recommends. A LAN endpoint mounts `lan_router` and cannot do so without
supplying the subnets. Choosing which you are is your decision; forgetting the check is
not one of the available mistakes.

## The Resource Manager nobody can dial

S2 Connect expects some RMs to be "purely an HTTPS client" — no listening socket, nothing
to connect to. Pairing cannot start by somebody dialling them, so they dial out and wait:
`POST /v1/waitForPairing` is held open by the other endpoint for twenty seconds — inside
the twenty-five the specification allows — and answered the moment there is something to
say, or with `204` when there is not.

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

`204` is "nothing yet, ask again", not a failure, and a queued instruction wakes
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
sessions — including the version index, the mandatory second, the certificate pinning, and
the token rotation.
