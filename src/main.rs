use spotify_recorder::run_service;
use zbus::connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let connection = connection::Builder::session()?
        .name("org.spotify_recorder")?
        .build()
        .await?;
    run_service(connection, "org.mpris.MediaPlayer2.spotify").await
}

