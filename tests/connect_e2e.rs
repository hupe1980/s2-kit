//! S2 Connect, end to end, over a real TLS connection on loopback.
//!
//! Everything else in this crate's test suite runs the protocol in memory, which proves
//! the state machines and nothing about the wire. This one starts an actual HTTPS server
//! with an actual self-signed certificate, points an actual `reqwest` client at it, and
//! pairs — so the parts that only exist because there is a socket are exercised too:
//!
//! * the leaf fingerprint the HMAC binds to comes out of a genuine TLS handshake, not a
//!   fixture, which is the one thing an in-memory test cannot check;
//! * the mandatory one-second delay is a real second of wall-clock;
//! * the status codes cross a real HTTP boundary, so a `403` that should have been a `401`
//!   shows up as the wrong recovery rather than as a passing assertion;
//! * the JSON is serialised and parsed by two independent code paths.
//!
//! A LAN pairing is the interesting case, because it is the one where TLS is deliberately
//! unauthenticated until the challenge response says otherwise.

#![cfg(all(feature = "connect-client", feature = "connect-server"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use s2_kit::connect::client::{Pairing, Session};
use s2_kit::connect::proto::*;
use s2_kit::connect::server::{
    Endpoint, LocalSubnets, Memory, PairingService, SessionService, Store, lan_router, router,
    serve, websocket_router,
};
use s2_kit::connect::tls::SelfSignedEndpoint;
use s2_kit::io::Driver;
use s2_kit::prelude::{CemConfig, CemSession, MessageKind, RmConfig, RmEvent, RmSession};
use s2_kit::types::Timestamp;

/// A running endpoint: a CEM in the LAN that Resource Managers pair with.
struct Harness {
    base_url: String,
    store: Arc<Memory>,
    cem_node: NodeId,
    endpoint: Endpoint,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

fn node(id: &str, role: Role, model: &str) -> NodeDescription {
    NodeDescription {
        id: NodeId::parse(id).expect("a legal id"),
        brand: "Acme".into(),
        logo_url: None,
        kind: if role == Role::Cem { "CEM" } else { "EVSE" }.into(),
        model_name: model.into(),
        user_defined_name: None,
        role,
    }
}

fn endpoint_description(deployment: Deployment) -> EndpointDescription {
    EndpointDescription {
        name: Some("Acme home hub".into()),
        logo_url: None,
        deployment: Some(deployment),
    }
}

async fn start() -> Harness {
    let identity = SelfSignedEndpoint::generate(["localhost".into(), "127.0.0.1".into()])
        .expect("a self-signed CA and leaf");
    let leaf = identity.leaf;
    let config = identity.server_config().expect("a server config");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let port = listener.local_addr().expect("a bound address").port();
    let base_url = format!("https://localhost:{port}/v1/");

    let store = Memory::new();
    let cem = node("cem-1", Role::Cem, "Home hub");
    let cem_node = cem.id;
    store.add_node(cem, Some(NodeIdAlias::parse("hub7").unwrap()));

    // This CEM is in the LAN and so is the RM, which makes the CEM the communication
    // server — so it is the side that hands out connection details.
    let details = ConnectionDetails {
        initiate_session_url: base_url.clone(),
        access_token: AccessToken::from_entropy(&[7u8; 32]).unwrap(),
        certificate_fingerprint: [("SHA256".to_string(), identity.leaf.to_hex())]
            .into_iter()
            .collect(),
    };

    let pairing = Arc::new(
        PairingService::lan(store.clone(), endpoint_description(Deployment::Lan), leaf)
            .with_connection_details(details),
    );
    let session = Arc::new(SessionService::new(
        store.clone(),
        format!("wss://localhost:{port}/s2"),
    ));
    let endpoint = Endpoint::new(pairing, session);
    // A LAN endpoint, so the LAN-only routes are mounted — behind the subnet check they
    // require. The tests dial 127.0.0.1, and loopback is always local.
    //
    // `/s2` is merged in as well, so the endpoint is a complete one: the token
    // `confirmAccessToken` hands out is a token something here will actually check.
    let app = lan_router(
        endpoint.clone(),
        LocalSubnets::detect().expect("this host's interfaces"),
    )
    .merge(websocket_router(
        endpoint.clone(),
        "/s2",
        |_node, socket| async move {
            // The manager's side of one S2 session, for as long as the socket lasts.
            let mut cem = CemSession::new(
                CemConfig::default().pre_negotiated(s2_kit::types::WireProfile::V1_0_0),
            );
            cem.open(Timestamp::now());
            let mut driver = Driver::new(socket);
            let _ = driver.run(&mut cem, |_event, _session| {}).await;
        },
    ));

    let (shutdown, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = serve(listener, config, app, async {
            let _ = rx.await;
        })
        .await;
    });

    // Let the accept loop reach its first poll.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    Harness {
        base_url,
        store,
        cem_node,
        endpoint,
        shutdown,
    }
}

impl Harness {
    /// Serve `/s2` over plain HTTP on another port, and return its `ws://` base.
    ///
    /// The same [`Endpoint`], so the same issued-token registry: what this proves is the
    /// route and the bearer check, which TLS neither helps nor hinders.
    async fn serve_websocket(&self) -> String {
        let app = websocket_router(self.endpoint.clone(), "/s2", |_node, socket| async move {
            let mut cem = CemSession::new(
                CemConfig::default().pre_negotiated(s2_kit::types::WireProfile::V1_0_0),
            );
            cem.open(Timestamp::now());
            let mut driver = Driver::new(socket);
            let _ = driver.run(&mut cem, |_event, _session| {}).await;
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let port = listener.local_addr().expect("a bound address").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        format!("ws://127.0.0.1:{port}")
    }
}

fn rm_client(base_url: &str) -> Pairing {
    Pairing::lan(
        base_url,
        node("rm-1", Role::Rm, "Charger 9000"),
        endpoint_description(Deployment::Lan),
    )
    .expect("a pairing client")
}

#[tokio::test]
async fn a_lan_pairing_completes_over_real_tls() {
    let harness = start().await;
    let token = PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap();
    harness.store.open_for_pairing(harness.cem_node, token);

    let code = PairingCode::parse("hub7-A1b2C3d4", TokenKind::Static).unwrap();
    let paired = rm_client(&harness.base_url)
        .run(code, Timestamp::now())
        .await
        .expect("the pairing must complete");

    // The RM learned who it paired with, and the CEM recorded it.
    assert_eq!(paired.pairing.remote.id, harness.cem_node);
    assert_eq!(paired.pairing.remote.model_name, "Home hub");
    let stored = harness
        .store
        .paired(paired.pairing.local)
        .expect("the server stored the pairing");
    assert_eq!(stored.remote.model_name, "Charger 9000");

    // Both LAN, so the CEM is the communication server and supplied the details.
    assert_eq!(paired.pairing.communication_server, Role::Cem);
    assert!(!paired.pairing.details.initiate_session_url.is_empty());

    // And the RM captured a certificate to pin — without it, every later connection to
    // this device would be unauthenticated.
    let identity = paired.identity.as_ref().expect("a captured identity");
    assert!(paired.policy().authenticates());
    // The chain is leaf-then-CA, and what gets pinned is the CA.
    assert_eq!(identity.chain.len(), 2);
    assert_eq!(identity.root, identity.chain[1]);

    let _ = harness.shutdown.send(());
}

#[tokio::test]
async fn a_session_is_initiated_against_the_pinned_certificate() {
    let harness = start().await;
    harness.store.open_for_pairing(
        harness.cem_node,
        PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
    );
    let paired = rm_client(&harness.base_url)
        .run(
            PairingCode::parse("hub7-A1b2C3d4", TokenKind::Static).unwrap(),
            Timestamp::now(),
        )
        .await
        .expect("the pairing must complete");

    // Everything from here runs against the pinned CA, not the pairing policy.
    let policy = paired.policy();
    assert!(policy.authenticates());
    let mut session = Session::new(
        &paired.pairing.details.initiate_session_url,
        &policy,
        paired.pairing.local,
        paired.pairing.remote.id,
        TokenStore::new(paired.pairing.details.access_token.clone()),
    )
    .expect("a session client");

    let credentials = session
        .initiate(Timestamp::now())
        .await
        .expect("session initiation must succeed");
    assert!(credentials.websocket_url.starts_with("wss://"));
    assert_eq!(
        credentials.profile,
        Some(s2_kit::types::WireProfile::V1_0_0)
    );
    assert!(!credentials.is_expired(Timestamp::now()));

    // The token rotated, and the old one is gone on both sides.
    assert_eq!(session.tokens().len(), 1);
    let now_current = session.tokens().current().unwrap();
    assert!(!now_current.verify(&paired.pairing.details.access_token));

    // A second session works too, which is the whole point of rotation being safe.
    let again = session
        .initiate(Timestamp::now())
        .await
        .expect("a second session");
    assert!(again.websocket_url.starts_with("wss://"));

    let _ = harness.shutdown.send(());
}

/// Pair, initiate, connect, and speak S2 — the whole of S2 Connect in one test.
///
/// A token nothing ever checks proves nothing, so this is the step that says the
/// `Authorization` header the client sends is the one the server is holding.
#[tokio::test]
async fn the_communication_token_opens_exactly_one_websocket() {
    let harness = start().await;
    harness.store.open_for_pairing(
        harness.cem_node,
        PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
    );
    let paired = rm_client(&harness.base_url)
        .run(
            PairingCode::parse("hub7-A1b2C3d4", TokenKind::Static).unwrap(),
            Timestamp::now(),
        )
        .await
        .expect("the pairing must complete");

    let mut session = Session::new(
        &paired.pairing.details.initiate_session_url,
        &paired.policy(),
        paired.pairing.local,
        paired.pairing.remote.id,
        TokenStore::new(paired.pairing.details.access_token.clone()),
    )
    .expect("a session client");
    let credentials = session
        .initiate(Timestamp::now())
        .await
        .expect("session initiation must succeed");

    // The server is holding exactly one token, waiting to be spent.
    assert_eq!(harness.endpoint.session().pending_connections(), 1);

    // A bearer the server never issued does not get in.
    assert!(
        harness
            .endpoint
            .session()
            .authorize_connection("not a token this endpoint issued", Timestamp::now())
            .is_none()
    );
    assert_eq!(harness.endpoint.session().pending_connections(), 1);

    // The real one does, once — and identifies which paired node is connecting.
    let node = harness
        .endpoint
        .session()
        .authorize_connection(credentials.websocket_token.as_str(), Timestamp::now())
        .expect("the token the client was handed must open the door");
    assert_eq!(node, paired.pairing.local);
    // "Valid for a single connection": the second attempt is refused.
    assert!(
        harness
            .endpoint
            .session()
            .authorize_connection(credentials.websocket_token.as_str(), Timestamp::now())
            .is_none()
    );
    assert_eq!(harness.endpoint.session().pending_connections(), 0);

    // And now the route itself, over a real HTTP upgrade. This first half is about the
    // header, the 401 and the session that follows; the `wss://` half is below.
    let ws_base = harness.serve_websocket().await;

    // No bearer at all: refused before the upgrade.
    assert!(
        s2_kit::io::WebSocket::connect(&format!("{ws_base}/s2"), None)
            .await
            .is_err(),
        "an unauthenticated upgrade must not be accepted"
    );

    let credentials = session
        .initiate(Timestamp::now())
        .await
        .expect("a second initiation");
    let token = credentials.websocket_token.as_str().to_string();
    let socket = s2_kit::io::WebSocket::connect(&format!("{ws_base}/s2"), Some(&token))
        .await
        .expect("the communication token must open the socket");

    // The manager on the other side is a real `CemSession` on a pre-negotiated profile,
    // so a real `RmSession` here can describe itself and be acknowledged.
    let mut rm = RmSession::new(
        RmConfig::default().pre_negotiated(s2_kit::types::WireProfile::V1_0_0),
        s2_kit::testing::battery_details(),
    );
    rm.open(Timestamp::now());
    let mut driver = Driver::new(socket);
    let mut acked = false;
    for _ in 0..8 {
        driver.step(&mut rm).await.expect("the socket must stay up");
        while let Some(event) = rm.poll_event() {
            if matches!(&event, RmEvent::Acked(handle)
                if handle.kind == MessageKind::ResourceManagerDetails)
            {
                acked = true;
            }
        }
        if acked {
            break;
        }
    }
    assert!(
        acked,
        "the manager behind the WebSocket must acknowledge the resource's details"
    );

    // And the token is spent: the same one will not open a second socket.
    assert!(
        s2_kit::io::WebSocket::connect(&format!("{ws_base}/s2"), Some(&token))
            .await
            .is_err(),
        "a communication token is valid for a single connection"
    );

    let _ = harness.shutdown.send(());
}

/// The LAN case the specification actually describes: `wss://` against the pinned CA.
///
/// A WebSocket client that verifies against a bundled root list cannot reach a conforming
/// LAN endpoint at all, and one that skips verification is not running the protocol.
#[tokio::test]
async fn the_websocket_runs_over_tls_against_the_pinned_certificate() {
    let harness = start().await;
    harness.store.open_for_pairing(
        harness.cem_node,
        PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
    );
    let paired = rm_client(&harness.base_url)
        .run(
            PairingCode::parse("hub7-A1b2C3d4", TokenKind::Static).unwrap(),
            Timestamp::now(),
        )
        .await
        .expect("the pairing must complete");
    let policy = paired.policy();

    let mut session = Session::new(
        &paired.pairing.details.initiate_session_url,
        &policy,
        paired.pairing.local,
        paired.pairing.remote.id,
        TokenStore::new(paired.pairing.details.access_token.clone()),
    )
    .expect("a session client");
    let credentials = session
        .initiate(Timestamp::now())
        .await
        .expect("session initiation must succeed");

    // The URL the endpoint published is the one dialled, verbatim — `wss://`, on the
    // same TLS listener the pairing ran against.
    assert!(credentials.websocket_url.starts_with("wss://"));

    // The public-PKI dialler cannot reach it, and should not be able to: a bundled root
    // list has never heard of a certificate a hub minted for itself this morning.
    assert!(
        s2_kit::io::WebSocket::connect(
            &credentials.websocket_url,
            Some(credentials.websocket_token.as_str()),
        )
        .await
        .is_err(),
        "the public PKI must not vouch for a self-signed LAN certificate"
    );

    let credentials = session
        .initiate(Timestamp::now())
        .await
        .expect("a fresh token, the last one having been spent on a failed dial");
    let socket = s2_kit::io::WebSocket::connect_with_policy(
        &credentials.websocket_url,
        Some(credentials.websocket_token.as_str()),
        &policy,
    )
    .await
    .expect("the pinned CA must be what the WebSocket verifies against");

    let mut rm = RmSession::new(
        RmConfig::default().pre_negotiated(s2_kit::types::WireProfile::V1_0_0),
        s2_kit::testing::battery_details(),
    );
    rm.open(Timestamp::now());
    let mut driver = Driver::new(socket);
    let mut acked = false;
    for _ in 0..8 {
        driver.step(&mut rm).await.expect("the socket must stay up");
        while let Some(event) = rm.poll_event() {
            if matches!(&event, RmEvent::Acked(handle)
                if handle.kind == MessageKind::ResourceManagerDetails)
            {
                acked = true;
            }
        }
        if acked {
            break;
        }
    }
    assert!(acked, "the two engines must talk over the TLS WebSocket");

    // And the pairing-only policy is refused outright, rather than opening an
    // unauthenticated session that looks like it worked.
    let credentials = session.initiate(Timestamp::now()).await.expect("another");
    let refused = s2_kit::io::WebSocket::connect_with_policy(
        &credentials.websocket_url,
        Some(credentials.websocket_token.as_str()),
        &s2_kit::connect::tls::TlsPolicy::LanPairingOnly,
    )
    .await;
    assert!(refused.is_err());

    let _ = harness.shutdown.send(());
}

/// `S2C §Pairing process`: "A CEM can be paired with multiple RMs at the same time. A RM
/// can only be paired with one CEM at a time."
///
/// Nothing in the pairing exchange enforces the second sentence — each attempt succeeds on
/// its own terms — and a device that answers to two energy managers takes two sets of
/// instructions.
#[tokio::test]
async fn a_resource_manager_pairs_with_one_energy_manager_at_a_time() {
    // This endpoint hosts the *RM*, and two different CEMs pair with it in turn.
    let identity = SelfSignedEndpoint::generate(["localhost".into()]).unwrap();
    let leaf = identity.leaf;
    let config = identity.server_config().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("https://localhost:{port}/v1/");

    let store = Memory::new();
    let rm = node("rm-1", Role::Rm, "Charger 9000");
    let rm_node = rm.id;
    store.add_node(rm, Some(NodeIdAlias::parse("evse1").unwrap()));

    // Both LAN, so the *client* (a CEM) is the communication server and posts its own
    // connection details; this endpoint hands out none.
    let pairing = Arc::new(PairingService::lan(
        store.clone(),
        endpoint_description(Deployment::Lan),
        leaf,
    ));
    let session = Arc::new(SessionService::new(
        store.clone(),
        format!("wss://localhost:{port}/s2"),
    ));
    let app = router(Endpoint::new(pairing, session));
    let (shutdown, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = serve(listener, config, app, async {
            let _ = rx.await;
        })
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut paired_ids = Vec::new();
    for which in ["cem-1", "cem-2"] {
        if !paired_ids.is_empty() {
            // The per-node rate limit is real, and `finalizePairing` starts its second.
            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        }
        store.open_for_pairing(
            rm_node,
            PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
        );
        let client = Pairing::lan(
            &base_url,
            node(which, Role::Cem, "Home hub"),
            endpoint_description(Deployment::Lan),
        )
        .unwrap();
        // Both LAN, so the CEM is the communication server and is the side that posts
        // where to connect.
        let own_details = ConnectionDetails {
            initiate_session_url: format!("https://localhost:{port}/v1/"),
            access_token: AccessToken::from_entropy(&[9u8; 32]).unwrap(),
            certificate_fingerprint: [("SHA256".to_string(), leaf.to_hex())]
                .into_iter()
                .collect(),
        };
        let paired = client
            .run_with_entropy(
                PairingCode::parse("evse1-A1b2C3d4", TokenKind::Static).unwrap(),
                Timestamp::now(),
                &[0xC3u8; 32],
                Some(own_details),
            )
            .await
            .unwrap_or_else(|e| panic!("{which} must pair: {e}"));
        paired_ids.push(paired.pairing.local);
    }

    // Exactly one survives, and it is the most recent.
    let remaining = store.paired_with(rm_node);
    assert_eq!(
        remaining.len(),
        1,
        "a Resource Manager must not stay paired with two managers: {remaining:?}"
    );
    assert_eq!(remaining[0], paired_ids[1]);
    assert!(
        store.paired(paired_ids[0]).is_none(),
        "the first CEM is gone"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn the_wrong_pairing_code_is_refused_and_costs_a_second() {
    let harness = start().await;
    harness.store.open_for_pairing(
        harness.cem_node,
        PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
    );

    // The client verifies the server's response in step 4, so a wrong token fails there.
    let started = std::time::Instant::now();
    let error = rm_client(&harness.base_url)
        .run(
            PairingCode::parse("hub7-WRONGTOKEN", TokenKind::Static).unwrap(),
            Timestamp::now(),
        )
        .await
        .expect_err("a wrong token cannot pair");
    assert!(
        matches!(
            error,
            s2_kit::connect::client::Error::Protocol(ConnectError::ChallengeFailed)
        ),
        "{error:?}"
    );
    // The mandatory delay applied even to the failure, which is what bounds guessing.
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(1),
        "the server answered in {:?}, faster than the specification allows",
        started.elapsed()
    );
    assert!(
        harness
            .store
            .paired(NodeId::parse("rm-1").unwrap())
            .is_none()
    );

    let _ = harness.shutdown.send(());
}

#[tokio::test]
async fn a_node_that_is_not_open_for_pairing_says_so_by_name() {
    let harness = start().await;
    // No `open_for_pairing`: the device's button has not been pressed.
    let error = rm_client(&harness.base_url)
        .run(
            PairingCode::parse("hub7-A1b2C3d4", TokenKind::Static).unwrap(),
            Timestamp::now(),
        )
        .await
        .expect_err("there is no token to pair against");

    let s2_kit::connect::client::Error::Http { status, detail, .. } = &error else {
        panic!("expected an HTTP error, got {error:?}");
    };
    assert_eq!(status.0, 400);
    // The named reason, so a user interface can say "press the button on the device"
    // rather than "pairing failed".
    let body: PairingRefused =
        serde_json::from_str(detail.as_deref().expect("a body")).expect("the schema's body");
    assert_eq!(
        body.error_message,
        PairingErrorMessage::NoValidPairingTokenOnPairingServer
    );

    let _ = harness.shutdown.send(());
}

#[tokio::test]
async fn an_unknown_alias_is_not_found_rather_than_generically_refused() {
    let harness = start().await;
    harness.store.open_for_pairing(
        harness.cem_node,
        PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
    );
    let error = rm_client(&harness.base_url)
        .run(
            PairingCode::parse("nosuch-A1b2C3d4", TokenKind::Static).unwrap(),
            Timestamp::now(),
        )
        .await
        .expect_err("no such node");
    let s2_kit::connect::client::Error::Http { detail, .. } = &error else {
        panic!("expected an HTTP error, got {error:?}");
    };
    let body: PairingRefused =
        serde_json::from_str(detail.as_deref().expect("a body")).expect("the schema's body");
    assert_eq!(body.error_message, PairingErrorMessage::NodeNotFound);

    let _ = harness.shutdown.send(());
}

#[tokio::test]
async fn two_customer_energy_managers_will_not_pair() {
    let harness = start().await;
    harness.store.open_for_pairing(
        harness.cem_node,
        PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
    );
    let error = Pairing::lan(
        &harness.base_url,
        node("cem-2", Role::Cem, "Another hub"),
        endpoint_description(Deployment::Lan),
    )
    .unwrap()
    .run(
        PairingCode::parse("hub7-A1b2C3d4", TokenKind::Static).unwrap(),
        Timestamp::now(),
    )
    .await
    .expect_err("a CEM cannot pair with a CEM");

    let s2_kit::connect::client::Error::Http { detail, .. } = &error else {
        panic!("expected an HTTP error, got {error:?}");
    };
    let body: PairingRefused =
        serde_json::from_str(detail.as_deref().expect("a body")).expect("the schema's body");
    assert_eq!(
        body.error_message,
        PairingErrorMessage::InvalidCombinationOfRoles
    );

    let _ = harness.shutdown.send(());
}

#[tokio::test]
async fn a_second_attempt_for_the_same_node_gets_service_unavailable() {
    let harness = start().await;
    harness.store.open_for_pairing(
        harness.cem_node,
        PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
    );

    // Two attempts at once against the same node. One must be told to come back — and
    // with `503`, which is "try again soon", not a failure.
    let code = || PairingCode::parse("hub7-A1b2C3d4", TokenKind::Static).unwrap();
    let first_client = rm_client(&harness.base_url);
    let second_client = Pairing::lan(
        &harness.base_url,
        node("rm-2", Role::Rm, "Another charger"),
        endpoint_description(Deployment::Lan),
    )
    .unwrap();
    let (first, second) = tokio::join!(
        first_client.run(code(), Timestamp::now()),
        second_client.run(code(), Timestamp::now())
    );

    let outcomes = [&first, &second];
    let busy = outcomes
        .iter()
        .filter(|r| {
            matches!(
                r,
                Err(s2_kit::connect::client::Error::Http { status, .. }) if status.is_busy()
            )
        })
        .count();
    assert_eq!(
        busy, 1,
        "exactly one attempt should be rate-limited:\n{first:?}\n{second:?}"
    );
    assert_eq!(
        outcomes.iter().filter(|r| r.is_ok()).count(),
        1,
        "and exactly one should succeed"
    );

    // The refused one is marked retryable, so a client knows to come back.
    for outcome in outcomes {
        if let Err(e) = outcome {
            assert!(e.is_transient(), "a 503 must read as transient: {e:?}");
        }
    }

    let _ = harness.shutdown.send(());
}

#[tokio::test]
async fn a_wan_router_does_not_serve_the_lan_only_routes_at_all() {
    // The unauthenticated operations must be impossible to expose by accident. A WAN
    // endpoint uses `router`, and `S2C` says it "**cannot** implement these operations"
    // and should answer 404 — which an absent route does for free.
    let identity = SelfSignedEndpoint::generate(["localhost".into()]).unwrap();
    let config = identity.server_config().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let store = Memory::new();
    store.add_node(node("cem-1", Role::Cem, "Home hub"), None);
    let endpoint = Endpoint::new(
        Arc::new(PairingService::wan(
            store.clone(),
            endpoint_description(Deployment::Wan),
            "localhost",
        )),
        Arc::new(SessionService::new(
            store,
            format!("wss://localhost:{port}/s2"),
        )),
    );
    // `router`, not `lan_router`.
    let app = router(endpoint);
    let (shutdown, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = serve(listener, config, app, async {
            let _ = rx.await;
        })
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let base = format!("https://localhost:{port}/v1/");
    let client = s2_kit::connect::client::LanClient::new(&base).unwrap();
    for outcome in [client.endpoint().await.err(), client.nodes().await.err()] {
        let Some(s2_kit::connect::client::Error::Http { status, .. }) = outcome else {
            panic!("a WAN endpoint must not serve the LAN-only routes");
        };
        assert_eq!(
            status.0, 404,
            "and 404 is what the specification recommends"
        );
    }

    let _ = shutdown.send(());
}

/// `S2C §Long-polling`, which exists for the one node nobody can dial.
///
/// Two things worth proving: a queued instruction wakes a request that is already waiting,
/// and a request with nothing to say comes back empty rather than failing.
#[tokio::test]
async fn a_node_with_no_server_is_told_what_to_do_by_long_polling() {
    let harness = start().await;
    let client = s2_kit::connect::client::LanClient::new(&harness.base_url).unwrap();
    let rm = NodeId::parse("rm-1").unwrap();
    let mine = vec![WaitForPairing {
        client_node_id: rm,
        client_node_description: Some(node("rm-1", Role::Rm, "Charger 9000")),
        client_endpoint_description: Some(endpoint_description(Deployment::Lan)),
        error_message: None,
    }];

    // A poll that arrives before the queue has anything waits, and is woken the moment
    // the endpoint has something to say — not at the end of its twenty-five seconds.
    let endpoint = harness.endpoint.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        endpoint.instruct_waiting_node(rm, WaitAction::RequestPairing);
    });
    let started = std::time::Instant::now();
    let instructions = client
        .wait_for_pairing(&mine)
        .await
        .expect("the long poll must answer");
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0].client_node_id, rm);
    assert_eq!(instructions[0].action, WaitAction::RequestPairing);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "a queued instruction must wake the held request, not wait out the hold"
    );

    // The endpoint learned what the node is, which is what a user interface lists.
    let waiting = harness.endpoint.waiting_nodes();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].0, rm);
    assert_eq!(
        waiting[0]
            .1
            .description
            .as_ref()
            .map(|d| d.model_name.as_str()),
        Some("Charger 9000")
    );

    // An instruction is handed out once: the next poll has nothing left to say.
    let endpoint = harness.endpoint.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        // Wake the held request with an instruction for a node this client did not ask
        // about, so the answer is genuinely empty rather than merely early.
        endpoint.instruct_waiting_node(NodeId::parse("rm-2").unwrap(), WaitAction::PreparePairing);
    });
    let again = client
        .wait_for_pairing(&mine)
        .await
        .expect("an empty answer is still an answer");
    assert!(again.is_empty(), "{again:?}");

    let _ = harness.shutdown.send(());
}

/// What an unauthenticated caller can make an endpoint allocate.
///
/// `requestPairing` and `waitForPairing` carry no bearer, and a subnet contains whatever
/// is plugged into it. Both things a caller chooses the size of are bounded: the request
/// body, and the nodes the long-polling table remembers.
#[tokio::test]
async fn an_unauthenticated_caller_cannot_make_the_endpoint_grow() {
    let harness = start().await;
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    // A body past the cap is refused before anything parses it.
    let huge = "x".repeat(s2_kit::connect::server::MAX_BODY_BYTES + 1024);
    let response = client
        .post(format!("{}requestPairing", harness.base_url))
        .header("content-type", "application/json")
        .body(huge)
        .send()
        .await
        .expect("the server must answer rather than die");
    assert!(
        response.status().is_client_error(),
        "got {}",
        response.status()
    );

    // And a long poll naming more nodes than any honest client has does not fill the
    // table: the endpoint answers, and remembers a bounded number of them.
    let lan = s2_kit::connect::client::LanClient::new(&harness.base_url).unwrap();
    // Six batches of the per-poll maximum is comfortably past the table's own cap.
    let per_poll = Endpoint::MAX_NODES_PER_POLL;
    for batch in 0..12u32 {
        let nodes: Vec<WaitForPairing> = (0..per_poll)
            .map(|i| WaitForPairing {
                client_node_id: NodeId::parse(&format!("flood-{batch}-{i}")).unwrap(),
                client_node_description: None,
                client_endpoint_description: None,
                error_message: None,
            })
            .collect();
        // Nothing is queued for any of them, so each poll returns empty after its hold —
        // which is why the wake-up below exists rather than a four-batch wait.
        let endpoint = harness.endpoint.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            endpoint.instruct_waiting_node(
                NodeId::parse("nobody").unwrap(),
                WaitAction::PreparePairing,
            );
        });
        let answer = lan.wait_for_pairing(&nodes).await.expect("an answer");
        assert!(answer.is_empty());
    }
    let waiting = harness.endpoint.waiting_nodes().len();
    assert_eq!(
        waiting,
        Endpoint::MAX_WAITING_NODES,
        "the long-polling table must stop at its cap, not grow past it"
    );

    let _ = harness.shutdown.send(());
}

#[tokio::test]
async fn the_lan_only_routes_describe_the_endpoint_to_a_user() {
    let harness = start().await;
    let client = s2_kit::connect::client::LanClient::new(&harness.base_url).unwrap();

    let description = client
        .endpoint()
        .await
        .expect("the endpoint describes itself");
    assert_eq!(description.name.as_deref(), Some("Acme home hub"));
    assert_eq!(description.deployment, Some(Deployment::Lan));

    let nodes = client.nodes().await.expect("the node list");
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].model_name, "Home hub");
    assert_eq!(nodes[0].role, Role::Cem);

    let _ = harness.shutdown.send(());
}

#[tokio::test]
async fn unpairing_makes_the_tokens_stop_working() {
    let harness = start().await;
    harness.store.open_for_pairing(
        harness.cem_node,
        PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
    );
    let paired = rm_client(&harness.base_url)
        .run(
            PairingCode::parse("hub7-A1b2C3d4", TokenKind::Static).unwrap(),
            Timestamp::now(),
        )
        .await
        .expect("the pairing must complete");

    let mut session = Session::new(
        &paired.pairing.details.initiate_session_url,
        &paired.policy(),
        paired.pairing.local,
        paired.pairing.remote.id,
        TokenStore::new(paired.pairing.details.access_token.clone()),
    )
    .unwrap();
    session
        .initiate(Timestamp::now())
        .await
        .expect("one session");

    session.unpair().await.expect("unpairing");
    assert!(harness.store.paired(paired.pairing.local).is_none());
    assert!(session.tokens().is_empty());

    let _ = harness.shutdown.send(());
}

#[tokio::test]
async fn the_router_tracks_and_releases_pairing_attempts() {
    // `requestPairing` carries no bearer, so the router remembers an attempt on behalf of
    // anyone who can reach it. Expiry and sweeping are unit-tested in `server::routes`;
    // what this checks is that the routes are actually wired to that bookkeeping — that
    // an attempt is recorded when one starts and released when it finishes.
    let harness = start().await;
    harness.store.open_for_pairing(
        harness.cem_node,
        PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
    );
    assert_eq!(harness.endpoint.live_attempts(), 0);

    // One that completes is released at once: "Once the pairing process is finished
    // (with or without success) the identifier can be discarded."
    rm_client(&harness.base_url)
        .run(
            PairingCode::parse("hub7-A1b2C3d4", TokenKind::Static).unwrap(),
            Timestamp::now(),
        )
        .await
        .expect("the pairing must complete");
    assert_eq!(
        harness.endpoint.live_attempts(),
        0,
        "a completed attempt is released immediately, not left to expire"
    );

    // The node's rate-limit slot frees one second after an attempt finishes; a second
    // pairing any sooner would be answered `503`, which is the limit doing its job.
    harness.store.open_for_pairing(
        harness.cem_node,
        PairingToken::parse("A1b2C3d4", TokenKind::Static).unwrap(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    // A client that fails at step 4 has already made the server record an attempt, and
    // never reaches `finalizePairing` to release it. That one is held until its budget
    // runs out and the next `remember` sweeps it.
    rm_client(&harness.base_url)
        .run(
            PairingCode::parse("hub7-WRONGTOKEN", TokenKind::Static).unwrap(),
            Timestamp::now(),
        )
        .await
        .expect_err("a wrong token cannot pair");
    assert_eq!(
        harness.endpoint.live_attempts(),
        1,
        "an abandoned attempt is held, to be swept when its budget runs out"
    );

    let _ = harness.shutdown.send(());
}

#[tokio::test]
async fn a_session_must_not_run_under_the_pairing_tls_policy() {
    // The type system cannot stop someone passing it, so the constructor does.
    let refused = Session::new(
        "https://localhost/v1/",
        &s2_kit::connect::tls::TlsPolicy::LanPairingOnly,
        NodeId::parse("rm-1").unwrap(),
        NodeId::parse("cem-1").unwrap(),
        TokenStore::default(),
    );
    assert!(
        refused.is_err(),
        "an unauthenticated session must be refused"
    );
}

#[tokio::test]
async fn plain_http_is_refused_before_anything_is_sent() {
    let refused = Pairing::lan(
        "http://localhost/v1/",
        node("rm-1", Role::Rm, "Charger"),
        endpoint_description(Deployment::Lan),
    );
    assert!(refused.is_err(), "S2 Connect is HTTPS only");
}
