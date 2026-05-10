mod mock;

use mock::run_mock;
use spotify_recorder::{ServiceControl, monitor_spotify};
use std::collections::HashMap;
use tokio::sync::mpsc;
use zbus::{connection, proxy, zvariant::Value};

#[proxy(
    interface = "org.spotify_recorder.Control",
    default_path = "/org/spotify_recorder/Control"
)]
trait Control {
    #[zbus(property)]
    fn recording_enabled(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_recording_enabled(&self, enabled: bool) -> zbus::Result<()>;
    #[zbus(property)]
    fn connection_status(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn current_song(&self) -> zbus::Result<String>;
}

#[tokio::test]
async fn test_service_control() -> Result<(), Box<dyn std::error::Error>> {
    let spotify_bus_name = "org.mpris.MediaPlayer2.spotify.test_control";
    let service_bus_name = "org.spotify_recorder.test";
    let (tx, rx) = mpsc::channel(10);

    // Start mock
    tokio::spawn(async move {
        run_mock(rx, spotify_bus_name).await.unwrap();
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let connection = connection::Builder::session()?
        .name(service_bus_name)?
        .build()
        .await?;
    let control = ServiceControl::new();

    // Register our control interface
    connection
        .object_server()
        .at("/org/spotify_recorder/Control", control)
        .await?;

    // Use proxy to talk to our own interface
    let control_proxy = ControlProxy::builder(&connection)
        .destination(service_bus_name)?
        .build()
        .await?;

    let (session_tx, _session_rx) = mpsc::channel(10);

    // Start monitor manually for test
    tokio::spawn(monitor_spotify(
        connection.clone(),
        spotify_bus_name.to_string(),
        session_tx,
    ));

    // Enable recording via DBus
    control_proxy.set_recording_enabled(true).await?;
    assert!(control_proxy.recording_enabled().await?);

    // Wait for monitor to detect Spotify and update status
    let mut status = "Disconnected".to_string();
    for _ in 0..10 {
        status = control_proxy.connection_status().await?;
        if status == "Connected" {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    assert_eq!(status, "Connected");

    // Send metadata
    let mut metadata = HashMap::new();
    metadata.insert(
        "mpris:trackid".to_string(),
        Value::from("track_ctrl_1").try_to_owned()?,
    );
    metadata.insert(
        "xesam:title".to_string(),
        Value::from("Control Title").try_to_owned()?,
    );
    metadata.insert(
        "xesam:artist".to_string(),
        Value::from(vec!["Control Artist"]).try_to_owned()?,
    );
    tx.send(mock::MockCommand::Metadata(metadata)).await?;

    // Wait for song update
    let mut song = "None".to_string();
    for _ in 0..10 {
        song = control_proxy.current_song().await?;
        if song == "Control Artist - Control Title" {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    assert_eq!(song, "Control Artist - Control Title");

    Ok(())
}
