use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;

use crate::now_playing::NowPlaying;

pub fn spawn_monitor(
    state: Arc<RwLock<Option<NowPlaying>>>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while running.load(Ordering::Acquire) {
            let next = read_now_playing();
            if let Ok(mut guard) = state.write() {
                *guard = next;
            }

            for _ in 0..20 {
                if !running.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    })
}

fn read_now_playing() -> Option<NowPlaying> {
    let output = Command::new(media_control_command())
        .arg("get")
        .arg("--no-artwork")
        .output();
    let output = match output {
        Ok(output) if output.status.success() => output,
        Ok(_) | Err(_) => return None,
    };

    parse_now_playing(&output.stdout)
}

fn media_control_command() -> PathBuf {
    env::var_os("SOUNDBAR_MEDIA_CONTROL")
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            option_env!("SOUNDBAR_BUNDLED_MEDIA_CONTROL")
                .map(PathBuf::from)
                .filter(|path| path.exists())
        })
        .unwrap_or_else(|| PathBuf::from("media-control"))
}

fn parse_now_playing(bytes: &[u8]) -> Option<NowPlaying> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let title = string_field(&value, "title");
    let artist = string_field(&value, "artist");
    let album = string_field(&value, "album");
    let app = string_field(&value, "bundleIdentifier")
        .or_else(|| string_field(&value, "displayName"))
        .or_else(|| string_field(&value, "application"));
    let playing = bool_field(&value, "playing")
        .or_else(|| string_field(&value, "playbackState").map(|state| state == "playing"))
        .unwrap_or(false);

    (title.is_some() || artist.is_some() || album.is_some() || app.is_some()).then_some(
        NowPlaying {
            title,
            artist,
            album,
            app,
            playing,
        },
    )
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn bool_field(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

#[cfg(test)]
mod tests {
    use super::parse_now_playing;

    #[test]
    fn parses_media_control_payload() {
        let payload = br#"{
            "title": "30 For 30",
            "artist": "SZA",
            "album": "SOS Deluxe: LANA",
            "bundleIdentifier": "com.apple.Music",
            "playing": true
        }"#;

        let now_playing = parse_now_playing(payload).expect("payload should parse");

        assert_eq!(now_playing.title.as_deref(), Some("30 For 30"));
        assert_eq!(now_playing.artist.as_deref(), Some("SZA"));
        assert_eq!(now_playing.album.as_deref(), Some("SOS Deluxe: LANA"));
        assert_eq!(now_playing.app.as_deref(), Some("com.apple.Music"));
        assert!(now_playing.playing);
    }
}
