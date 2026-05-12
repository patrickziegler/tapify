use clap::Parser;
use std::path::PathBuf;
use zbus::connection;

pub use taped::watchdog::run_service;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Destination directory for recorded tracks
    destination: Option<PathBuf>,

    /// Pattern for constructing filenames
    #[arg(long, default_value = "{albumArtist} - {album}/{trackNumber} - {title}")]
    pattern: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let music_dir = args.destination.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        PathBuf::from(home).join("Music").join("Spotify")
    });

    let connection = connection::Builder::session()?
        .build()
        .await?;

    run_service(
        connection,
        "org.mpris.MediaPlayer2.spotify",
        music_dir,
        args.pattern,
    )
    .await
}
