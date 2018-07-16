use fibers::sync::mpsc;
use futures::{Async, Poll, Stream};
use hyparview::{self, Action as HyparviewAction, Node as HyparviewNode};
use plumtree::message::Message;
use plumtree::{self, Action as PlumtreeAction, Node as PlumtreeNode};
use rand::{self, Rng, SeedableRng, StdRng};
use slog::Logger;
use std::collections::VecDeque;
use std::net::SocketAddr;

use rpc::RpcMessage;
use ServiceHandle;
use {Error, ErrorKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalNodeId(u64);
impl LocalNodeId {
    pub fn new(id: u64) -> Self {
        LocalNodeId(id)
    }

    pub fn value(&self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeId {
    pub addr: SocketAddr, // TODO: location(?)
    pub local_id: LocalNodeId,
}

#[derive(Debug, Clone)]
pub struct NodeHandle {
    local_id: LocalNodeId,
    message_tx: mpsc::Sender<RpcMessage>,
}
impl NodeHandle {
    pub fn local_id(&self) -> LocalNodeId {
        self.local_id
    }

    pub fn send_rpc_message(&self, message: RpcMessage) {
        let _ = self.message_tx.send(message);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageId {
    pub node_id: NodeId,
    pub seqno: u64,
}

#[derive(Debug)]
pub struct System;
impl plumtree::System for System {
    type NodeId = NodeId;
    type MessageId = MessageId;
    type MessagePayload = Vec<u8>; // TODO:
}

#[derive(Debug)]
pub struct Node {
    logger: Logger,
    id: NodeId,
    local_id: LocalNodeId, // TODO: remove
    service: ServiceHandle,
    message_rx: mpsc::Receiver<RpcMessage>,
    hyparview_node: HyparviewNode<NodeId, StdRng>,
    plumtree_node: PlumtreeNode<System>,
    message_seqno: u64,
    deliverable_messages: VecDeque<Message<System>>,
}
impl Node {
    pub fn new(logger: Logger, service: ServiceHandle) -> Node {
        let id = service.generate_node_id();
        let (message_tx, message_rx) = mpsc::channel();
        let handle = NodeHandle {
            local_id: id.local_id,
            message_tx,
        };
        service.register_local_node(handle);
        let rng = StdRng::from_seed(rand::thread_rng().gen());
        Node {
            logger,
            id: id.clone(),
            local_id: id.local_id,
            service,
            message_rx,
            hyparview_node: HyparviewNode::with_options(
                id.clone(),
                hyparview::NodeOptions::new().set_rng(rng),
            ),
            plumtree_node: PlumtreeNode::new(id.clone()),
            message_seqno: 0, // TODO: random (or make initial node id random)
            deliverable_messages: VecDeque::new(),
        }
    }

    pub fn join(&mut self, contact_peer: NodeId) {
        info!(
            self.logger,
            "Joins a group by contacting to {:?}", contact_peer
        );
        self.hyparview_node.join(contact_peer);
    }

    pub fn broadcast(&mut self, message: Vec<u8>) {
        warn!(self.logger, "[TODO] Broadcast: {:?}", message);
        let mid = MessageId {
            node_id: self.id.clone(),
            seqno: self.message_seqno,
        };
        self.message_seqno += 1;

        let m = Message {
            id: mid,
            payload: message,
        };
        self.plumtree_node.broadcast_message(m);
    }

    fn handle_hyparview_action(&mut self, action: HyparviewAction<NodeId>) {
        match action {
            HyparviewAction::Send {
                destination,
                message,
            } => {
                warn!(self.logger, "[TODO] Send: {:?}", message);
                let message = RpcMessage::Hyparview(message);
                // TODO: handle error (i.e., disconnection)
                self.service.send_message(destination, message);
            }
            HyparviewAction::Notify { event } => {
                use hyparview::Event;
                match event {
                    Event::NeighborDown { node } => {
                        info!(self.logger, "Neighbor down: {:?}", node);
                        self.plumtree_node.handle_neighbor_down(&node);
                    }
                    Event::NeighborUp { node } => {
                        info!(self.logger, "Neighbor up: {:?}", node);
                        self.plumtree_node.handle_neighbor_up(&node);
                    }
                }
            }
            HyparviewAction::Disconnect { node } => {
                info!(self.logger, "Disconnected: {:?}", node);
            }
        }
    }

    fn handle_plumtree_action(&mut self, action: PlumtreeAction<System>) {
        warn!(self.logger, "[TODO] Action: {:?}", action);
        match action {
            PlumtreeAction::Send {
                destination,
                message,
            } => {
                let message = RpcMessage::Plumtree(message);
                self.service.send_message(destination, message);
            }
            PlumtreeAction::Deliver { message } => {
                self.deliverable_messages.push_back(message);
            }
        }
    }

    fn handle_rpc_message(&mut self, message: RpcMessage) {
        match message {
            RpcMessage::Hyparview(m) => {
                warn!(self.logger, "[TODO] Recv: {:?}", m); // TODO: remove
                self.hyparview_node.handle_protocol_message(m);
            }
            RpcMessage::Plumtree(m) => {
                warn!(self.logger, "[TODO] Recv: {:?}", m); // TODO: remove
                self.plumtree_node.handle_protocol_message(m);
            }
        }
    }

    fn leave(&self) {
        for peer in self.hyparview_node.active_view().iter().cloned() {
            let message = hyparview::message::DisconnectMessage {
                sender: self.id.clone(),
            };
            let message = hyparview::message::ProtocolMessage::Disconnect(message);
            let message = RpcMessage::Hyparview(message);
            self.service.send_message(peer, message);
        }
    }
}
impl Stream for Node {
    type Item = Message<System>;
    type Error = Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let mut did_something = true;
        while did_something {
            did_something = false;

            if let Some(message) = self.deliverable_messages.pop_front() {
                return Ok(Async::Ready(Some(message)));
            }

            while let Some(action) = self.hyparview_node.poll_action() {
                self.handle_hyparview_action(action);
                did_something = true;
            }
            while let Some(action) = self.plumtree_node.poll_action() {
                self.handle_plumtree_action(action);
                did_something = true;
            }
            while let Async::Ready(message) = self.message_rx.poll().expect("Never fails") {
                let message = track_assert_some!(message, ErrorKind::Other, "Service down");
                self.handle_rpc_message(message);
                did_something = true;
            }

            // TODO: call hyperview shuffle/fill/sync periodically
        }
        Ok(Async::NotReady)
    }
}
impl Drop for Node {
    fn drop(&mut self) {
        self.service.deregister_local_node(self.local_id);
        self.leave();
    }
}
