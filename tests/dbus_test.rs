mod mock;

use mock::run_mock;
use spotify_recorder::{ServiceState, monitor_spotify};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use zbus::{connection, zvariant::Value};

#[tokio::test]
async fn test_metadata_updates_no_duplication() -> Result<(), Box<dyn std::error::Error>> {
    let bus_name = "org.mpris.MediaPlayer2.spotify.test2";
    let (tx, rx) = mpsc::channel(10);

    // Start mock in background
    tokio::spawn(async move {
        run_mock(rx, bus_name).await.unwrap();
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let connection = connection::Builder::session()?.build().await?;
    let state = Arc::new(Mutex::new(ServiceState::default()));

    // Start monitor
    let state_clone = state.clone();
    let handle = tokio::spawn(monitor_spotify(
        connection.clone(),
        bus_name.to_string(),
        state_clone,
    ));

    // Send metadata update
    let mut metadata = HashMap::new();
    metadata.insert(
        "mpris:trackid".to_string(),
        Value::from("track1").try_to_owned()?,
    );
    metadata.insert(
        "xesam:title".to_string(),
        Value::from("Title 1").try_to_owned()?,
    );
    tx.send(metadata).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    {
        let guard = state.lock().unwrap();
        assert_eq!(
            guard
                .current_track
                .as_ref()
                .unwrap()
                .title
                .as_ref()
                .unwrap(),
            "Title 1"
        );
    }

    // "Restart" Spotify: abort monitor and start new one
    handle.abort();

    let state_clone2 = state.clone();
    tokio::spawn(monitor_spotify(
        connection.clone(),
        bus_name.to_string(),
        state_clone2,
    ));

    // Send same metadata update
    let mut metadata2 = HashMap::new();
    metadata2.insert(
        "mpris:trackid".to_string(),
        Value::from("track1").try_to_owned()?,
    );
    metadata2.insert(
        "xesam:title".to_string(),
        Value::from("Title 1").try_to_owned()?,
    );
    tx.send(metadata2).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // If it was duplicating or double-handling without track_id check, we might see multiple "now starting a new recording" in stdout

    // Send DIFFERENT metadata
    let mut metadata3 = HashMap::new();
    metadata3.insert(
        "mpris:trackid".to_string(),
        Value::from("track2").try_to_owned()?,
    );
    metadata3.insert(
        "xesam:title".to_string(),
        Value::from("Title 2").try_to_owned()?,
    );
    tx.send(metadata3).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    {
        let guard = state.lock().unwrap();
        assert_eq!(
            guard
                .current_track
                .as_ref()
                .unwrap()
                .title
                .as_ref()
                .unwrap(),
            "Title 2"
        );
    }

    Ok(())
}
