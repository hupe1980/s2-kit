//! S2 Connect end to end, in one process: discover nothing, pair, and open a session.
//!
//! A Resource Manager (an EV charger) pairs with a Customer Energy Manager (a home hub)
//! over a real TLS connection on loopback, then initiates a session and comes back with
//! somewhere to connect a WebSocket. It is the shape of a real deployment with the network
//! shrunk to nothing:
//!
//! ```console
//! $ cargo run --example connect_pair --features connect-client,connect-server
//! ```
//!
//! The interesting part is what the certificate does. During pairing, the charger does
//! *not* verify the hub's certificate — it cannot, the hub minted it itself — and instead
//! mixes its fingerprint into the challenge response. If the hub's answer comes out right,
//! the certificate was the right one, and from that moment on everything runs against the
//! CA pinned from that same chain — including the WebSocket at the end, which is where the
//! two S2 engines finally meet.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::wildcard_imports
)]

use std::sync::Arc;

use s2_kit::connect::client::{Pairing, Session};
use s2_kit::connect::proto::*;
use s2_kit::connect::server::{
    Endpoint, LocalSubnets, Memory, PairingService, SessionService, lan_router, serve,
    websocket_router,
};
use s2_kit::connect::tls::SelfSignedEndpoint;
use s2_kit::io::{Driver, WebSocket};
use s2_kit::prelude::{CemConfig, CemEvent, CemSession, MessageKind, RmConfig, RmEvent, RmSession};
use s2_kit::types::{Timestamp, WireProfile};

fn describe(id: &str, role: Role, brand: &str, model: &str) -> NodeDescription {
    NodeDescription {
        id: NodeId::parse(id).unwrap(),
        brand: brand.into(),
        logo_url: None,
        kind: if role == Role::Cem { "CEM" } else { "EVSE" }.into(),
        model_name: model.into(),
        user_defined_name: None,
        role,
    }
}

fn lan(name: &str) -> EndpointDescription {
    EndpointDescription {
        name: Some(name.into()),
        logo_url: None,
        deployment: Some(Deployment::Lan),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ---------------------------------------------------------------------
    // The hub: a LAN endpoint, so it mints its own certificate authority.
    // ---------------------------------------------------------------------
    let identity = SelfSignedEndpoint::generate(["localhost".into()])?;
    println!("hub leaf fingerprint  {}", identity.leaf);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let base = format!("https://localhost:{port}/v1/");

    let store = Memory::new();
    let hub = describe("cem-1", Role::Cem, "Acme", "Home hub");
    let hub_id = hub.id;
    store.add_node(hub, Some(NodeIdAlias::parse("hub7").unwrap()));

    // Both nodes are in the LAN, so the CEM is the communication server — it is the side
    // that hands out connection details.
    let pairing = Arc::new(
        PairingService::lan(store.clone(), lan("Acme home hub"), identity.leaf)
            .with_connection_details(ConnectionDetails {
                initiate_session_url: base.clone(),
                access_token: AccessToken::from_entropy(&[7u8; 32])?,
                certificate_fingerprint: [("SHA256".into(), identity.leaf.to_hex())]
                    .into_iter()
                    .collect(),
            }),
    );
    let session_service = Arc::new(SessionService::new(
        store.clone(),
        format!("wss://localhost:{port}/s2"),
    ));
    // `lan_router`, because this hub is in the LAN and so serves `/endpoint` and
    // `/nodes` — the unauthenticated operations, behind the same-subnet check that is
    // their only access control. A WAN endpoint would use `router` and answer 404 there.
    let endpoint = Endpoint::new(pairing, session_service);
    let app = lan_router(endpoint.clone(), LocalSubnets::detect()?).merge(websocket_router(
        endpoint,
        "/s2",
        |node, socket| async move {
            // One `CemSession` per accepted connection. `node` says which paired resource
            // this is; a real hub would look up what it knows about that device.
            println!("hub accepted socket  from {node}");
            let mut cem = CemSession::new(CemConfig::default().pre_negotiated(WireProfile::V1_0_0));
            cem.open(Timestamp::now());
            let _ = Driver::new(socket)
                .run(&mut cem, |event, _session| {
                    if let CemEvent::ResourceDescribed(details) = event {
                        println!("hub learned about    {}", details.resource_id);
                    }
                })
                .await;
        },
    ));
    let config = identity.server_config()?;
    tokio::spawn(async move {
        let _ = serve(listener, config, app, std::future::pending()).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // The user presses the button on the hub, which opens it for pairing and shows a code.
    let token = PairingToken::parse("A1b2C3d4", TokenKind::Static)?;
    store.open_for_pairing(hub_id, token);
    println!("hub shows code        hub7-A1b2C3d4\n");

    // ---------------------------------------------------------------------
    // The charger: types in the code and pairs.
    // ---------------------------------------------------------------------
    let charger = describe("rm-1", Role::Rm, "Acme", "Charger 9000");
    let code = PairingCode::parse("hub7-A1b2C3d4", TokenKind::Static)?;

    let started = std::time::Instant::now();
    let paired = Pairing::lan(&base, charger, lan("Acme charger"))?
        .run(code, Timestamp::now())
        .await?;
    println!(
        "paired with           {} {}",
        paired.pairing.remote.brand, paired.pairing.remote.model_name
    );
    println!(
        "  took                {:.1?} (≥ 1 s is the mandatory delay)",
        started.elapsed()
    );
    println!(
        "  comms server        {:?}",
        paired.pairing.communication_server
    );
    println!(
        "  pinned CA           {} bytes of certificate",
        paired.identity.as_ref().map_or(0, |i| i.root.len())
    );
    println!(
        "  TLS from here on    authenticates: {}\n",
        paired.policy().authenticates()
    );

    // ---------------------------------------------------------------------
    // A session: two requests, and somewhere to connect.
    // ---------------------------------------------------------------------
    let mut session = Session::new(
        &paired.pairing.details.initiate_session_url,
        &paired.policy(),
        paired.pairing.local,
        paired.pairing.remote.id,
        TokenStore::new(paired.pairing.details.access_token.clone()),
    )?;

    for round in 1..=2 {
        let credentials = session.initiate(Timestamp::now()).await?;
        println!("session {round}             {}", credentials.websocket_url);
        println!("  S2 JSON version     {:?}", credentials.profile.unwrap());
        println!(
            "  token expires       {} ({} s)",
            credentials.expires(),
            CommunicationToken::lifetime().as_secs_f64()
        );
        // The access token rotated, and the old one is already gone.
        println!("  access tokens held  {}\n", session.tokens().len());
    }

    // ---------------------------------------------------------------------
    // The last mile: the communication token opening a real WebSocket.
    // ---------------------------------------------------------------------
    let credentials = session.initiate(Timestamp::now()).await?;
    // `connect_with_policy`, not `connect`: the hub's certificate is self-signed, and what
    // vouches for it is the CA this pairing pinned. The public PKI has never heard of it.
    let socket = WebSocket::connect_with_policy(
        &credentials.websocket_url,
        Some(credentials.websocket_token.as_str()),
        &paired.policy(),
    )
    .await?;
    println!("websocket open        {}", credentials.websocket_url);

    let mut rm = RmSession::new(
        RmConfig::default().pre_negotiated(WireProfile::V1_0_0),
        s2_kit::testing::battery_details(),
    );
    rm.open(Timestamp::now());
    let mut driver = Driver::new(socket);
    for _ in 0..6 {
        driver.step(&mut rm).await?;
        while let Some(event) = rm.poll_event() {
            if let RmEvent::Acked(handle) = event
                && handle.kind == MessageKind::ResourceManagerDetails
            {
                println!("hub acknowledged      ResourceManagerDetails\n");
                println!("That is the whole of S2 Connect: a code typed by a person, an HMAC");
                println!("bound to a certificate nobody had a reason to trust, and an S2");
                println!("session running over it.");
                return Ok(());
            }
        }
    }
    println!("the hub did not answer in time");
    Ok(())
}
