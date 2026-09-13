//! The long-polling queue behind `POST /v1/waitForPairing`.
//!
//! `S2C §Long-polling` exists for one kind of node: a Resource Manager that is "purely an
//! HTTPS client", with no listening socket. Nothing can dial it, so it dials out and holds
//! a request open, and the endpoint answers when a person presses a button there.
//!
//! That makes this the one place where an S2 Connect server has something to *say* rather
//! than something to answer: a queue per client node, a wake-up when something is added,
//! and a hold of [`LONG_POLL_HOLD`] before answering "nothing yet".

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::connect::proto::{
    EndpointDescription, LONG_POLL_HOLD, NodeDescription, NodeId, WaitAction, WaitForPairing,
    WaitInstruction,
};

/// What an endpoint knows about one node that is waiting to be told what to do.
#[derive(Debug, Clone, Default)]
pub struct WaitingNode {
    /// The node's own description, once it has sent one.
    ///
    /// Absent until the endpoint asks with [`WaitAction::SendNodeDescription`]: a node
    /// merely checking in should not describe itself every twenty-five seconds.
    pub description: Option<NodeDescription>,
    /// Where it is.
    pub endpoint: Option<EndpointDescription>,
    /// Why it says it cannot proceed, if it said so.
    pub error: Option<crate::connect::proto::WaitErrorMessage>,
}

/// The queue of instructions a long-polling endpoint has for its clients.
///
/// Cheap to clone: everything is behind one lock and one notifier.
#[derive(Debug, Default)]
pub(super) struct LongPoll {
    state: std::sync::Mutex<State>,
    /// Woken whenever an instruction is queued, so a held request answers at once instead
    /// of after the rest of its twenty-five seconds.
    woken: tokio::sync::Notify,
}

#[derive(Debug, Default)]
struct State {
    /// At most one pending action per client node: `S2C §Long-polling` says the server
    /// "may only provide at most one item for each clientNodeId", and a queue that grew
    /// would hand out an action the user has already cancelled.
    pending: BTreeMap<NodeId, WaitAction>,
    /// What each polling node has told us about itself.
    seen: BTreeMap<NodeId, WaitingNode>,
}

impl LongPoll {
    /// The most nodes one endpoint will remember across long polls.
    ///
    /// `waitForPairing` needs no bearer and its body is a *list* of identifiers the caller
    /// chose, so a map that grew one entry per identifier would be unbounded memory driven
    /// by unauthenticated input. A LAN endpoint talks to tens of devices.
    pub(super) const MAX_WAITING: usize = 256;

    /// The most nodes one poll may ask about. "The client can represent multiple nodes" is
    /// not "the client can represent every node".
    pub(super) const MAX_PER_POLL: usize = 32;

    /// Tell a client node to do something the next time it asks.
    ///
    /// Replaces whatever was queued for that node, because the actions supersede one
    /// another: a `cancelPreparePairing` after a `preparePairing` is not two things to do.
    pub(super) fn instruct(&self, client: NodeId, action: WaitAction) {
        if let Ok(mut state) = self.state.lock() {
            // Bounded for the same reason as `seen`, and by the same number — except that
            // this map is only ever grown by the *host*, so a full one is a bug in the
            // host rather than an attack. Dropping the new entry rather than an old one
            // keeps the actions already promised.
            if state.pending.len() < Self::MAX_WAITING || state.pending.contains_key(&client) {
                state.pending.insert(client, action);
            }
        }
        self.woken.notify_waiters();
    }

    /// Everything the endpoint has learned from nodes that are waiting.
    pub(super) fn waiting(&self) -> Vec<(NodeId, WaitingNode)> {
        self.state.lock().map_or_else(
            |_| Vec::new(),
            |state| {
                state
                    .seen
                    .iter()
                    .map(|(id, node)| (*id, node.clone()))
                    .collect()
            },
        )
    }

    /// Record what this poll told us, and take whatever is queued for its nodes.
    fn take(&self, nodes: &[WaitForPairing]) -> Vec<WaitInstruction> {
        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for node in nodes.iter().take(Self::MAX_PER_POLL) {
            // Remembering a node is what an unauthenticated caller can make us do, so it
            // is the step that is bounded. Answering one we already know about is not:
            // an entry that exists costs nothing to update, and a device that has been
            // polling for a week must not be forgotten because a stranger filled the map.
            if !state.seen.contains_key(&node.client_node_id)
                && state.seen.len() >= Self::MAX_WAITING
            {
                continue;
            }
            let seen = state.seen.entry(node.client_node_id).or_default();
            // A poll that carries a description is the answer to `sendNodeDescription`;
            // one that does not must not erase what we were told last time.
            if node.client_node_description.is_some() {
                seen.description.clone_from(&node.client_node_description);
            }
            if node.client_endpoint_description.is_some() {
                seen.endpoint.clone_from(&node.client_endpoint_description);
            }
            seen.error = node.error_message;

            if let Some(action) = state.pending.remove(&node.client_node_id) {
                out.push(WaitInstruction {
                    client_node_id: node.client_node_id,
                    action,
                });
            }
        }
        out
    }

    /// Answer a poll: at once if there is something to say, otherwise after the hold.
    ///
    /// The empty answer is not a failure — `S2C §Long-polling` has the server reply "just
    /// before the request would time out", and the client asks again. A wake-up meant for
    /// another node ends this request early with an empty list, which costs one more
    /// request; tracking which waiter each instruction is for would cost a missed wake-up
    /// twenty-five seconds.
    pub(super) async fn poll(&self, nodes: &[WaitForPairing]) -> Vec<WaitInstruction> {
        let ready = self.take(nodes);
        if !ready.is_empty() {
            return ready;
        }
        // Register as a waiter *before* looking again. `notify_waiters` only wakes
        // waiters that are already registered, and `notified()` does not register until
        // it is first polled — so without `enable()` an instruction queued on another
        // thread between this check and the first poll is one this request sleeps
        // through for a full twenty-five seconds.
        let woken = self.woken.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();

        let ready = self.take(nodes);
        if !ready.is_empty() {
            return ready;
        }
        let hold = core::time::Duration::from(LONG_POLL_HOLD);
        if tokio::time::timeout(hold, woken).await.is_err() {
            return Vec::new();
        }
        self.take(nodes)
    }
}
