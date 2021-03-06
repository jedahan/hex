extern crate futures;
extern crate tokio;
extern crate getopts;
extern crate cpal;
extern crate rb;
extern crate nix;
extern crate terminal_size;

extern crate hex_conf;
extern crate hex_database;
extern crate hex_music_container;

mod audio;
mod play;
mod modify;
mod sync;

use std::io::{self, Write, BufRead};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;
use std::fs;
use std::path::{Path, PathBuf};
use hex_database::{Instance, View, search::SearchQuery, Track, GossipConf, TrackKey, Playlist};

use futures::Future;

fn main() {
    let (conf, path) = match hex_conf::Conf::new() {
        Ok(x) => x,
        Err(err) => {
            eprintln!("Error: Could not load configuration {:?}", err);
            (hex_conf::Conf::default(), PathBuf::from("/opt/music/"))
        }
    };
    let data_path = path.join("data");
    let db_path = path.join("music.db");

    let mut gossip = GossipConf::new();

    if let Some(ref peer) = conf.peer {
        gossip = gossip.addr((conf.host, peer.port));
        gossip = gossip.id(peer.id());
        gossip = gossip.network_key(peer.network_key());
    }

    let mut instance = Instance::from_file(&db_path, gossip);
    let view = instance.view();

    let (sender, receiver): (Sender<TrackKey>, Receiver<TrackKey>) = channel();
    let path_copy = data_path.clone();
    thread::spawn(move || {
        while let Ok(key) = receiver.recv() {
            if !path_copy.join(key.to_string()).exists() {
                instance.ask_for_file(key.to_vec()).wait().unwrap();
            }
        }
    });

    'outer: loop {
        print!(" > ");
        io::stdout().flush().ok().expect("Could not flush stdout");

        // get next line
        let line;

        let stdin = io::stdin();
        let mut iterator = stdin.lock().lines();

        loop {
            match iterator.next() {
                Some(Ok(e)) => { line = e; break; },
                Some(Err(_)) => continue,
                None => {
                    println!("");
                    continue 'outer
                }
            }
        }

        let mut args: Vec<&str> = line.splitn(2, ' ').collect();
        if args.len() == 0 {
            continue;
        } else if args.len() == 1 {
            args.push("");
        }

        let query = SearchQuery::new(&args[1]);
        let mut query = view.search_prep(query).unwrap();
        let tracks: Vec<Track> = view.search(&mut query).collect();

        let data_path = data_path.clone();
        let sender = sender.clone();
        match args[0] {
            "" => {
                print_overview(&view);
            },
            "show" => {
                show_tracks(&args[1], tracks);
            },
            "delete" => {
                delete_tracks(&view, &data_path, tracks);
            },
            "add-playlist" => {
                add_playlist(&view, tracks);
            },
            "sync" => {
                sync::sync_tracks(tracks, sender, data_path);
            },
            "play" => {
                for key in tracks.iter().map(|x| x.key.clone()) {
                    sender.send(key).unwrap();
                }

                play::play_tracks(data_path.clone(), tracks);
            },
            "modify" => {
                modify::modify_tracks(&view, tracks);
            },
            "quit" => {
                println!("Exit ..");
                return;
            },
            _ => {
                println!("Supported actions:");
                println!("  show, delete, add-playlist, sync, play, modify, quit");
            }
        }
    }
}

fn show_tracks(query: &str, tracks: Vec<Track>) {
    println!("Found {} tracks for query: `{}`", tracks.len(), query);
    println!("");

    for track in tracks {
        if let (Some(ref title), Some(ref interpret)) = (&track.title, &track.interpret) {
            println!("\t{} ## {}", title, interpret);
        }
    }

}

fn add_playlist(db: &View, tracks: Vec<Track>) {
    println!("Create new playlist with {} tracks", tracks.len());

    let last_key = db.last_playlist_key().unwrap();
    let pl = Playlist {
        key: last_key + 1,
        title: "New Playlist".into(),
        desc: None,
        tracks: tracks.into_iter().map(|x| x.key).collect(),
        origin: vec![0; 16]
    };

    db.add_playlist(pl).unwrap();
}

fn delete_tracks(db: &View, data_path: &Path, tracks: Vec<Track>) {
    print!("Do you really want to delete {} tracks [n]: ", tracks.len());
    io::stdout().flush().unwrap();

    let mut input = String::new();
    match io::stdin().read_line(&mut input) {
        Ok(_) => {
            if input != "y\n" {
                return;
            }
        },
        Err(err) => {
            eprintln!("Error: {}", err);
            return;
        }
    }

    for track in tracks {
        db.delete_track(track.key).unwrap();

       if fs::remove_file(data_path.join(track.key.to_path())).is_err() {
           eprintln!("Error: Could not remove file of track {}", track.key.to_string());
       }
    }
}

fn print_overview(db: &View) {
    let mut tracks = db.get_tracks();
    tracks.sort_by(|a, b| a.favs_count.cmp(&b.favs_count).reverse());

    let duration = tracks.iter().fold(0.0, |y,x| y + x.duration);

    println!(" => Found {} tracks in total length {} min", tracks.len(), (duration / 60.0).floor());

    for track in tracks.iter().take(10) {
        if let (Some(ref title), Some(ref interpret)) = (&track.title, &track.interpret) {
            println!("\t{} ## {}", title, interpret);
        }
    }

    println!("");

    let playlists = db.get_playlists();

    println!(" => Found {} playlists:", playlists.len());

    for pl in playlists {
        println!("\t{} with {} tracks", pl.title, pl.tracks.len());
    }
}
