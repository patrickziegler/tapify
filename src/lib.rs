pub mod mpris;

use crate::mpris::PlayerProxy;
use futures_util::StreamExt;
use id3::TagLike;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use zbus::{fdo::DBusProxy, interface, names::BusName, zvariant::OwnedValue};

#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    pub track_id: String,
    pub title: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub art_url: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
}

pub struct RecordingSession {
    pub child: Child,
    pub temp_path: PathBuf,
    pub track: TrackInfo,
}

#[derive(
    Debug, Clone, Copy, PartialEq, zbus::zvariant::Type, serde::Serialize, serde::Deserialize,
)]
#[repr(u8)]
pub enum ConnectionStatus {
    Disconnected = 0,
    Connected = 1,
}

impl TryFrom<OwnedValue> for ConnectionStatus {
    type Error = zbus::zvariant::Error;
    fn try_from(v: OwnedValue) -> Result<Self, Self::Error> {
        let s: &str = v.downcast_ref()?;
        match s {
            "Connected" => Ok(ConnectionStatus::Connected),
            _ => Ok(ConnectionStatus::Disconnected),
        }
    }
}

pub async fn exporter_task(mut rx: mpsc::Receiver<RecordingSession>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let music_dir = PathBuf::from(home).join("Music").join("Spotify");

    while let Some(mut session) = rx.recv().await {
        let track = session.track;
        let temp_path = session.temp_path;

        info!("Exporting track: {} - {}", 
            track.artist.as_deref().unwrap_or("Unknown Artist"),
            track.title.as_deref().unwrap_or("Unknown Title")
        );

        // 1. Wait for pw-record to finish and close the file
        match session.child.wait().await {
            Ok(status) => {
                if !status.success() {
                    warn!("pw-record exited with error: {}", status);
                }
            }
            Err(e) => {
                error!("Failed to wait for pw-record: {}", e);
            }
        }

        // 2. Small delay to ensure FS synchronization
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        if !temp_path.exists() {
            error!("Recording file {:?} does not exist after process exit", temp_path);
            continue;
        }

        let artist = track.artist.as_deref().unwrap_or("Unknown Artist");
        let album = track.album.as_deref().unwrap_or("Unknown Album");
        let title = track.title.as_deref().unwrap_or("Unknown Title");
        let track_number = track.track_number.unwrap_or(0);

        let dest_dir = music_dir.join(artist).join(album);
        if let Err(e) = tokio::fs::create_dir_all(&dest_dir).await {
            error!("Failed to create directory {:?}: {}", dest_dir, e);
            continue;
        }

        let file_name = format!("{:02} - {}.wav", track_number, title);
        let dest_path = dest_dir.join(file_name);

        // Apply ID3 tags
        let track_for_tags = track.clone();
        let temp_path_for_tags = temp_path.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || {
            apply_tags(&temp_path_for_tags, &track_for_tags)
        }).await.unwrap() {
            warn!("Failed to apply tags to {:?}: {}", temp_path, e);
        }

        if let Err(e) = move_file(&temp_path, &dest_path).await {
            error!("Failed to move file to {:?}: {}", dest_path, e);
        } else {
            info!("Track exported to {:?}", dest_path);
        }
    }
}

async fn move_file(source: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    if let Err(e) = tokio::fs::rename(source, dest).await {
        if e.raw_os_error() == Some(18) {
            // Invalid cross-device link, fallback to copy and delete
            tokio::fs::copy(source, dest).await?;
            tokio::fs::remove_file(source).await?;
            Ok(())
        } else {
            Err(e)
        }
    } else {
        Ok(())
    }
}

fn apply_tags(path: &std::path::Path, track: &TrackInfo) -> anyhow::Result<()> {
    let mut tag = id3::Tag::new();
    tag.set_title(track.title.as_deref().unwrap_or("Unknown Title"));
    tag.set_artist(track.artist.as_deref().unwrap_or("Unknown Artist"));
    tag.set_album(track.album.as_deref().unwrap_or("Unknown Album"));
    if let Some(artist) = &track.album_artist {
        tag.set_album_artist(artist);
    }
    if let Some(n) = track.track_number {
        tag.set_track(n as u32);
    }
    if let Some(n) = track.disc_number {
        tag.set_disc(n as u32);
    }

    // Download album art
    if let Some(art_url) = &track.art_url {
        if let Ok(response) = reqwest::blocking::get(art_url) {
            if let Ok(bytes) = response.bytes() {
                tag.add_frame(id3::frame::Picture {
                    mime_type: "image/jpeg".to_string(), // Spotify art is usually jpeg
                    picture_type: id3::frame::PictureType::CoverFront,
                    description: "Album Art".to_string(),
                    data: bytes.to_vec(),
                });
            }
        }
    }

    tag.write_to_path(path, id3::Version::Id3v24)?;
    Ok(())
}

pub struct ServiceControl {
    pub recording_enabled: bool,
    pub connection_status: ConnectionStatus,
    pub current_track: Option<TrackInfo>,
    pub waiting_for_next_track: bool,
}

impl ServiceControl {
    pub fn new() -> Self {
        Self {
            recording_enabled: false,
            connection_status: ConnectionStatus::Disconnected,
            current_track: None,
            waiting_for_next_track: false,
        }
    }
}

#[interface(name = "org.spotify_recorder.Control")]
impl ServiceControl {
    #[zbus(property)]
    fn recording_enabled(&self) -> bool {
        self.recording_enabled
    }

    #[zbus(property)]
    pub async fn set_recording_enabled(&mut self, #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>, enabled: bool) {
        if self.recording_enabled != enabled {
            self.recording_enabled = enabled;
            info!(
                "Recording mode: {}",
                if enabled { "Enabled" } else { "Disabled" }
            );
            if enabled {
                self.waiting_for_next_track = true;
            } else {
                self.waiting_for_next_track = false;
            }
            self.recording_enabled_changed(&emitter).await.unwrap_or_default();
        }
    }

    #[zbus(property)]
    fn connection_status(&self) -> String {
        match self.connection_status {
            ConnectionStatus::Connected => "Connected".to_string(),
            ConnectionStatus::Disconnected => "Disconnected".to_string(),
        }
    }

    #[zbus(property)]
    fn current_song(&self) -> String {
        self.current_track
            .as_ref()
            .map(|t| {
                format!(
                    "{} - {}",
                    t.artist.as_deref().unwrap_or("Unknown Artist"),
                    t.title.as_deref().unwrap_or("Unknown Title")
                )
            })
            .unwrap_or_else(|| "None".to_string())
    }
}

pub async fn run_service(
    connection: zbus::Connection,
    spotify_bus_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (tx, rx) = mpsc::channel(10);
    tokio::spawn(exporter_task(rx));

    let dbus_proxy = DBusProxy::new(&connection).await?;
    let mut name_owner_changed = dbus_proxy.receive_name_owner_changed().await?;

    let mut monitor_handle: Option<JoinHandle<()>> = None;

    // Initial check
    let spotify_bus_name_owned = BusName::try_from(spotify_bus_name)?;
    if let Ok(owner) = dbus_proxy.get_name_owner(spotify_bus_name_owned).await {
        info!("Spotify found: {}", owner);
        update_connection_status(&connection, ConnectionStatus::Connected).await;
        monitor_handle = Some(tokio::spawn(monitor_spotify(
            connection.clone(),
            spotify_bus_name.to_string(),
            tx.clone(),
        )));
    }

    while let Some(signal) = name_owner_changed.next().await {
        let args = signal.args()?;
        if args.name() == spotify_bus_name {
            if let Some(_new_owner) = args.new_owner().as_ref() {
                info!("Spotify appeared");
                if let Some(handle) = monitor_handle.take() {
                    handle.abort();
                }
                update_connection_status(&connection, ConnectionStatus::Connected).await;
                monitor_handle = Some(tokio::spawn(monitor_spotify(
                    connection.clone(),
                    spotify_bus_name.to_string(),
                    tx.clone(),
                )));
            } else {
                warn!("Spotify disappeared");
                if let Some(handle) = monitor_handle.take() {
                    handle.abort();
                }
                update_connection_status(&connection, ConnectionStatus::Disconnected).await;
            }
        }
    }

    Ok(())
}

async fn update_connection_status(connection: &zbus::Connection, status: ConnectionStatus) {
    if let Ok(iface_ref) = connection
        .object_server()
        .interface::<_, ServiceControl>("/org/spotify_recorder/Control")
        .await
    {
        let mut iface = iface_ref.get_mut().await;
        if iface.connection_status != status {
            iface.connection_status = status;
            if status == ConnectionStatus::Disconnected {
                iface.current_track = None;
                iface
                    .current_song_changed(iface_ref.signal_emitter())
                    .await
                    .unwrap_or_default();
            }
            iface
                .connection_status_changed(iface_ref.signal_emitter())
                .await
                .unwrap_or_default();
        }
    }
}

pub async fn monitor_spotify(
    connection: zbus::Connection,
    bus_name: String,
    tx: mpsc::Sender<RecordingSession>,
) {
    update_connection_status(&connection, ConnectionStatus::Connected).await;
    let player_proxy = match PlayerProxy::builder(&connection)
        .destination(bus_name)
        .unwrap()
        .build()
        .await
    {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to create PlayerProxy: {}", e);
            return;
        }
    };

    let mut metadata_stream = player_proxy.receive_metadata_changed().await;
    let mut playback_stream = player_proxy.receive_playback_status_changed().await;

    let destination = player_proxy.inner().destination();
    let dest_str = destination.as_str();
    info!("Monitoring updates for {}...", dest_str);

    let mut session: Option<RecordingSession> = None;
    let mut current_status = player_proxy.playback_status().await.unwrap_or_else(|_| "Unknown".to_string());

    // Initial state
    if let Ok(metadata) = player_proxy.metadata().await {
        handle_metadata_update(metadata, &connection, &tx, &mut session, &current_status).await;
    }

    loop {
        tokio::select! {
            Some(_) = metadata_stream.next() => {
                if let Ok(metadata) = player_proxy.metadata().await {
                    handle_metadata_update(metadata, &connection, &tx, &mut session, &current_status).await;
                }
            }
            Some(_) = playback_stream.next() => {
                if let Ok(status) = player_proxy.playback_status().await {
                    current_status = status.clone();
                    handle_playback_status_update(status, &connection, &tx, &mut session).await;
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(500)) => {
                // Periodic check for recording_enabled changes to handle immediate stop/discard
                if let Ok(iface_ref) = connection
                    .object_server()
                    .interface::<_, ServiceControl>("/org/spotify_recorder/Control")
                    .await
                {
                    let guard = iface_ref.get().await;
                    if !guard.recording_enabled && session.is_some() {
                        drop(guard);
                        discard_recording(&mut session).await;
                    }
                }
            }
            else => break,
        }
    }

    // Cleanup session if monitor stops
    if let Some(s) = session.take() {
        let _ = tx.send(s).await;
    }
}

async fn handle_playback_status_update(
    status: String,
    connection: &zbus::Connection,
    tx: &mpsc::Sender<RecordingSession>,
    session: &mut Option<RecordingSession>,
) {
    info!("Playback status changed: {}", status);

    if let Ok(iface_ref) = connection
        .object_server()
        .interface::<_, ServiceControl>("/org/spotify_recorder/Control")
        .await
    {
        let guard = iface_ref.get().await;
        let recording_enabled = guard.recording_enabled;
        drop(guard);

        if !recording_enabled && session.is_some() {
            discard_recording(session).await;
            return;
        }

        if status == "Playing" {
            start_recording_if_needed(connection, session, &status).await;
        } else {
            stop_recording(tx, session).await;
        }
    }
}

async fn get_default_sink_monitor() -> Option<String> {
    // 1. Get default sink name
    let output = Command::new("pactl").arg("get-default-sink").output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let default_sink = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let monitor_name = format!("{}.monitor", default_sink);

    // 2. Get sources list and find the monitor
    let output = Command::new("pactl").arg("list").arg("short").arg("sources").output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let sources = String::from_utf8_lossy(&output.stdout);
    for line in sources.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[1] == monitor_name {
            return Some(parts[0].to_string());
        }
    }

    None
}

async fn start_recording_if_needed(
    connection: &zbus::Connection,
    session: &mut Option<RecordingSession>,
    playback_status: &str,
) {
    if session.is_some() || playback_status != "Playing" {
        return;
    }

    if let Ok(iface_ref) = connection
        .object_server()
        .interface::<_, ServiceControl>("/org/spotify_recorder/Control")
        .await
    {
        let guard = iface_ref.get().await;
        if guard.recording_enabled && !guard.waiting_for_next_track {
            if let Some(track) = &guard.current_track {
                let target_node = get_default_sink_monitor().await;
                
                let temp_file = match tempfile::NamedTempFile::new() {
                    Ok(f) => f,
                    Err(e) => {
                        error!("Failed to create temp file: {}", e);
                        return;
                    }
                };
                let temp_path = match temp_file.into_temp_path().keep() {
                    Ok(p) => p,
                    Err(e) => {
                        error!("Failed to persist temp file: {}", e);
                        return;
                    }
                };

                info!("Starting recording to {:?}", temp_path);
                let mut cmd = Command::new("pw-record");
                if let Some(node) = target_node {
                    info!("Recording from node: {}", node);
                    cmd.arg("--target").arg(node);
                } else {
                    warn!("Could not detect default sink monitor, falling back to default input");
                }
                
                match cmd.arg(&temp_path).spawn() {
                    Ok(child) => {
                        *session = Some(RecordingSession {
                            child,
                            temp_path,
                            track: track.clone(),
                        });
                    }
                    Err(e) => {
                        error!("Failed to start pw-record: {}", e);
                    }
                }
            }
        }
    }
}

async fn discard_recording(session: &mut Option<RecordingSession>) {
    if let Some(mut s) = session.take() {
        info!("Discarding active recording for {}", s.track.title.as_deref().unwrap_or("Unknown"));
        let _ = s.child.kill().await;
        let _ = tokio::fs::remove_file(&s.temp_path).await;
    }
}

async fn stop_recording(
    tx: &mpsc::Sender<RecordingSession>,
    session: &mut Option<RecordingSession>,
) {
    if let Some(mut s) = session.take() {
        info!("Stopping recording for {}", s.track.title.as_deref().unwrap_or("Unknown"));
        let _ = s.child.kill().await;
        let _ = tx.send(s).await;
    }
}

fn parse_track_info(metadata: &HashMap<String, OwnedValue>) -> TrackInfo {
    let mut track = TrackInfo::default();

    if let Some(v) = metadata.get("mpris:trackid") {
        if let Ok(s) = v.downcast_ref::<&str>() {
            track.track_id = s.to_string();
        }
    }

    track.title = metadata
        .get("xesam:title")
        .and_then(|v| v.downcast_ref::<&str>().ok().map(|s| s.to_string()));

    track.album = metadata
        .get("xesam:album")
        .and_then(|v| v.downcast_ref::<&str>().ok().map(|s| s.to_string()));

    track.art_url = metadata
        .get("mpris:artUrl")
        .and_then(|v| v.downcast_ref::<&str>().ok().map(|s| s.to_string()));

    track.track_number = metadata
        .get("xesam:trackNumber")
        .and_then(|v| v.downcast_ref::<i32>().ok());

    track.disc_number = metadata
        .get("xesam:discNumber")
        .and_then(|v| v.downcast_ref::<i32>().ok());

    track.artist = metadata.get("xesam:artist").and_then(|v| {
        let a: Result<&zbus::zvariant::Array, _> = v.downcast_ref();
        a.ok().map(|array| {
            array
                .iter()
                .filter_map(|val| {
                    let s: Result<&str, _> = val.try_into();
                    s.ok().map(|s| s.to_string())
                })
                .collect::<Vec<String>>()
                .join(", ")
        })
    });

    track.album_artist = metadata.get("xesam:albumArtist").and_then(|v| {
        let a: Result<&zbus::zvariant::Array, _> = v.downcast_ref();
        a.ok().map(|array| {
            array
                .iter()
                .filter_map(|val| {
                    let s: Result<&str, _> = val.try_into();
                    s.ok().map(|s| s.to_string())
                })
                .collect::<Vec<String>>()
                .join(", ")
        })
    });

    track
}

async fn handle_metadata_update(
    metadata: HashMap<String, OwnedValue>,
    connection: &zbus::Connection,
    tx: &mpsc::Sender<RecordingSession>,
    session: &mut Option<RecordingSession>,
    playback_status: &str,
) {
    let track = parse_track_info(&metadata);

    if let Ok(iface_ref) = connection
        .object_server()
        .interface::<_, ServiceControl>("/org/spotify_recorder/Control")
        .await
    {
        let mut guard = iface_ref.get_mut().await;
        let recording_enabled = guard.recording_enabled;

        // Check if it's the same track
        if let Some(current) = &guard.current_track {
            if current.track_id == track.track_id {
                return;
            }

            // Track changed, handle previous session
            if session.is_some() {
                if !recording_enabled {
                    discard_recording(session).await;
                } else {
                    stop_recording(tx, session).await;
                }
            }
        }

        info!("New track detected: {:?} - {:?}", track.artist, track.title);
        guard.current_track = Some(track);

        if recording_enabled && guard.waiting_for_next_track {
            info!("First track after enabling recording mode detected, starting with next track.");
            guard.waiting_for_next_track = false;
        }

        // Emit PropertyChanged for CurrentSong
        guard
            .current_song_changed(iface_ref.signal_emitter())
            .await
            .unwrap_or_default();

        // Drop guard before calling start_recording_if_needed to avoid deadlock
        drop(guard);
        start_recording_if_needed(connection, session, playback_status).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn test_parse_track_info() {
        let mut metadata = HashMap::new();
        metadata.insert(
            "mpris:trackid".to_string(),
            Value::from("track1").try_to_owned().unwrap(),
        );
        metadata.insert(
            "xesam:title".to_string(),
            Value::from("Title").try_to_owned().unwrap(),
        );
        metadata.insert(
            "xesam:album".to_string(),
            Value::from("Album").try_to_owned().unwrap(),
        );
        metadata.insert(
            "xesam:artist".to_string(),
            Value::from(vec!["Artist 1", "Artist 2"]).try_to_owned().unwrap(),
        );
        metadata.insert(
            "xesam:albumArtist".to_string(),
            Value::from(vec!["Album Artist 1"]).try_to_owned().unwrap(),
        );
        metadata.insert(
            "xesam:trackNumber".to_string(),
            Value::from(5i32).try_to_owned().unwrap(),
        );
        metadata.insert(
            "xesam:discNumber".to_string(),
            Value::from(1i32).try_to_owned().unwrap(),
        );
        metadata.insert(
            "mpris:artUrl".to_string(),
            Value::from("https://example.com/art.jpg").try_to_owned().unwrap(),
        );

        let track = parse_track_info(&metadata);

        assert_eq!(track.track_id, "track1");
        assert_eq!(track.title, Some("Title".to_string()));
        assert_eq!(track.album, Some("Album".to_string()));
        assert_eq!(track.artist, Some("Artist 1, Artist 2".to_string()));
        assert_eq!(track.album_artist, Some("Album Artist 1".to_string()));
        assert_eq!(track.track_number, Some(5));
        assert_eq!(track.disc_number, Some(1));
        assert_eq!(track.art_url, Some("https://example.com/art.jpg".to_string()));
    }
}
