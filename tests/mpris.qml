import QtQuick
import Quickshell
import Quickshell.Services.Mpris

ShellRoot {
    id: root
    property var players: Mpris.players.values
    property var spectrumComponent: Quickshell.env("STASH_TEST_MEDIA_STATE")
        ? Qt.createComponent(Quickshell.env("STASH_TEST_MEDIA_STATE")) : null
    property var spectrumMedia: spectrumComponent && spectrumComponent.status === Component.Ready
        ? spectrumComponent.createObject(root) : null
    Timer {
        interval: 1500
        running: true
        onTriggered: {
            const player = players.find(p => p.identity === "Stash MPRIS test");
            console.log("Players:", Mpris.players.values.length, "identity:", player?.identity,
                "playing:", player?.isPlaying, "title:", player?.trackTitle,
                "artists:", JSON.stringify(player?.trackArtists), "album:", player?.trackAlbum,
                "art:", player?.trackArtUrl);
            const playerValid = player && player.isPlaying
                && player.trackTitle === "Test song"
                && player.trackArtist === "Test artist"
                && player.trackAlbum === "Test album"
                && player.trackArtUrl === "file:///tmp/album%20cover%23.png";
            const spectrumValid = !Quickshell.env("STASH_TEST_MEDIA_STATE") || (spectrumMedia
                && spectrumMedia.state.title === "Test song"
                && spectrumMedia.state.artist === "Test artist"
                && spectrumMedia.state.album === "Test album"
                && spectrumMedia.state.artUrl === "file:///tmp/album%20cover%23.png");
            const valid = playerValid && spectrumValid;
            console.log("SpectrumOS MPRIS client:", valid ? "PASS" : "FAIL");
            Qt.exit(valid ? 0 : 1);
        }
    }
}
