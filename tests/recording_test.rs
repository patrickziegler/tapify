mod mock;

use mock::{run_mock, MockCommand};
use spotify_recorder::{ServiceControl, monitor_spotify, exporter_task};
use std::collections::HashMap;
use tokio::sync::mpsc;
use zbus::{connection, zvariant::Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[tokio::test]
async fn test_recording_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt::try_init();
    
    // 1. Create a mock pw-record and pactl script
    let temp_dir = tempfile::tempdir()?;
    let mock_pw_record_path = temp_dir.path().join("pw-record");
    fs::write(&mock_pw_record_path, "#!/bin/sh\n# shift until we find the file path (the last arg)\nfor arg; do\n  case $arg in\n    --target) shift ;;\n    [0-9]*) shift ;;\n    *) FILE=$arg ;;\n  esac\ndone\ntouch \"$FILE\"\nwhile true; do sleep 1; done")?;
    let mut perms = fs::metadata(&mock_pw_record_path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&mock_pw_record_path, perms)?;

    let mock_pactl_path = temp_dir.path().join("pactl");
    fs::write(&mock_pactl_path, "#!/bin/sh\ncase $1 in\n  get-default-sink) echo \"default-sink\" ;;\n  list) echo \"1 default-sink.monitor module-null-sink.c s16le 2ch 44100Hz RUNNING\" ;;\nesac")?;
    let mut perms = fs::metadata(&mock_pactl_path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&mock_pactl_path, perms)?;

    // 2. Add temp_dir to PATH
    let old_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", temp_dir.path().to_str().unwrap(), old_path);
    unsafe {
        std::env::set_var("PATH", new_path);
    }

    let spotify_bus_name = "org.mpris.MediaPlayer2.spotify.test_recording";
    let service_bus_name = "org.spotify_recorder.test_rec";
    let (mock_tx, mock_rx) = mpsc::channel(10);

    // Start mock Spotify
    tokio::spawn(async move {
        run_mock(mock_rx, spotify_bus_name).await.unwrap();
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let connection = connection::Builder::session()?
        .name(service_bus_name)?
        .build()
        .await?;
    
    let mut control = ServiceControl::new();
    // Start with recording DISABLED
    control.recording_enabled = false;

    connection
        .object_server()
        .at("/org/spotify_recorder/Control", control)
        .await?;

    let (exporter_tx, exporter_rx) = mpsc::channel(10);

    // Start exporter
    tokio::spawn(exporter_task(exporter_rx));

    // Start monitor
    tokio::spawn(monitor_spotify(
        connection.clone(),
        spotify_bus_name.to_string(),
        exporter_tx,
    ));

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // 1. Send metadata for Song 1
    let mut metadata = HashMap::new();
    metadata.insert("mpris:trackid".to_string(), Value::from("track1").try_to_owned()?);
    metadata.insert("xesam:title".to_string(), Value::from("Song 1").try_to_owned()?);
    mock_tx.send(MockCommand::Metadata(metadata)).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 2. Start playing Song 1
    mock_tx.send(MockCommand::PlaybackStatus("Playing".to_string())).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // 3. Enable recording mode while Song 1 is playing
    let proxy = connection
        .object_server()
        .interface::<_, ServiceControl>("/org/spotify_recorder/Control")
        .await?;
    proxy.get_mut().await.set_recording_enabled(proxy.signal_emitter().clone(), true).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // 4. Change track to Song 2 (should trigger waiting_for_next_track = false, AND start recording Song 2)
    let mut metadata2 = HashMap::new();
    metadata2.insert("mpris:trackid".to_string(), Value::from("track2").try_to_owned()?);
    metadata2.insert("xesam:title".to_string(), Value::from("Song 2").try_to_owned()?);
    mock_tx.send(MockCommand::Metadata(metadata2)).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    // Song 1 should NOT be exported
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let music_dir = PathBuf::from(home).join("Music").join("Spotify");
    let song1_path = music_dir.join("Unknown Artist").join("Unknown Album").join("00 - Song 1.wav");
    assert!(!song1_path.exists(), "Song 1 should NOT be exported because it was playing when recording was enabled");

    // 5. Disable recording mode while Song 2 is recording
    proxy.get_mut().await.set_recording_enabled(proxy.signal_emitter().clone(), false).await;
    
    tokio::time::sleep(tokio::time::Duration::from_millis(600)).await; // wait for periodic check

    // 6. Song 2 should NOT be exported
    let song2_path = music_dir.join("Unknown Artist").join("Unknown Album").join("00 - Song 2.wav");
    assert!(!song2_path.exists(), "Song 2 should NOT be exported because recording was disabled during its capture");

    // 7. Re-enable recording
    proxy.get_mut().await.set_recording_enabled(proxy.signal_emitter().clone(), true).await;
    
    // 8. Change track to Song 3 (should set waiting_for_next_track = false)
    let mut metadata3 = HashMap::new();
    metadata3.insert("mpris:trackid".to_string(), Value::from("track3").try_to_owned()?);
    metadata3.insert("xesam:title".to_string(), Value::from("Song 3").try_to_owned()?);
    mock_tx.send(MockCommand::Metadata(metadata3)).await?;
    
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // 9. Change track to Song 4 (should trigger Song 3 export)
    let mut metadata4 = HashMap::new();
    metadata4.insert("mpris:trackid".to_string(), Value::from("track4").try_to_owned()?);
    metadata4.insert("xesam:title".to_string(), Value::from("Song 4").try_to_owned()?);
    mock_tx.send(MockCommand::Metadata(metadata4)).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // 10. Change track to Song 5 (should trigger Song 4 export)
    let mut metadata5 = HashMap::new();
    metadata5.insert("mpris:trackid".to_string(), Value::from("track5").try_to_owned()?);
    metadata5.insert("xesam:title".to_string(), Value::from("Song 5").try_to_owned()?);
    mock_tx.send(MockCommand::Metadata(metadata5)).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    // 11. Check if Song 3 was exported
    let song3_path = music_dir.join("Unknown Artist").join("Unknown Album").join("00 - Song 3.wav");
    assert!(song3_path.exists(), "Exported Song 3 should exist at {:?}", song3_path);

    Ok(())
}
