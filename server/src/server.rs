//! Websocket server implementation
//!
//! The websocket uses Tokio under the hood and manages a state for each connection. It also shares
//! the latest token to all clients and logs every events concerning connecting and disconnecting. 

use std::fmt::Debug;
use std::time::Instant;
use std::sync::{Arc, Mutex};
use std::path::{Path, PathBuf};

use websocket::message::OwnedMessage;
use websocket::server::InvalidConnection;
use websocket::async::Server;

use tokio_core::reactor::{Handle, Core};
use futures::{Future, Sink, Stream};

use state::State;
use hex_conf::Conf;

use hex_database::{Instance, GossipConf};

/// Start the websocket server, supplied with a configuration
pub fn start(conf: Conf, path: PathBuf) {
	let mut core = Core::new().unwrap();
	let handle = core.handle();

	// bind to the server
    let addr = (conf.host, conf.server.port);
	let server = Server::bind(addr, &handle).unwrap();

    let mut gossip = GossipConf::new();
    
    if let Some(ref peer) = conf.peer {
        gossip = gossip.addr((conf.host, peer.port));
        gossip = gossip.id(peer.id());
        gossip = gossip.network_key(peer.network_key());
    }

    let instance = Instance::from_file(&path.join("music.db"), gossip);

	// a stream of incoming connections
	let f = server.incoming()
        // we don't wanna save the stream if it drops
        .map_err(|InvalidConnection { error, .. }| error)
        .for_each(|(upgrade, addr)| {
            println!("Got a connection from: {}", addr);
            // check if it has the protocol we want
            if !upgrade.protocols().iter().any(|s| s == "rust-websocket") {
                // reject it if it doesn't
                spawn_future(upgrade.reject(), "Upgrade Rejection", &handle);
                return Ok(());
            }

            let handle2 = handle.clone();
            let path_cpy = path.clone();
            let view = instance.view();

            // accept the request to be a ws connection if it does
            let f = upgrade
                .use_protocol("rust-websocket")
                .accept()
                .and_then(move |(s,_)| {
                    let now = Instant::now();
                    let mut state = State::new(handle2, &path_cpy, view);

                    let (sink, stream) = s.split();

                    stream
                    //.take_while(|m| Ok(!m.is_close()))
                    .filter_map(move |m| {
                        match m {
                            OwnedMessage::Ping(p) => Some(OwnedMessage::Pong(p)),
                            OwnedMessage::Pong(_) => None,
                            OwnedMessage::Text(msg) => Some(OwnedMessage::Text("Text not supported".into())),
                            OwnedMessage::Binary(data) => {
                                state.process(addr.to_string(), data).map(|x| OwnedMessage::Binary(x))
                            },
                            OwnedMessage::Close(_) => {
                                //state.collection.add_event(Action::Connect(now.elapsed().as_secs() as f32).with_origin(addr.to_string())).unwrap();

                                Some(OwnedMessage::Close(None))
                            },
                            _ => Some(m)
                        }
                    })
                    .forward(sink)
                    .and_then(move |(_, sink)| {
                        println!("BLUB");
                        sink.send(OwnedMessage::Close(None))
                    })
                });

            spawn_future(f, "Client Status", &handle);
            Ok(())
        });

    println!("Server is running!");

	core.run(f).unwrap();
}

fn spawn_future<F, I, E>(f: F, desc: &'static str, handle: &Handle)
	where F: Future<Item = I, Error = E> + 'static,
	      E: Debug
{
	handle.spawn(f.map_err(move |e| println!("{}: '{:?}'", desc, e))
	              .map(move |_| println!("{}: Finished.", desc)));
}
