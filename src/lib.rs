pub mod mpris;

use crate::mpris::PlayerProxy;
use futures_util::StreamExt;
use std::collections::HashMap;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use zbus::{fdo::DBusProxy, interface, names::BusName, zvariant::OwnedValue};

#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    pub track_id: String,
    pub title: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub art_url: Option<String>,
    pub track_number: Option<i32>,
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

impl From<ConnectionStatus> for zbus::zvariant::Value<'_> {
    fn from(status: ConnectionStatus) -> Self {
        match status {
            ConnectionStatus::Disconnected => "Disconnected".into(),
            ConnectionStatus::Connected => "Connected".into(),
        }
    }
}

pub struct ServiceControl {
    pub recording_enabled: bool,
    pub connection_status: ConnectionStatus,
    pub current_track: Option<TrackInfo>,
}

impl ServiceControl {
    pub fn new() -> Self {
        Self {
            recording_enabled: false,
            connection_status: ConnectionStatus::Disconnected,
            current_track: None,
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
    fn set_recording_enabled(&mut self, enabled: bool) {
        if self.recording_enabled != enabled {
            self.recording_enabled = enabled;
            info!(
                "Recording mode: {}",
                if enabled { "Enabled" } else { "Disabled" }
            );
        }
    }

    #[zbus(property)]
    fn connection_status(&self) -> ConnectionStatus {
        self.connection_status
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
    let control = ServiceControl::new();

    connection
        .object_server()
        .at("/org/spotify_recorder/Control", control)
        .await?;

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

pub async fn monitor_spotify(connection: zbus::Connection, bus_name: String) {
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

    let destination = player_proxy.inner().destination();
    let dest_str = destination.as_str();
    info!("Monitoring metadata updates for {}...", dest_str);

    // Initial metadata
    if let Ok(metadata) = player_proxy.metadata().await {
        handle_metadata_update(metadata, &connection).await;
    }

    while let Some(_) = metadata_stream.next().await {
        if let Ok(metadata) = player_proxy.metadata().await {
            handle_metadata_update(metadata, &connection).await;
        }
    }
}

async fn handle_metadata_update(
    metadata: HashMap<String, OwnedValue>,
    connection: &zbus::Connection,
) {
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

    if let Ok(iface_ref) = connection
        .object_server()
        .interface::<_, ServiceControl>("/org/spotify_recorder/Control")
        .await
    {
        let mut guard = iface_ref.get_mut().await;

        // Check if it's the same track
        if let Some(current) = &guard.current_track {
            if current.track_id == track.track_id {
                return;
            }

            if guard.recording_enabled {
                // Export previous
                let artist = current.artist.as_deref().unwrap_or("Unknown Artist");
                let title = current.title.as_deref().unwrap_or("Unknown Title");
                println!("exporting file: /tmp/recordings/{}/{}.wav", artist, title);
            }
        }

        info!("New track detected: {:?} - {:?}", track.artist, track.title);
        guard.current_track = Some(track);

        if guard.recording_enabled {
            println!("now starting a new recording");
        }

        // Emit PropertyChanged for CurrentSong
        guard
            .current_song_changed(iface_ref.signal_emitter())
            .await
            .unwrap_or_default();
    }
}
