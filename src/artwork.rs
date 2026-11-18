//! Progressive artwork enrichment, run exclusively by the cover lookup worker.
use lofty::{
    config::{ParseOptions, WriteOptions},
    picture::{Picture, PictureType},
    prelude::*,
    probe::Probe,
    tag::Tag,
};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_IMAGE: u64 = 10 * 1024 * 1024;

pub fn resolve(track: &Path, download: bool, embed: bool) -> Option<PathBuf> {
    let cover = crate::app::App::find_cover_art(track)
        .or_else(|| download.then(|| fetch(track)).flatten())?;
    if embed {
        // Failure to write (read-only media, unsupported format, etc.) must not
        // prevent displaying the cached artwork or interrupt playback.
        let _ = embed_cover(track, &cover);
    }
    Some(cover)
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn matching_releases(json: &serde_json::Value, artist: &str, album: &str) -> Vec<String> {
    json["releases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            normalize(r["title"].as_str().unwrap_or("")) == normalize(album)
                && r["artist-credit"].as_array().is_some_and(|credits| {
                    credits.iter().any(|c| {
                        [c["name"].as_str(), c["artist"]["name"].as_str()]
                            .into_iter()
                            .flatten()
                            .any(|name| normalize(name) == normalize(artist))
                    })
                })
        })
        .filter_map(|r| r["id"].as_str())
        .filter(|id| id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .take(3)
        .map(str::to_owned)
        .collect()
}

fn fetch(track: &Path) -> Option<PathBuf> {
    let file = Probe::open(track)
        .ok()?
        .options(ParseOptions::new().read_properties(false))
        .read()
        .ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let artist = tag
        .get_string(&lofty::tag::ItemKey::AlbumArtist)
        .map(str::to_owned)
        .or_else(|| tag.artist().map(|s| s.into_owned()))?;
    let album = tag.album()?.into_owned();
    if normalize(&artist).is_empty() || normalize(&album).is_empty() {
        return None;
    }
    let cache = dirs::cache_dir()?.join("stash/album_art");
    std::fs::create_dir_all(&cache).ok()?;
    let key = format!(
        "{:x}",
        Sha256::digest(format!("{}\n{}", normalize(&artist), normalize(&album)))
    );
    let target = cache.join(format!("album-{key}.jpg"));
    if crate::preview::load_image(&target).is_ok() {
        return Some(target);
    }
    let attempt = cache.join(format!("album-{key}.attempt"));
    if std::fs::metadata(&attempt)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(3600))
    {
        return None;
    }
    std::fs::write(&attempt, b"").ok()?;
    // One lookup worker; space searches even when songs are skipped quickly.
    std::thread::sleep(Duration::from_millis(1100));
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .build();
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let query = format!(
        "release:\"{}\" AND artist:\"{}\"",
        quote(&album),
        quote(&artist)
    );
    let response = agent
        .get("https://musicbrainz.org/ws/2/release/")
        .set(
            "User-Agent",
            concat!(
                "stash/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/gibranlp/stash)"
            ),
        )
        .query("query", &query)
        .query("fmt", "json")
        .query("limit", "10")
        .call()
        .ok()?;
    let mut json_bytes = Vec::new();
    response
        .into_reader()
        .take(512 * 1024)
        .read_to_end(&mut json_bytes)
        .ok()?;
    let json = serde_json::from_slice(&json_bytes).ok()?;
    for id in matching_releases(&json, &artist, &album) {
        let url = format!("https://coverartarchive.org/release/{id}/front-500");
        let Ok(response) = agent.get(&url).call() else {
            continue;
        };
        let mut bytes = Vec::new();
        if response
            .into_reader()
            .take(MAX_IMAGE + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() as u64 > MAX_IMAGE
        {
            continue;
        }
        let Ok(mut temp) = tempfile::NamedTempFile::new_in(&cache) else {
            return None;
        };
        if temp.write_all(&bytes).is_err() || crate::preview::load_image(temp.path()).is_err() {
            continue;
        }
        if temp.persist(&target).is_ok() {
            return Some(target);
        }
    }
    None
}

fn embed_cover(track: &Path, cover: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(track)?;
    // Atomic replacement of a symlink or hard link would change its meaning.
    if !metadata.is_file() || metadata.permissions().readonly() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Ok(());
        }
    }
    let mut tagged = Probe::open(track)?
        .options(ParseOptions::new().read_properties(false))
        .read()?;
    if tagged.tags().iter().any(|tag| !tag.pictures().is_empty()) {
        return Ok(());
    }
    let kind = tagged.file_type().primary_tag_type();
    // Only use tag formats with established embedded-picture support.
    if !matches!(
        kind,
        lofty::tag::TagType::Id3v2
            | lofty::tag::TagType::VorbisComments
            | lofty::tag::TagType::Mp4Ilst
            | lofty::tag::TagType::Ape
    ) {
        return Ok(());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(cover)?
        .take(MAX_IMAGE + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= MAX_IMAGE, "Cover too large");
    let mut picture = Picture::from_reader(&mut Cursor::new(bytes))?;
    picture.set_pic_type(PictureType::CoverFront);
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(kind));
    }
    tagged.primary_tag_mut().unwrap().push_picture(picture);
    // Edit a sibling copy then replace atomically: the decoder can continue
    // reading its existing open file and failed writes leave the original intact.
    let suffix = format!(
        ".{}",
        track
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("audio")
    );
    let temp = tempfile::Builder::new()
        .prefix(".stash-art-")
        .suffix(&suffix)
        .tempfile_in(track.parent().unwrap_or(Path::new(".")))?;
    std::fs::copy(track, temp.path())?;
    tagged.save_to_path(temp.path(), WriteOptions::default())?;
    let check = Probe::open(temp.path())?.read()?;
    anyhow::ensure!(
        check.tags().iter().any(|t| !t.pictures().is_empty()),
        "Artwork was not saved"
    );
    let current = std::fs::metadata(track)?;
    anyhow::ensure!(
        current.len() == metadata.len() && current.modified()? == metadata.modified()?,
        "Track changed during artwork lookup"
    );
    std::fs::set_permissions(temp.path(), metadata.permissions())?;
    temp.as_file().sync_all()?;
    temp.persist(track)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embeds_without_changing_audio_or_existing_art() {
        for (extension, data) in [
            (
                "mp3",
                include_bytes!("../tests/fixtures/artwork.mp3").as_slice(),
            ),
            (
                "flac",
                include_bytes!("../tests/fixtures/artwork.flac").as_slice(),
            ),
            (
                "ogg",
                include_bytes!("../tests/fixtures/artwork.ogg").as_slice(),
            ),
            (
                "m4a",
                include_bytes!("../tests/fixtures/artwork.m4a").as_slice(),
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let track = dir.path().join(format!("track.{extension}"));
            std::fs::write(&track, data).unwrap();
            let cover = dir.path().join("cover.png");
            image::RgbImage::from_pixel(4, 4, image::Rgb([255, 0, 0]))
                .save(&cover)
                .unwrap();
            let decode = |path: &Path| {
                let output = std::process::Command::new("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(path)
                    .args(["-map", "0:a:0", "-f", "s16le", "-"])
                    .output();
                match output {
                    Ok(output) => {
                        assert!(
                            output.status.success(),
                            "{extension}: {}",
                            String::from_utf8_lossy(&output.stderr)
                        );
                        assert!(!output.stdout.is_empty());
                        Some(output.stdout)
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => panic!("{e}"),
                }
            };
            let samples = decode(&track);
            let before = Probe::open(&track).unwrap().read().unwrap();
            embed_cover(&track, &cover).unwrap_or_else(|e| panic!("{extension}: {e}"));
            let after = Probe::open(&track).unwrap().read().unwrap();
            assert_eq!(
                before.properties().duration(),
                after.properties().duration(),
                "{extension}"
            );
            assert_eq!(
                samples,
                decode(&track),
                "Audio samples changed for {extension}"
            );
            assert_eq!(
                after.primary_tag().unwrap().title().as_deref(),
                Some("Artwork test")
            );
            let picture = &after.primary_tag().unwrap().pictures()[0];
            if extension != "m4a" {
                assert_eq!(picture.pic_type(), PictureType::CoverFront);
            }
            assert_eq!(picture.data(), std::fs::read(&cover).unwrap());
            let saved = std::fs::read(&track).unwrap();
            embed_cover(&track, &cover).unwrap();
            assert_eq!(saved, std::fs::read(&track).unwrap());
        }
    }

    #[test]
    fn invalid_art_leaves_original_intact() {
        let dir = tempfile::tempdir().unwrap();
        let track = dir.path().join("track.mp3");
        let original = include_bytes!("../tests/fixtures/artwork.mp3");
        std::fs::write(&track, original).unwrap();
        let cover = dir.path().join("cover.jpg");
        std::fs::write(&cover, b"not an image").unwrap();
        assert!(embed_cover(&track, &cover).is_err());
        assert_eq!(std::fs::read(track).unwrap(), original);
    }

    #[test]
    fn only_matches_album_and_artist() {
        let json = serde_json::json!({"releases": [
            {"id":"12345678-1234-1234-1234-123456789abc", "title":"The Album", "artist-credit":[{"name":"Artist"}]},
            {"id":"aaaaaaaa-1234-1234-1234-123456789abc", "title":"The Album (Live)", "artist-credit":[{"name":"Artist"}]},
            {"id":"bbbbbbbb-1234-1234-1234-123456789abc", "title":"The Album", "artist-credit":[{"name":"Other"}]}
        ]});
        assert_eq!(matching_releases(&json, "ARTIST", "The Album").len(), 1);
        assert!(matching_releases(&json, "Unknown", "The Album").is_empty());
    }
}
