#![feature(test)]
#![feature(duration_as_u128)]
//! Implements peer membership and gossip protocol to communicate in P2P fashion
//!
//! Every peer has a unique name and manages a certain number of open connections to other peers
//! (atm fully connected to all peers). After a connection is established all known peers are
//! exchanged, resulting in a global membership view. We assume a low order of peers here,
//! therefore this should not be problematic. This module cares about all details of connection and
//! trust between peers and offers a stream and thread-safe `GossipPush` structure to dissiminate
//! requests to other peers.
//!
//! ## Example
//! ```ignore
//! let gossip = Gossip::new("127.0.0.1:8001".parse::<SocketAddr>(), None, "My Peer".into());
//! let writer = gossip.writer();
//!
//! let gossip = gossip.for_each(|id, buf| {
//!     println!("Got buf(n = {}) from {}", buf.len(), id);
//!
//!     Ok(())
//! });
//!
//! tokio::run(gossip);
//! ```
extern crate test;
#[macro_use]
extern crate log;
extern crate nix;
extern crate bytes;
extern crate net2;
extern crate ring;
extern crate bincode;
extern crate tokio;
#[macro_use]
extern crate futures;
#[macro_use]
extern crate serde;

pub mod local_ip;
pub mod error;
pub mod transition;
mod protocol;
pub mod discover;

pub use error::*;
pub use transition::{Transition, TransitionKey, Inspector};

use std::sync::{Mutex, Arc};
use std::net::SocketAddr;
use std::collections::HashMap;

use futures::{Async, Stream, task, Poll, future, Future};
use futures::sync::mpsc::{Receiver, Sender, channel};
use tokio::io;
use tokio::prelude::task::Task;
use tokio::net::{TcpListener, TcpStream, tcp::Incoming};

use self::protocol::{Peer, ResolvePeers, PeerCodecWrite, NetworkKey};
pub use self::protocol::Packet;
pub use self::discover::{Beacon, Discover};

/// Identification of a peer. This is the public key (256bit) of a Schnorr signature using a
/// twisted Edwards form of Curve25519. The key is used to verify that a message is signed by its
/// author.
pub type PeerId = Vec<u8>;

/// Contains information about the whereabouts of a peer
///
/// The identity as well as the connection to a peer are stored here. They are
/// telling us how to reach out for a peer and how we should encrypt data for him.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PeerPresence {
    pub id: PeerId,
    addr: SocketAddr,
    writer: Option<usize>
}

pub enum SpreadTo {
    Everyone,
    Peer(PeerId)
}

/// Push packets to peers, either to everyone or to a single destination. 
///
/// This wraps the write map inside a mutex and is therefore safe to share across threads. Any
/// attempts to write to a closed socket is at the moment ignored. Furthermore it is assumed that
/// flushing is immediately successful.
pub struct Spread<T: Inspector> {
    my_id: PeerId,
    task: Arc<Mutex<Option<Task>>>,
    peers: Arc<Mutex<HashMap<PeerId, PeerCodecWrite<TcpStream>>>>,
    inspector: Arc<Mutex<T>>
}

impl<T: Inspector> Clone for Spread<T> {
    fn clone(&self) -> Spread<T> {
        Spread {
            my_id: self.my_id.clone(),
            task: self.task.clone(),
            peers: self.peers.clone(),
            inspector: self.inspector.clone()
        }
    }
}

//unsafe impl<T: Inspector> Send for Spread<T> {}
//unsafe impl<T: Inspector> Sync for Spread<T> {}

impl<T: Inspector> Spread<T> {
    pub fn new(my_id: PeerId, inspector: Arc<Mutex<T>>) -> Spread<T> {
        Spread { 
            peers: Arc::new(Mutex::new(HashMap::new())), 
            task: Arc::new(Mutex::new(None)),
            my_id, inspector 
        }
    }

    pub fn get(&self) -> impl Future<Item = (), Error = ()> {
        let peers_cloned = self.peers.clone();
        let task_cloned = self.task.clone();
        let mut set_task = false;

        let future = future::poll_fn(move || {
            if !set_task {
                let task = task::current();
                *(task_cloned.lock().unwrap()) = Some(task);
                set_task = true;
            }

            let mut removed = Vec::new();
            {
                let mut peers = peers_cloned.lock().unwrap();
                for (id, peer) in peers.iter_mut() {
                    if !peer.is_empty() {
                        match peer.poll_flush() {
                            Err(_) => {
                                removed.push(id.clone());
                            },
                            _ => {}
                        }
                    }
                }
            }

            let mut peers = peers_cloned.lock().unwrap();
            for id in removed {
                peers.remove(&id).unwrap().shutdown().unwrap();
            }


            return Ok(Async::NotReady); 
        });

        future
    }

    pub fn add_peer(&self, id: &PeerId, writer: PeerCodecWrite<TcpStream>) -> usize {
        let mut peers = self.peers.lock().unwrap();
        let len = peers.keys().len();
        peers.insert(id.clone(), writer);

        return len;
    }

    pub fn spread(&self, packet: Packet, dest: SpreadTo) {
        match dest {
            SpreadTo::Everyone => {
                let mut peers = self.peers.lock().unwrap();
                for (_, peer) in peers.iter_mut() {
                    peer.buffer(packet.clone());
                }
            },
            SpreadTo::Peer(id) => {
                let mut peers = self.peers.lock().unwrap();
                if let Some(ref mut peer) = peers.get_mut(&id) {
                    peer.buffer(packet);
                }
            }
        }

        if let Some(ref task) = *self.task.lock().unwrap() {
            task.notify();
        }
    }

    pub fn push(&self, buf: Vec<u8>) {
        let tips = self.inspector.lock().unwrap().tips();

        let transition = Transition::new(self.my_id.clone(), tips, buf);
        // store the new transition in our database (assuming it is correct)
        self.inspector.lock().unwrap().store(transition.clone());

        // and forward to everyone else
        self.spread(Packet::Push(transition), SpreadTo::Everyone);
    }
}

/// Configuration
pub struct GossipConf {
    pub addr: Option<SocketAddr>,
    pub key: Option<NetworkKey>,
    contact: Option<SocketAddr>,
    pub id: Option<PeerId>
}

impl GossipConf {
    pub fn new() -> GossipConf {
        GossipConf { addr: None, key: None, contact: None, id: None }
    }

    pub fn addr<T: Into<SocketAddr>>(mut self, addr: T) -> GossipConf {
        self.addr = Some(addr.into());

        self
    }

    pub fn network_key<T: Into<NetworkKey>>(mut self, key: T) -> GossipConf {
        self.key = Some(key.into());

        self
    }

    pub fn contact<T: Into<SocketAddr>>(mut self, contact: T) -> GossipConf {
        self.contact = Some(contact.into());

        self
    }

    pub fn id<T: Into<PeerId>>(mut self, id: T) -> GossipConf {
        self.id = Some(id.into());

        self
    }

    pub fn retrieve(self) -> (SocketAddr, NetworkKey, Option<SocketAddr>, PeerId) {
        (
            self.addr.expect("Missing binding addr!"),
            self.key.expect("Network key is missing!"),
            self.contact,
            self.id.expect("Peer identification is missing!")
        )
    }
}

/// Implements the peer sampling and data dissemination
///
/// It consists of four parts. First a channel to which connected peers are hooked up. They
/// will send packets through the PeerCodec. Second an incoming field to accept new peers asking
/// for a connection. Third a stream of emerging connections which are not fully established. And
/// forth a log of existing connections to peer.
pub struct Gossip<T: Inspector> {
    myself: PeerPresence,
    recv: Receiver<(PeerId, Packet)>,
    sender: Sender<(PeerId, Packet)>,
    books: HashMap<PeerId, PeerPresence>,
    writer: Spread<T>,
    resolve: ResolvePeers,
    incoming: Incoming,
    key: NetworkKey,
    inspector: Arc<Mutex<T>>
}

impl<T: Inspector> Gossip<T> {
    pub fn new(conf: GossipConf, inspector: T) -> Gossip<T> {
        let (mut addr, key, contact, id) = conf.retrieve();

        let (sender, receiver) = channel(1024);

        // check if port is available
        let listener;
        loop {
            match TcpListener::bind(&addr) {
                Ok(a) => {listener = a; break; }
                Err(_) => {
                    let old_port = addr.port();
                    addr.set_port(old_port + 1);
                }
            }
        }

        let myself = PeerPresence {
            id: id.clone(),
            addr: listener.local_addr().unwrap(),
            writer: None
        };

        let tips = inspector.tips();
        let tips = inspector.restore(tips).unwrap();

        let peers = match contact {
            Some(addr) => {
                vec![Peer::connect(&addr, key, myself.clone(), tips)]
            },
            None => {
                match Beacon::new(1, key, myself.addr.port()).wait(2) {
                    Some(contact) => vec![Peer::connect(&contact, key, myself.clone(), tips)],
                    _ => {
                        Vec::new()
                    }
                }
            }
        };

        let inspector = Arc::new(Mutex::new(inspector));

        println!("Gossip: Start server with addr {:?}", addr);

        Gossip {
            myself: myself,
            recv: receiver,
            sender: sender,
            books: HashMap::new(),
            incoming: listener.incoming(),
            resolve: ResolvePeers::new(peers),
            writer: Spread::new(id, inspector.clone()),
            key, inspector
        }
    }

    pub fn writer(&self) -> Spread<T> {
        self.writer.clone()
    }

    pub fn id(&self) -> PeerId {
        self.myself.id.clone()
    }

    pub fn network(&self) -> NetworkKey {
        self.key.clone()
    }

    pub fn addr(&self) -> SocketAddr {
        self.myself.addr.clone()
    }
}

/// Create a new stream, managing the gossip protocol
impl<T: Inspector> Stream for Gossip<T> {
    type Item = Packet;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        // first look for newly arriving peers and await a Join message
        match self.incoming.poll() {
            Ok(Async::Ready(Some(socket))) => {
                let tips = self.inspector.lock().unwrap().tips();
                let tips = self.inspector.lock().unwrap().restore(tips).unwrap();

                trace!("New connection from {}", socket.peer_addr().unwrap());

                self.resolve.add_peer(Peer::send_join(socket, self.key, self.myself.clone(), tips));
            },
            Err(err) => {
                println!("Listener err: {:?}", err);

                return Err(err);
            },
            _ => {}
        }

        // poll all connecting peers
        //
        match self.resolve.poll() {
            Ok(Async::Ready(Some((reader, mut writer, mut presence, tips)))) => {
                //if self.books.contains_key(&presence.id) || self.myself.id == presence.id {
                if false {
                    warn!("Got already existing id {:?} from {:?}", presence.id, presence.addr);

                    writer.shutdown().unwrap();
                } else {
                    info!("New peer connected with {:?} tips from {:?}", tips.len(), presence.addr);

                    // hook up the packet output to us
                    reader.redirect_to(self.sender.clone(), presence.id.clone(), task::current());
                    // ask for other peers if this is our contact
                    if self.books.is_empty() {
                        writer.buffer(Packet::GetPeers(None));
                        writer.poll_flush().unwrap();
                    }

                    // if everything is fine, send new transitions for this peer
                    for transition in self.inspector.lock().unwrap().subgraph(tips) {
                        writer.buffer(Packet::Push(transition));

                        // write everything to the peer
                        writer.poll_flush().unwrap();

                    }

                    self.books.insert(presence.id.clone(), presence.clone());

                    // empty a new log entry for our peer
                    let idx = self.writer.add_peer(&presence.id, writer);
                    presence.writer = Some(idx);

                    // the connection is established
                    //return Ok(Async::Ready(Some((presence.id, Vec::new()))));
                }


            },
            _ => {}
        }

        // now try to get a new packet from the hooked peers
        loop {
        let res = self.recv.poll();
        let (id, packet) = try_ready!(res.map_err(|_| io::ErrorKind::Other)).unwrap();
        
        // and process it with some logic
        match packet {
            Packet::GetPeers(None) => {
                let mut list: Vec<PeerPresence> = self.books.values().cloned()
                    .filter_map(|mut x| {
                        if x.id != id {
                            x.writer = None;
                            return Some(x);
                        }
                        
                        return None;
                    }).collect();

                self.writer.spread(Packet::GetPeers(Some(list)), SpreadTo::Peer(id));
            },
            Packet::GetPeers(Some(peers)) => {
                for presence in peers {
                    if !self.books.contains_key(&presence.id) && !self.resolve.has_peer(&presence.id) {
                        info!("Add peer {:?} in {:?}", presence.id, self.myself.id);
                        let tips = self.inspector.lock().unwrap().tips();
                        let tips = self.inspector.lock().unwrap().restore(tips).unwrap();
                        self.resolve.add_peer(Peer::connect(&presence.addr, self.key, self.myself.clone(), tips));
                    }
                }
            },
            Packet::Push(transition) => {
                if !self.inspector.lock().unwrap().approve(&transition) {
                    error!("Received wrong transition!");
                } else if !self.inspector.lock().unwrap().has(&transition.key()) {
                    self.inspector.lock().unwrap().store(transition.clone());

                    // forward to everyone else :(
                    self.writer.spread(Packet::Push(transition.clone()), SpreadTo::Everyone);

                    // the peer has send us a new block of data, forward it
                    return Ok(Async::Ready(Some(Packet::Push(transition))));
                } else {
                    trace!("Got a well-known transition!");
                }
            },
            Packet::File(file_id, data) => {
                if let Some(data) = data {
                    return Ok(Async::Ready(Some(Packet::File(file_id, Some(data)))));
                } else {
                    if let Some(data) = self.inspector.lock().unwrap().get_file(&file_id) {
                        self.writer.spread(Packet::File(file_id, Some(data)), SpreadTo::Peer(id));
                    }
                }
            },
            Packet::Close => {
                self.books.remove(&id);

                info!("Connection to {:?} closed", id);
            },
            _ => {}
        }
        }
    }
}
