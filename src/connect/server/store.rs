//! Where an endpoint keeps what it knows.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::connect::proto::{
    AccessToken, ConnectionDetails, NodeDescription, NodeId, NodeIdAlias, PairingToken, TokenStore,
};

/// One node this endpoint is paired with.
#[derive(Debug, Clone)]
pub struct PairedNode {
    /// Which of our nodes.
    pub local: NodeId,
    /// Who they are.
    pub remote: NodeDescription,
    /// The access tokens that open the door, newest first.
    pub tokens: TokenStore,
    /// What we told them about where to connect, if we are the communication server.
    pub details: Option<ConnectionDetails>,
}

/// Everything an endpoint must remember across a restart.
///
/// Pairings that live only in memory are pairings a power cut destroys, and the recovery
/// from that is a person walking to the device with a pairing code. Implement this over
/// whatever the host actually persists to.
pub trait Store: Send + Sync + 'static {
    /// The pairing token currently active for one of our nodes, if any.
    ///
    /// `None` means `NoValidPairingTokenOnPairingServer`: the node exists but is not
    /// accepting pairings, which is the normal state of a device nobody has just pressed
    /// the button on.
    fn pairing_token(&self, node: NodeId) -> Option<PairingToken>;

    /// Which of our nodes an alias names.
    fn resolve_alias(&self, alias: &NodeIdAlias) -> Option<NodeId>;

    /// Our node, when the endpoint hosts exactly one and the client named none.
    fn sole_node(&self) -> Option<NodeId>;

    /// Everything about one of our nodes.
    fn node(&self, node: NodeId) -> Option<NodeDescription>;

    /// Every node this endpoint hosts, for `GET /v1/nodes`.
    fn nodes(&self) -> Vec<(NodeDescription, Option<NodeIdAlias>)>;

    /// Record a completed pairing.
    fn pair(&self, paired: PairedNode);

    /// A pairing, by the remote node's identifier.
    fn paired(&self, remote: NodeId) -> Option<PairedNode>;

    /// Every remote node one of our own nodes is currently paired with.
    ///
    /// `S2C §Pairing process`: "A CEM can be paired with multiple RMs at the same time. A
    /// RM can only be paired with one CEM at a time." The second half is a rule somebody
    /// has to apply, and applying it needs this question answered — the pairings are
    /// keyed by the *remote* node, so there is otherwise no way to ask which of them
    /// belong to one of ours.
    fn paired_with(&self, local: NodeId) -> Vec<NodeId>;

    /// Replace a pairing's tokens after a rotation.
    fn set_tokens(&self, remote: NodeId, tokens: TokenStore);

    /// Forget a pairing.
    fn unpair(&self, remote: NodeId);

    /// Which of our nodes a presented access token belongs to, if any.
    fn node_for_token(&self, token: &AccessToken) -> Option<NodeId>;
}

/// A [`Store`] that keeps everything in memory.
///
/// Correct, and suitable for a test or a device that re-pairs on every boot. Anything
/// that must survive a power cut needs a real one.
#[derive(Debug, Default)]
pub struct Memory {
    inner: std::sync::RwLock<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Our own nodes, by identifier.
    nodes: BTreeMap<NodeId, NodeDescription>,
    /// Aliases onto them.
    aliases: BTreeMap<String, NodeId>,
    /// Pairing tokens currently accepting a pairing.
    tokens: BTreeMap<NodeId, PairingToken>,
    /// Completed pairings, by the *remote* node's identifier.
    pairings: BTreeMap<NodeId, PairedNode>,
}

impl Memory {
    /// An empty store.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Add one of our own nodes.
    pub fn add_node(&self, node: NodeDescription, alias: Option<NodeIdAlias>) {
        let Ok(mut inner) = self.inner.write() else {
            return;
        };
        if let Some(alias) = alias {
            inner.aliases.insert(alias.0, node.id);
        }
        inner.nodes.insert(node.id, node);
    }

    /// Open one of our nodes for pairing, with this token.
    ///
    /// What `preparePairing` does, and what pressing the button on a device does.
    pub fn open_for_pairing(&self, node: NodeId, token: PairingToken) {
        if let Ok(mut inner) = self.inner.write() {
            inner.tokens.insert(node, token);
        }
    }

    /// Stop accepting pairings for a node.
    pub fn close_pairing(&self, node: NodeId) {
        if let Ok(mut inner) = self.inner.write() {
            inner.tokens.remove(&node);
        }
    }
}

impl Store for Memory {
    fn pairing_token(&self, node: NodeId) -> Option<PairingToken> {
        self.inner.read().ok()?.tokens.get(&node).cloned()
    }

    fn resolve_alias(&self, alias: &NodeIdAlias) -> Option<NodeId> {
        self.inner.read().ok()?.aliases.get(&alias.0).copied()
    }

    fn sole_node(&self) -> Option<NodeId> {
        let inner = self.inner.read().ok()?;
        // "If it doesn't know any, but expects the endpoint to represent only one node
        // … it doesn't provide a value for nodeId and nodeIdAlias." Only answer when
        // there really is exactly one; guessing would pair the wrong device.
        if inner.nodes.len() == 1 {
            inner.nodes.keys().next().copied()
        } else {
            None
        }
    }

    fn node(&self, node: NodeId) -> Option<NodeDescription> {
        self.inner.read().ok()?.nodes.get(&node).cloned()
    }

    fn nodes(&self) -> Vec<(NodeDescription, Option<NodeIdAlias>)> {
        let Ok(inner) = self.inner.read() else {
            return Vec::new();
        };
        inner
            .nodes
            .values()
            .map(|node| {
                let alias = inner
                    .aliases
                    .iter()
                    .find(|(_, id)| **id == node.id)
                    .map(|(alias, _)| NodeIdAlias(alias.clone()));
                (node.clone(), alias)
            })
            .collect()
    }

    fn pair(&self, paired: PairedNode) {
        if let Ok(mut inner) = self.inner.write() {
            // A completed pairing consumes the token: `S2C` makes a dynamic one expire and
            // leaving a static one open would let the next caller pair as well.
            inner.tokens.remove(&paired.local);
            inner.pairings.insert(paired.remote.id, paired);
        }
    }

    fn paired(&self, remote: NodeId) -> Option<PairedNode> {
        self.inner.read().ok()?.pairings.get(&remote).cloned()
    }

    fn paired_with(&self, local: NodeId) -> Vec<NodeId> {
        let Ok(inner) = self.inner.read() else {
            return Vec::new();
        };
        inner
            .pairings
            .values()
            .filter(|paired| paired.local == local)
            .map(|paired| paired.remote.id)
            .collect()
    }

    fn set_tokens(&self, remote: NodeId, tokens: TokenStore) {
        if let Ok(mut inner) = self.inner.write()
            && let Some(paired) = inner.pairings.get_mut(&remote)
        {
            paired.tokens = tokens;
        }
    }

    fn unpair(&self, remote: NodeId) {
        if let Ok(mut inner) = self.inner.write() {
            inner.pairings.remove(&remote);
        }
    }

    fn node_for_token(&self, token: &AccessToken) -> Option<NodeId> {
        let inner = self.inner.read().ok()?;
        inner
            .pairings
            .values()
            .find(|p| p.tokens.accepts(token))
            .map(|p| p.remote.id)
    }
}
