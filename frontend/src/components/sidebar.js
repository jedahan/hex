import { h, Component } from 'preact';
import { Icon, Layout, Navigation } from 'preact-mdl';
import style from 'Style/sidebar';
import Protocol from 'Lib/protocol';

export default class Sidebar extends Component {
	state = {
        playlists: [],
        create: false
    };
    
	hide = () => {
        let elm = this.base.parentNode.getElementsByClassName("mdl-layout__obfuscator")[0];
        elm.classList.remove('is-visible');
		this.base.classList.remove('is-visible');
	};

    componentDidMount() {
        let self = this;
        Protocol.get_playlists().then(x => {
            self.setState({playlists: x});
        });

        Protocol.ontransition(action => {
            if("DeletePlaylist" in action["Transition"]) {
                let playlists = self.state.playlists.filter(x => x.key != action["Transition"]["DeletePlaylist"]);
                console.log(playlists)
                self.setState({playlists});
            }

            if("UpsertPlaylist" in action["Transition"]) {
                const index = self.state.playlists.findIndex(e => e.key == action["Transition"]["UpsertPlaylist"].key);

                let playlists = self.state.playlists;
                if(index === -1)
                    playlists.push(action["Transition"]["UpsertPlaylist"]);
                else
                    playlists[index] = action["Transition"]["UpsertPlaylist"];

                self.setState({playlists});
            }
        });
    }

    click(e) {
        this.setState({create: true});
        e.stopPropagation();
    }

    add_playlist(e) {
        if(e.keyCode && e.keyCode != 13)
            return;

        const name = this.elm_name.value;

        let self = this;
        Protocol.add_playlist(name).then(new_pl => {
            let playlists = self.state.playlists;
            playlists.push(new_pl);

            this.setState({playlists: playlists, create: false});
        });

        e.stopPropagation();
    }


	render({},{playlists, create}) {
		return (
			<Layout.Drawer onClick={this.hide.bind(this)}>
				<Layout.Title>Example App</Layout.Title>
				<Navigation>
					<Navigation.Link href="/" class={style.link}><Icon icon="home" /><b>Übersicht</b></Navigation.Link>
					<Navigation.Link href="/Verlauf" class={style.link}><Icon icon="history" /><b>Verlauf</b></Navigation.Link>
                    <div class={style.line} />
                    <div class={style.header}>Playlists
                        { !create && (
                            <Icon icon="add" onClick={this.click.bind(this)} />
                        )}
                    </div>
                    { create && (
                        <div class={style.add_playlist}><input placeholder="Name" onClick={e => e.stopPropagation()} ref={x => this.elm_name = x} onKeyup={this.add_playlist.bind(this)}/><Icon icon="add" onClick={this.add_playlist.bind(this)} /> </div>
                    )}
                    { playlists && playlists.map( x => (
                        <Navigation.Link href={"/playlist/" + x.key} class={style.link}><Icon icon="queue music" /><b>{x.title}</b><span>{x.tracks.length}</span></Navigation.Link>
                    ))}
				</Navigation>
			</Layout.Drawer>
		);
	}
}
