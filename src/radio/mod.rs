//! Internet radio: the HAK.FM station list (SomaFM broadcaster streams) and
//! the public SomaFM song feed used for "now playing" and recent tracks.
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadioStation {
    pub name: &'static str,
    pub genre: &'static str,
    pub description: &'static str,
    pub slug: &'static str,
}

impl RadioStation {
    pub fn stream_url(&self) -> String {
        format!("https://ice5.somafm.com/{}-128-mp3", self.slug)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadioSong {
    pub title: String,
    pub artist: String,
    pub album: String,
}

pub const STATIONS: &[RadioStation] = &[
    RadioStation { name: "DEF CON Radio", genre: "HACKER", description: "The year-round DEF CON channel.", slug: "defcon" },
    RadioStation { name: "Groove Salad", genre: "FOCUS", description: "Downtempo grooves for long sessions.", slug: "groovesalad" },
    RadioStation { name: "Drone Zone", genre: "AMBIENT", description: "Slow atmospheric soundscapes.", slug: "dronezone" },
    RadioStation { name: "Deep Space One", genre: "AMBIENT", description: "Deep ambient for quiet work.", slug: "deepspaceone" },
    RadioStation { name: "The Dark Zone", genre: "DARK AMBIENT", description: "The darker end of deep ambient.", slug: "darkzone" },
    RadioStation { name: "Doomed", genre: "INDUSTRIAL", description: "Dark industrial and ambient.", slug: "doomed" },
    RadioStation { name: "cliqhop idm", genre: "IDM", description: "Blips, beeps, and intricate beats.", slug: "cliqhop" },
    RadioStation { name: "Groove Salad Classic", genre: "DOWNTEMPO", description: "Classic early-2000s chill.", slug: "gsclassic" },
    RadioStation { name: "Space Station Soma", genre: "SPACE", description: "Spaced-out ambient electronica.", slug: "spacestation" },
    RadioStation { name: "Mission Control", genre: "SPACE", description: "Mission audio woven into electronica.", slug: "missioncontrol" },
    RadioStation { name: "Vaporwaves", genre: "VAPORWAVE", description: "A hazy vaporwave signal.", slug: "vaporwaves" },
    RadioStation { name: "Underground 80s", genre: "SYNTH / NEW WAVE", description: "Early-80s synthpop and new wave.", slug: "u80s" },
    RadioStation { name: "Metal Detector", genre: "METAL", description: "From black metal to doom and thrash.", slug: "metal" },
    RadioStation { name: "Dub Step Beyond", genre: "BASS", description: "Dubstep, dub, and deep bass.", slug: "dubstep" },
    RadioStation { name: "Secret Agent", genre: "SPY JAZZ", description: "Spy jazz and lounge atmosphere.", slug: "secretagent" },
    RadioStation { name: "Sonic Universe", genre: "JAZZ", description: "Modern jazz and fusion.", slug: "sonicuniverse" },
    RadioStation { name: "Beat Blender", genre: "ELECTRONIC", description: "Downtempo and electronic grooves.", slug: "beatblender" },
    RadioStation { name: "The Trip", genre: "PSYCHEDELIC", description: "Psychedelic electronic journeys.", slug: "thetrip" },
    RadioStation { name: "Lush", genre: "CHILL", description: "Atmospheric vocals and electronic music.", slug: "lush" },
    RadioStation { name: "Illinois Street Lounge", genre: "LOUNGE", description: "Vintage lounge and exotica.", slug: "illstreet" },
    RadioStation { name: "Fluid", genre: "DOWNTEMPO", description: "Downtempo electronic music.", slug: "fluid" },
    RadioStation { name: "PopTron", genre: "INDIE", description: "Indie pop and alternative.", slug: "poptron" },
    RadioStation { name: "Black Rock FM", genre: "FESTIVAL", description: "Burning Man community radio.", slug: "brfm" },
];

#[derive(serde::Deserialize)]
struct SongFeed {
    songs: Vec<FeedSong>,
}

#[derive(serde::Deserialize)]
struct FeedSong {
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    album: String,
}

// Most recent first; the first entry is what's on air right now.
pub fn fetch_songs(station: &RadioStation) -> Result<Vec<RadioSong>, String> {
    let url = format!("https://somafm.com/songs/{}.json", station.slug);
    let feed: SongFeed = ureq::get(&url)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    Ok(feed.songs.into_iter()
        .filter(|s| !s.title.trim().is_empty())
        .map(|s| RadioSong { title: s.title, artist: s.artist, album: s.album })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stations_are_unique_and_use_somafm_streams() {
        for (i, station) in STATIONS.iter().enumerate() {
            assert!(STATIONS[i + 1..].iter().all(|other| other.slug != station.slug));
            assert_eq!(station.slug, station.slug.trim());
            assert!(station.stream_url().starts_with("https://ice5.somafm.com/"));
        }
    }
}
