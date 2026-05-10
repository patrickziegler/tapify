use std::collections::HashMap;
use tokio::sync::mpsc;
use zbus::{connection::Builder, interface, zvariant::OwnedValue};

pub enum MockCommand {
    Metadata(HashMap<String, OwnedValue>),
    #[allow(dead_code)]
    PlaybackStatus(String),
}

pub struct SpotifyMock {
    metadata: HashMap<String, OwnedValue>,
    playback_status: String,
}

impl SpotifyMock {
    pub fn new() -> Self {
        Self {
            metadata: HashMap::new(),
            playback_status: "Paused".to_string(),
        }
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl SpotifyMock {
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.metadata.clone()
    }

    #[zbus(property)]
    fn playback_status(&self) -> &str {
        &self.playback_status
    }
}

pub async fn run_mock(
    mut receiver: mpsc::Receiver<MockCommand>,
    bus_name: &str,
) -> zbus::Result<()> {
    let mock = SpotifyMock::new();
    let connection = Builder::session()?
        .name(bus_name)?
        .serve_at("/org/mpris/MediaPlayer2", mock)?
        .build()
        .await?;

    let object_server = connection.object_server();

    while let Some(cmd) = receiver.recv().await {
        let interface = object_server
            .interface::<_, SpotifyMock>("/org/mpris/MediaPlayer2")
            .await?;
        let emitter = interface.signal_emitter();
        match cmd {
            MockCommand::Metadata(new_metadata) => {
                interface.get_mut().await.metadata = new_metadata;
                interface.get_mut().await.metadata_changed(emitter).await?;
            }
            MockCommand::PlaybackStatus(new_status) => {
                interface.get_mut().await.playback_status = new_status;
                interface.get_mut().await.playback_status_changed(emitter).await?;
            }
        }
    }

    Ok(())
}
