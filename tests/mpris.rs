#![cfg(target_os = "linux")]

use std::process::Command;
use std::time::Duration;

// Run under a private bus: dbus-run-session -- cargo test --test mpris -- --ignored
#[test]
#[ignore = "requires dbus-run-session and playerctl"]
fn publishes_metadata_after_a_burst_of_playback_updates() {
    let mut controls = souvlaki::MediaControls::new(souvlaki::PlatformConfig {
        dbus_name: "stash_test",
        display_name: "Stash MPRIS test",
        hwnd: None,
    })
    .unwrap();
    controls.attach(|_| {}).unwrap();
    for seconds in 0..20 {
        controls
            .set_playback(souvlaki::MediaPlayback::Playing {
                progress: Some(souvlaki::MediaPosition(Duration::from_secs(seconds))),
            })
            .unwrap();
    }
    controls
        .set_metadata(souvlaki::MediaMetadata {
            title: Some("Test song"),
            artist: Some("Test artist"),
            album: Some("Test album"),
            cover_url: Some("file:///tmp/album%20cover%23.png"),
            duration: Some(Duration::from_secs(180)),
        })
        .unwrap();
    // The old backend would still have most updates queued after this delay.
    std::thread::sleep(Duration::from_millis(800));
    let result = Command::new("playerctl")
        .args([
            "--player",
            "stash_test",
            "metadata",
            "--format",
            "{{title}}|{{artist}}|{{album}}|{{mpris:artUrl}}|{{mpris:length}}",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout).trim(),
        "Test song|Test artist|Test album|file:///tmp/album%20cover%23.png|180000000"
    );

    // Exercise the same client library used by SpectrumOS, when installed.
    if Command::new("qs").arg("--version").output().is_ok() {
        let qml = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mpris.qml");
        let mut command = Command::new("timeout");
        let installed_media = "/usr/share/spectrumos/spectrum-shell/MediaState.qml";
        if std::path::Path::new(installed_media).exists() {
            command.env(
                "STASH_TEST_MEDIA_STATE",
                format!("file://{installed_media}"),
            );
        }
        let result = command
            .args(["10s", "qs", "--path", qml.to_str().unwrap()])
            .env("QT_QPA_PLATFORM", "offscreen")
            .env("XDG_CACHE_HOME", "/tmp/stash-mpris-test-cache")
            .env("XDG_STATE_HOME", "/tmp/stash-mpris-test-state")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    controls.detach().unwrap();
}
