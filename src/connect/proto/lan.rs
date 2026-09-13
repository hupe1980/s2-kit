//! The operations only LAN endpoints implement, as wire types.
//!
//! `S2C §LAN-LAN only interactions`: "WAN endpoints **cannot** implement these operations.
//! It is **recommended** that WAN endpoints respond with status code 404." They exist
//! because a node discovered over DNS-SD is a host and a port and nothing else — these are
//! what turn it into a brand, a model name and a list a person can choose from.
//!
//! # None of this is authenticated
//!
//! There is no bearer and no challenge here; the only protection is that the server
//! "**must** check if the request originated from the same subnet" and answer `401`
//! otherwise. A client cannot verify that a server did so. So everything learned here is
//! material for a user interface — "pair with *Acme Charger 9000*?" — and never a basis
//! for a decision. The pairing HMAC is what decides.
//!
//! # Long-polling, and why it exists
//!
//! A Resource Manager on constrained hardware may be "purely an HTTPS client" with no
//! server at all, which makes it unreachable — and pairing has to start somewhere. So it
//! calls `waitForPairing` on the other endpoint and holds the request open until the
//! server tells it what to do. That inverts who dials without inverting who is the pairing
//! server, which is the point.

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use super::{EndpointDescription, NodeDescription, NodeId, PairingErrorMessage};

/// The body of `POST /v1/preparePairing`.
///
/// A hint, not a request: "It is up to the server implementation to decide what to do with
/// this signal, but it can be used to display a pop-up with the pairing token in its UI."
/// The device shows the code at the moment the user is looking for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparePairing {
    /// Who is about to try.
    pub client_node_description: NodeDescription,
    /// From where.
    pub client_endpoint_description: EndpointDescription,
    /// Which of the server's nodes.
    pub server_node_id: NodeId,
}

/// The body of `POST /v1/cancelPreparePairing`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelPreparePairing {
    /// Who has stopped.
    pub client_node_id: NodeId,
    /// Which of the server's nodes they were going to pair with.
    pub server_node_id: NodeId,
}

/// One entry of the `waitForPairing` request array.
///
/// "Note that the client can represent multiple nodes so the request body and the response
/// contains a list."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitForPairing {
    /// Which of the client's nodes is available.
    pub client_node_id: NodeId,
    /// Sent only in the request that follows a `sendNodeDescription` action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_node_description: Option<NodeDescription>,
    /// Likewise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_endpoint_description: Option<EndpointDescription>,
    /// Why the client cannot proceed, when it cannot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<WaitErrorMessage>,
}

/// Why a long-polling client cannot act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WaitErrorMessage {
    /// The client has no pairing token, so there is nothing for it to prove.
    NoValidTokenOnPairingClient,
}

/// What the server tells a long-polling client to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WaitAction {
    /// Send your description on the next poll.
    #[serde(rename = "sendNodeDescription")]
    SendNodeDescription,
    /// The user has started pairing on my side.
    #[serde(rename = "preparePairing")]
    PreparePairing,
    /// The user has stopped.
    #[serde(rename = "cancelPreparePairing")]
    CancelPreparePairing,
    /// Begin the pairing exchange now.
    #[serde(rename = "requestPairing")]
    RequestPairing,
}

/// One entry of the `waitForPairing` response array.
///
/// "The server may only provide at most one item for each clientNodeId."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitInstruction {
    /// Which of the client's nodes this is for.
    pub client_node_id: NodeId,
    /// What to do.
    pub action: WaitAction,
}

/// The `400` body of `preparePairing`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareRefused {
    /// Which of the enumerated reasons.
    pub error_message: PairingErrorMessage,
    /// Free text, for a log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_info: Option<String>,
}

/// How long a long-polling server may hold a request open.
///
/// `S2C §Long-polling`: the server answers "just before the request would time out". The
/// client's own timeout must be longer than this, or it will abandon requests the server
/// was about to answer.
pub const LONG_POLL_HOLD: crate::types::Duration = crate::types::Duration::from_secs(25);

/// How long a long-polling client should wait before giving up on one request.
pub const LONG_POLL_TIMEOUT: crate::types::Duration = crate::types::Duration::from_secs(30);

/// What `GET /v1/nodes` answers: the descriptions, and nothing else.
pub type NodeListing = Vec<NodeDescription>;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn a_clients_timeout_outlasts_the_servers_hold() {
        // Otherwise the client abandons requests the server was about to answer, and the
        // pairing never starts.
        assert!(LONG_POLL_TIMEOUT > LONG_POLL_HOLD);
    }

    #[test]
    fn the_actions_are_spelled_as_the_schema_spells_them() {
        for (action, expected) in [
            (WaitAction::SendNodeDescription, "\"sendNodeDescription\""),
            (WaitAction::PreparePairing, "\"preparePairing\""),
            (WaitAction::CancelPreparePairing, "\"cancelPreparePairing\""),
            (WaitAction::RequestPairing, "\"requestPairing\""),
        ] {
            assert_eq!(serde_json::to_string(&action).unwrap(), expected);
        }
        assert_eq!(
            serde_json::to_string(&WaitErrorMessage::NoValidTokenOnPairingClient).unwrap(),
            "\"NoValidTokenOnPairingClient\""
        );
    }

    #[test]
    fn a_wait_entry_carries_only_what_the_server_asked_for() {
        let entry = WaitForPairing {
            client_node_id: NodeId::parse("rm-1").unwrap(),
            client_node_description: None,
            client_endpoint_description: None,
            error_message: None,
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 1, "{json}");
        assert_eq!(json["clientNodeId"], "rm-1".to_string());
    }
}
