//! Websocket server implementation
//!
//! The websocket uses Tokio under the hood and manages a state for each connection. It also shares
//! the latest token to all clients and logs every events concerning connecting and disconnecting. 

use std::fmt::Debug;
use std::time::Instant;
use std::rc::Rc;
use std::cell::RefCell;
use std::path::PathBuf;

use websocket::WebSocketError;
use websocket::message::OwnedMessage;
use websocket::server::InvalidConnection;
use websocket::async::Server;

use tokio_core::reactor::{Handle, Core};
use futures::{Future, Sink, Stream, sync::mpsc::{Sender, channel}};

use state::State;
use hex_conf::Conf;

use hex_server_protocol::{Answer, AnswerAction};
use hex_database::{Instance, GossipConf, TransitionAction};

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

    let mut instance = Instance::from_file(&path.join("music.db"), gossip);

    let broadcasts: Rc<RefCell<Vec<Sender<TransitionAction>>>> = Rc::new(RefCell::new(Vec::new()));

    let tmp = broadcasts.clone();
    let c = instance.recv().for_each(|x| {
        for i in &(*tmp.borrow()) {
            i.clone().send(x.clone()).wait().unwrap();
        }

        Ok(())
    });

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
            let (s, r) = channel(1024);

            broadcasts.borrow_mut().push(s);

            // accept the request to be a ws connection if it does
            let f = upgrade
                .use_protocol("rust-websocket")
                .accept()
                .and_then(move |(s,_)| {
                    let mut state = State::new(handle2, &path_cpy, view);

                    let (sink, stream) = s.split();

                    //sink.send(OwnedMessage::Close(None));

                    let stream = stream.filter_map(move |m| {
                        match m {
                            OwnedMessage::Ping(p) => Some(OwnedMessage::Pong(p)),
                            OwnedMessage::Pong(_) => None,
                            OwnedMessage::Text(_) => Some(OwnedMessage::Text("Text not supported".into())),
                            OwnedMessage::Binary(data) => {
                                state.process(addr.to_string(), data).map(|x| OwnedMessage::Binary(x))
                            },
                            OwnedMessage::Close(_) => {
                                //state.collection.add_event(Action::Connect(now.elapsed().as_secs() as f32).with_origin(addr.to_string())).unwrap();

                                Some(OwnedMessage::Close(None))
                            }
                        }
                    });

                    let push = r.and_then(|x| {
                        Answer::new([0u32; 4], Ok(AnswerAction::Transition(x))).to_buf()
                            .map(|x| OwnedMessage::Binary(x))
                            .map_err(|_| ())
                    }).map_err(|_| WebSocketError::NoDataAvailable);

                    Stream::select(stream, push)
                    .forward(sink)
                    .and_then(move |(_, sink)| {
                        println!("BLUB");
                        sink.send(OwnedMessage::Close(None))
                    })
                });

            spawn_future(f, "Client Status", &handle);
            Ok(())
        }).map_err(|_| ());

    println!("Server is running!");

	core.run(Future::join(f, c)).unwrap();
}

fn spawn_future<F, I, E>(f: F, desc: &'static str, handle: &Handle)
	where F: Future<Item = I, Error = E> + 'static,
	      E: Debug
{
	handle.spawn(f.map_err(move |e| println!("{}: '{:?}'", desc, e))
	              .map(move |_| println!("{}: Finished.", desc)));
}
