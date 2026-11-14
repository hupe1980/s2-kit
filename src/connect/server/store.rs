//! Where an endpoint keeps what it knows.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::connect::proto::{
    AccessToken, ConnectionDetails, NodeDescription, NodeId, NodeIdAlias, PairingToken, TokenStore,
};
use crate::types::{Duration, Timestamp};

/// Which pairing: one of this endpoint's nodes, and the remote node it is paired with.
///
/// Both halves, because neither is unique on its own. `S2C §Pairing process`: "A CEM can
/// be paired with multiple RMs at the same time" — so a CEM endpoint holds several
/// pairings with the same `local`, and a WAN endpoint hosting a household's battery and
/// heat pump holds two pairings with the same `remote` CEM. A store keyed by either half
/// alone silently overwrites one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PairingKey {
    /// One of this endpoint's nodes.
    pub local: NodeId,
    /// The node it is paired with.
    pub remote: NodeId,
}

/// One node this endpoint is paired with.
#[derive(Debug, Clone)]
pub struct PairedNode {
    /// Which of our nodes.
    pub local: NodeId,
    /// Who they are.
    pub remote: NodeDescription,
    /// The access tokens that open the door, newest first.
    pub tokens: TokenStore,
    /// The connection details exchanged during pairing: the ones we handed out (7A), or
    /// the ones the client posted when it is the communication server (7B) — in which
    /// case [`ConnectionDetails::ca_fingerprint`] is what to pin when connecting to it.
    pub details: Option<ConnectionDetails>,
    /// Unpaired by this endpoint, and kept only so the next `initiateSession` can be told
    /// so: `S2C §Unpairing by the communication server` — "The next
    /// `/[version]/initiateSession` HTTPS API request **must** fail with the
    /// `No longer paired` response" (the operation is `initiateSession`, erratum E27).
    pub revoked: bool,
}

impl PairedNode {
    /// The key this pairing is stored under.
    #[must_use]
    pub fn key(&self) -> PairingKey {
        PairingKey {
            local: self.local,
            remote: self.remote.id,
        }
    }

    /// The certificate to pin when *this* endpoint connects to the peer — a 6B pairing,
    /// where the peer is the communication server.
    #[must_use]
    pub fn pin(&self) -> Option<crate::connect::tls::Fingerprint> {
        self.details
            .as_ref()
            .and_then(ConnectionDetails::ca_fingerprint)
            .map(crate::connect::tls::Fingerprint::from_bytes)
    }
}

/// Everything an endpoint must remember across a restart.
///
/// Pairings that live only in memory are pairings a power cut destroys, and the recovery
/// from that is a person walking to the device with a pairing code. Implement this over
/// whatever the host actually persists to.
pub trait Store: Send + Sync + 'static {
    /// The pairing token currently accepting a pairing for one of our nodes, if any.
    ///
    /// `None` means `NoValidPairingTokenOnPairingServer`: the node exists but is not
    /// accepting pairings — the normal state of a device nobody has just pressed the
    /// button on, and the state a dynamic token returns to when it expires
    /// (`S2C §pairing token`: "Dynamically generated pairing tokens **must** expire").
    fn pairing_token(&self, node: NodeId, now: Timestamp) -> Option<PairingToken>;

    /// Which of our nodes an alias names.
    fn resolve_alias(&self, alias: &NodeIdAlias) -> Option<NodeId>;

    /// Our node, when the endpoint hosts exactly one and the client named none.
    fn sole_node(&self) -> Option<NodeId>;

    /// Everything about one of our nodes.
    fn node(&self, node: NodeId) -> Option<NodeDescription>;

    /// Every node this endpoint hosts, for `GET /v1/nodes`.
    fn nodes(&self) -> Vec<(NodeDescription, Option<NodeIdAlias>)>;

    /// Record a completed pairing, replacing any earlier one under the same key, and
    /// consume the pairing token that opened it.
    fn pair(&self, paired: PairedNode);

    /// One pairing.
    fn paired(&self, key: PairingKey) -> Option<PairedNode>;

    /// Every pairing one of our own nodes has, revoked ones excluded.
    ///
    /// `S2C §Pairing process`: "A RM can only be paired with one CEM at a time." Applying
    /// that needs this question answered.
    fn pairings_of(&self, local: NodeId) -> Vec<PairingKey>;

    /// Replace a pairing's tokens after a rotation.
    fn set_tokens(&self, key: PairingKey, tokens: TokenStore);

    /// Mark a pairing unpaired by this endpoint, keeping it until the peer has been told.
    fn revoke(&self, key: PairingKey);

    /// Forget a pairing.
    fn unpair(&self, key: PairingKey);

    /// Which pairing a presented access token belongs to, revoked pairings excluded.
    fn pairing_for_token(&self, token: &AccessToken) -> Option<PairingKey>;
}

/// How long a dynamic pairing token stays valid when nothing else is said.
///
/// `S2C §pairing token`: "five minutes is the recommended duration".
pub const DYNAMIC_TOKEN_LIFETIME: Duration = Duration::from_secs(5 * 60);

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
    /// Pairing tokens currently accepting a pairing, and when each stops (`None`: a
    /// static token, valid until closed).
    tokens: BTreeMap<NodeId, (PairingToken, Option<Timestamp>)>,
    /// Completed pairings.
    pairings: BTreeMap<PairingKey, PairedNode>,
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

    /// Open one of our nodes for pairing with a **static** token — the one printed on the
    /// device, valid until [`close_pairing`](Self::close_pairing) or a completed pairing.
    pub fn open_for_pairing(&self, node: NodeId, token: PairingToken) {
        if let Ok(mut inner) = self.inner.write() {
            inner.tokens.insert(node, (token, None));
        }
    }

    /// Open one of our nodes for pairing with a **dynamic** token, valid for
    /// [`DYNAMIC_TOKEN_LIFETIME`] from `now` — what pressing the button on a device with
    /// a display does.
    pub fn open_for_pairing_until(&self, node: NodeId, token: PairingToken, expires: Timestamp) {
        if let Ok(mut inner) = self.inner.write() {
            inner.tokens.insert(node, (token, Some(expires)));
        }
    }

    /// [`open_for_pairing_until`](Self::open_for_pairing_until) `now` plus the
    /// recommended five minutes.
    pub fn open_for_pairing_dynamic(&self, node: NodeId, token: PairingToken, now: Timestamp) {
        let expires = now.checked_add(DYNAMIC_TOKEN_LIFETIME).unwrap_or(now);
        self.open_for_pairing_until(node, token, expires);
    }

    /// Stop accepting pairings for a node.
    pub fn close_pairing(&self, node: NodeId) {
        if let Ok(mut inner) = self.inner.write() {
            inner.tokens.remove(&node);
        }
    }
}

impl Store for Memory {
    fn pairing_token(&self, node: NodeId, now: Timestamp) -> Option<PairingToken> {
        let inner = self.inner.read().ok()?;
        let (token, expires) = inner.tokens.get(&node)?;
        match expires {
            Some(expires) if *expires <= now => None,
            _ => Some(token.clone()),
        }
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
            inner.pairings.insert(paired.key(), paired);
        }
    }

    fn paired(&self, key: PairingKey) -> Option<PairedNode> {
        self.inner.read().ok()?.pairings.get(&key).cloned()
    }

    fn pairings_of(&self, local: NodeId) -> Vec<PairingKey> {
        let Ok(inner) = self.inner.read() else {
            return Vec::new();
        };
        inner
            .pairings
            .iter()
            .filter(|(key, paired)| key.local == local && !paired.revoked)
            .map(|(key, _)| *key)
            .collect()
    }

    fn set_tokens(&self, key: PairingKey, tokens: TokenStore) {
        if let Ok(mut inner) = self.inner.write()
            && let Some(paired) = inner.pairings.get_mut(&key)
        {
            paired.tokens = tokens;
        }
    }

    fn revoke(&self, key: PairingKey) {
        if let Ok(mut inner) = self.inner.write()
            && let Some(paired) = inner.pairings.get_mut(&key)
        {
            paired.revoked = true;
        }
    }

    fn unpair(&self, key: PairingKey) {
        if let Ok(mut inner) = self.inner.write() {
            inner.pairings.remove(&key);
        }
    }

    fn pairing_for_token(&self, token: &AccessToken) -> Option<PairingKey> {
        let inner = self.inner.read().ok()?;
        // Every pairing is examined, so how long this takes does not say which matched.
        let mut found = None;
        for (key, paired) in &inner.pairings {
            if paired.tokens.accepts(token) && !paired.revoked {
                found = Some(*key);
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::proto::{Role, TokenKind};

    fn id(s: &str) -> NodeId {
        NodeId::parse(s).expect("a legal id")
    }

    fn node(s: &str, role: Role) -> NodeDescription {
        NodeDescription {
            id: id(s),
            brand: "Test".into(),
            logo_url: None,
            kind: "test".into(),
            model_name: "test".into(),
            user_defined_name: None,
            role,
        }
    }

    fn paired(local: &str, remote: &str, token: &str) -> PairedNode {
        PairedNode {
            local: id(local),
            remote: node(remote, Role::Cem),
            tokens: TokenStore::new(AccessToken::new(token)),
            details: None,
            revoked: false,
        }
    }

    #[test]
    fn one_cem_paired_with_two_nodes_of_one_endpoint_keeps_both() {
        // A WAN endpoint hosting a battery and a heat pump, both paired with one CEM. Keyed
        // by the remote node alone, the second pairing would overwrite the first, and the
        // CEM's next session to the battery would be refused as `NoLongerPaired`.
        let store = Memory::new();
        store.pair(paired("battery", "cem", "token-battery"));
        store.pair(paired("heat-pump", "cem", "token-heat-pump"));
        let battery = PairingKey {
            local: id("battery"),
            remote: id("cem"),
        };
        let heat_pump = PairingKey {
            local: id("heat-pump"),
            remote: id("cem"),
        };
        assert!(store.paired(battery).is_some());
        assert!(store.paired(heat_pump).is_some());
        assert_eq!(
            store.pairing_for_token(&AccessToken::new("token-battery")),
            Some(battery)
        );
        store.unpair(battery);
        assert!(
            store.paired(heat_pump).is_some(),
            "unpairing one keeps the other"
        );
    }

    #[test]
    fn a_dynamic_token_expires_and_a_static_one_does_not() {
        let store = Memory::new();
        let now = Timestamp::from_unix(1_700_000_000, 0);
        let token = || PairingToken::parse("A1b2C3d4", TokenKind::Dynamic).expect("a token");
        store.open_for_pairing_dynamic(id("rm"), token(), now);
        assert!(store.pairing_token(id("rm"), now).is_some());
        let later = now.checked_add(DYNAMIC_TOKEN_LIFETIME).expect("in range");
        assert!(
            store.pairing_token(id("rm"), later).is_none(),
            "five minutes and it is gone"
        );

        store.open_for_pairing(id("rm"), token());
        assert!(store.pairing_token(id("rm"), later).is_some());
    }

    #[test]
    fn a_revoked_pairing_opens_nothing_but_is_still_there_to_be_told_about() {
        let store = Memory::new();
        let entry = paired("rm", "cem", "token");
        let key = entry.key();
        store.pair(entry);
        store.revoke(key);
        assert!(
            store
                .pairing_for_token(&AccessToken::new("token"))
                .is_none()
        );
        assert!(store.pairings_of(id("rm")).is_empty());
        assert!(store.paired(key).is_some_and(|p| p.revoked));
    }
}
