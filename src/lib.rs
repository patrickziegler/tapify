pub mod mpris;

use crate::mpris::PlayerProxy;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use zbus::{fdo::DBusProxy, names::BusName, zvariant::OwnedValue};

#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    pub track_id: String,
    pub title: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub art_url: Option<String>,
    pub track_number: Option<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct ServiceState {
    pub current_track: Option<TrackInfo>,
}

impl ServiceState {
    pub fn new() -> Self {
        Self {
            current_track: None,
        }
    }
}

pub async fn run_service(
    connection: zbus::Connection,
    spotify_bus_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let dbus_proxy = DBusProxy::new(&connection).await?;
    let mut name_owner_changed = dbus_proxy.receive_name_owner_changed().await?;
    let state = Arc::new(Mutex::new(ServiceState::new()));

    let mut monitor_handle: Option<JoinHandle<()>> = None;

    // Initial check
    let spotify_bus_name_owned = BusName::try_from(spotify_bus_name)?;
    if let Ok(owner) = dbus_proxy.get_name_owner(spotify_bus_name_owned).await {
        info!("Spotify found: {}", owner);
        let state_clone = state.clone();
        monitor_handle = Some(tokio::spawn(monitor_spotify(
            connection.clone(),
            spotify_bus_name.to_string(),
            state_clone,
        )));
    }

    while let Some(signal) = name_owner_changed.next().await {
        let args = signal.args()?;
        if args.name() == spotify_bus_name {
            if let Some(_new_owner) = args.new_owner().as_ref() {
                info!("Spotify appeared");
                // Cancel previous monitor if any (though it should have errored out)
                if let Some(handle) = monitor_handle.take() {
                    handle.abort();
                }
                let state_clone = state.clone();
                monitor_handle = Some(tokio::spawn(monitor_spotify(
                    connection.clone(),
                    spotify_bus_name.to_string(),
                    state_clone,
                )));
            } else {
                warn!("Spotify disappeared");
                if let Some(handle) = monitor_handle.take() {
                    handle.abort();
                }
            }
        }
    }

    Ok(())
}

pub async fn monitor_spotify(
    connection: zbus::Connection,
    bus_name: String,
    state: Arc<Mutex<ServiceState>>,
) {
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

    let destination = player_proxy.inner().destination();
    let dest_str = destination.as_str();
    info!("Monitoring metadata updates for {}...", dest_str);

    // Initial metadata
    if let Ok(metadata) = player_proxy.metadata().await {
        handle_metadata_update(metadata, &state);
    }

    while let Some(_) = metadata_stream.next().await {
        if let Ok(metadata) = player_proxy.metadata().await {
            handle_metadata_update(metadata, &state);
        }
    }
}

fn handle_metadata_update(metadata: HashMap<String, OwnedValue>, state: &Arc<Mutex<ServiceState>>) {
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

    let mut state_guard = state.lock().unwrap();

    // Check if it's the same track (Spotify sometimes sends multiple metadata updates for same track)
    if let Some(current) = &state_guard.current_track {
        if current.track_id == track.track_id {
            return;
        }

        // Export previous
        let artist = current.artist.as_deref().unwrap_or("Unknown Artist");
        let title = current.title.as_deref().unwrap_or("Unknown Title");
        println!("exporting file: /tmp/recordings/{}/{}.wav", artist, title);
    }

    info!("New track detected: {:?} - {:?}", track.artist, track.title);
    state_guard.current_track = Some(track);
    println!("now starting a new recording");
}
