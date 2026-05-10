use spotify_recorder::run_service;
use zbus::connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let control = spotify_recorder::ServiceControl::new();
    let connection = connection::Builder::session()?
        .name("org.spotify_recorder")?
        .serve_at("/org/spotify_recorder/Control", control)?
        .build()
        .await?;
    run_service(connection, "org.mpris.MediaPlayer2.spotify").await
}

