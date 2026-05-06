use std::collections::HashMap;
use tokio::sync::mpsc;
use zbus::{connection::Builder, interface, zvariant::OwnedValue};

pub struct SpotifyMock {
    metadata: HashMap<String, OwnedValue>,
}

impl SpotifyMock {
    pub fn new() -> Self {
        Self {
            metadata: HashMap::new(),
        }
    }

    pub fn set_metadata(&mut self, metadata: HashMap<String, OwnedValue>) {
        self.metadata = metadata;
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
        "Playing"
    }
}

pub async fn run_mock(
    mut receiver: mpsc::Receiver<HashMap<String, OwnedValue>>,
    bus_name: &str,
) -> zbus::Result<()> {
    let mock = SpotifyMock::new();
    let connection = Builder::session()?
        .name(bus_name)?
        .serve_at("/org/mpris/MediaPlayer2", mock)?
        .build()
        .await?;

    let object_server = connection.object_server();

    while let Some(new_metadata) = receiver.recv().await {
        let interface = object_server
            .interface::<_, SpotifyMock>("/org/mpris/MediaPlayer2")
            .await?;
        interface.get_mut().await.set_metadata(new_metadata);
        let emitter = interface.signal_emitter();
        interface.get_mut().await.metadata_changed(emitter).await?;
    }

    Ok(())
}
