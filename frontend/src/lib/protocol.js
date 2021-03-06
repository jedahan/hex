import { guid } from './uuid.js'
const _proto = import(/* webpackChunkName: "hex_server_protocol" */ './hex_server_protocol');

_proto.catch(x => console.log("REJECT: " + x));

const CALLS = {
    Search: ["query"],
    GetTrack: ["key"],
    StreamNext: ["key"],
    StreamEnd: [],
    StreamSeek: ["sample"],
    UpdateTrack: ["key", "title", "album", "interpret", "people", "composer"],
    GetSuggestion: ["key"],
    AddPlaylist: ["name"],
    DeletePlaylist: ["key"],
    SetPlaylistImage: ["key"],
    AddToPlaylist: ["key", "playlist"],
    DeleteFromPlaylist: ["key", "playlist"],
    UpdatePlaylist: ["key", "title", "desc"],
    GetPlaylists: [],
    GetPlaylist: ["key"],
    GetPlaylistsOfTrack: ["key"],
    DeleteTrack: ["key"],
    UploadYoutube: ["path"],
    VoteForTrack: ["key"],
    AskUploadProgress: [],
    GetToken: ["token"],
    UpdateToken: ["token", "key", "played", "pos"],
    CreateToken: [],
    LastToken: [],
    GetSummary: [],
    GetTransitions: [],
    Download: ["format", "tracks"],
    AskDownloadProgress: []
}

let proto = null;

class Protocol {
    constructor() {
        let self = this;
        this.buffered_requests = [];
        this.pending_requests = {};
        this.transaction_fncs = [];

        // create function calls to the protocol
        for(const call in CALLS) {
            // convert CamelCase to underscore_case for function calls
            const under = call.split(/(?=[A-Z])/).join('_').toLowerCase();
            if(CALLS[call].length == 0)
                this[under] = new Function("", "return this.request('" + call + "', null);");
            else
                this[under] = new Function(CALLS[call].join(", "), "return this.request('" + call + "', {" + CALLS[call].join(",") + "});");
        }

        _proto.then(x => {
            proto = x;
            self.try_connect();
        });

        return this;
    }

    try_connect() {
        this.socket = new WebSocket('ws://localhost:2794', "rust-websocket");
        this.socket.binaryType = 'arraybuffer';

        let self = this;
        this.socket.onclose = function(e) {
            console.error("Connection to " + this.url + " closed with code " + e.code + "!");

            setTimeout(_ => self.try_connect(), 500);
        }

        this.socket.onmessage = this.message.bind(this);

        this.socket.onopen = function() {
            console.log("Connection opened!");
            const buffered = self.buffered_requests.splice(0, self.buffered_requests.length);

            for(const idx in buffered) {
                const [id, req] = buffered[idx];

                const buf = proto.request_to_buf(id, req);
                self.socket.send(buf.buffer);
            }
        }

        this.socket.onerror = function(err) {
            console.log("Got error");
            console.log(err)
        }
    }

    ontransition(fn) {
        this.transaction_fncs.push(fn);
    }

    message(msg) {
        let answ = new proto.Wrapper(new Uint8Array(msg.data));
        
        if(!answ)
            console.error("Could not parse answer!");
        
        const id = answ.id();
        if(id[0] == 0 && id[1] == 0 && id[2] == 0 && id[3] == 0) {
            for(const fn in this.transaction_fncs) {
                this.transaction_fncs[fn](answ.action())
            }

            return;
        } else if(!id) {
            console.log(new Uint8Array(msg.data));
            console.error("Could not parse answer!");
            return;
        }

        if(this.pending_requests[id] == null) {
            console.error("Got answer without request!");
            return;
        }
        
        const [type, resolve, reject] = this.pending_requests[id];
        let action = answ.action();
        console.log(action);
        console.log(id);
            
        if(typeof action === "string" && action != type) {
            reject(action);
            return;
        }
        
        //console.log("Answer");
        //console.log(action);

        const key = Object.keys(action)[0];
        action = action[key];
        
        /*const pack_type = Object.keys(action)[0];
        console.log(pack_type);
        if(type != pack_type) {
            reject("Got packet with invalid type!");
            return;
        }*/
        
        answ = null;
        msg = null;
        delete this.pending_requests[id];

        resolve(action);
    }

    dice_id() {
        return Array.from({length: 4}, () => Math.floor(Math.random() * (2 ** 32)));
    }

    request(type, param, id) {
        if(id == null)
            id = this.dice_id();

        let req = {};
        req[type] = param;

        const promise = new Promise((resolve, reject) => this.pending_requests[id] = [type, resolve, reject]);

        if(!proto || this.socket.readyState != WebSocket.OPEN) {
            this.buffered_requests.push([id, req]);
        
            return promise;
        }

        const buf = proto.request_to_buf(id, req);
        
        if(!buf) {
            console.error("Could not serialize packet: " + JSON.stringify(req));
            return Promise.reject("could not serialize");
        }

        this.socket.send(buf.buffer);

        return promise;
    }

    start_search(query) {
        const id = this.dice_id();

        let self = this;
        return function() {
            return self.request('Search', {'query': query}, id);
        };
    }

    upload_track(name, format, data) {
        const id = this.dice_id();
        const promise = new Promise((resolve, reject) => this.pending_requests[id] = ["UploadTrack", resolve, reject]);

        let self = this;
        var arrayBuffer;
        var fileReader = new FileReader();
        fileReader.onload = function(event) {
            arrayBuffer = new Uint8Array(event.target.result);
            const buf = proto.upload_track(id, name, format, arrayBuffer);

            if(!buf) {
                console.error("Could not serialize packet: " + JSON.stringify(req));
                promise.reject(JSON.stringify(req));
            }

            self.socket.send(buf.buffer);

        };
        fileReader.readAsArrayBuffer(data);

        return promise;
    }

    start_stream(key) {
        const id = this.dice_id();

        let self = this;
        let first = true;
        return [
            function() {
                if(first) {
                    first = false;
                    return self.request("StreamNext", {"key": key}, id);
                } else 
                    return self.request("StreamNext", {"key": null}, id);
            },
            function(sample) {
                return self.request("StreamSeek", {"sample": sample}, id);
            },
            function() {
                return self.request("StreamEnd", null, id);
            }
        ];
    }

    get_suggestions(keys) {
        var promises = [];
        for(const key of keys) {
            promises.push(this.get_suggestion(key));
        }

        return Promise.all(promises);
    }

    upload_tracks(tracks) {
        let promises = [];
        for(const track of tracks) {
            promises.push(this.upload_track(track[0], track[1], track[2]));
        }

        return Promise.all(promises);
    }
}

export default new Protocol();
