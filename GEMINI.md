# Project Overview

`taped` is a system service for Linux that allows to record and export tracks played on Spotify.
In order to know when a new track begins, the service monitors metadata updates coming from Spotify via DBus.
The provided metadata (incl. album covers) is added to the exported files in the form of ID3 tags.
The files are exported to a given destination directory with a subfolder tree for artist / album.

- Language: Rust, C (helper binaries)
- Audio system: PipeWire (native, not PulseAudio fallback unless stated)
- Runtime: Linux desktop (user session)
- Dev environment: container connecting to host PipeWire and DBus via XDG_RUNTIME_DIR

# Software Requirements

- **Robustness**: Handle arbitrary startup order (service vs. Spotify) and Spotify restarts mid-session.
- **Concurrency**: Use `tokio` for async event handling and task orchestration.
- **Recording Logic**: 
    - When recording mode is enabled, the service must wait for the *next* metadata update (track change) before starting to capture audio. This ensures only full tracks are recorded.
    - When recording mode is disabled, any active recording must stop immediately and be discarded (not exported).
- **Audio Routing**: Use PipeWire CLI tools (`pactl`, `wpctl`, `pw-loopback`, `pw-dump`) to manage virtual sinks and routing. Avoid unmaintained PipeWire crates.
- **Background Export**: Exporting (tagging, moving files) must happen in a dedicated background thread/task to avoid blocking the recorder.
- **Metadata**: Add ID3 tags (including album art via `mpris:artUrl`) to exported files.
- **Security**: Must run without root privileges.
- **Observability**: Provide detailed logging.
- **Extensibility**: Design with a future DBus control interface in mind (start/stop, config).

# Architecture & Design

## State Machine

The service will be modeled as a state machine to handle the dynamic lifecycle of Spotify and PipeWire nodes:

1. **`WaitingForSpotify`**: Service is up, but Spotify's DBus interface or PipeWire node is missing.
2. **`SpotifyReady`**: Spotify is detected on DBus and PipeWire. Virtual sink is created, and audio is routed.
3. **`Recording`**: Actively capturing audio to a temporary file. Transitioned to on `PropertiesChanged` (Metadata) or `PlaybackStatus` (Playing).
4. **`Idle`**: Spotify is present but playback is paused/stopped.

## Component Overview

- **DBus Monitor**: Uses `zbus` to listen for `org.mpris.MediaPlayer2.Player` updates.
- **PipeWire Manager**: Orchestrates CLI tools for sink creation and stream routing.
- **Recorder**: Captures audio from the virtual sink monitor.
- **Exporter**: Background worker that processes completed recordings, downloads album art, and applies ID3 tags.

# Testing Strategy

- **Mocking**: Implement fakes/mocks for:
    - DBus events (simulating Spotify appearing/disappearing and metadata changes).
    - PipeWire state (simulating node discovery and CLI tool outputs).
- **Integration Tests**: Self-contained tests that verify the state machine transitions and recording lifecycle without requiring a running Spotify instance or real PipeWire setup.
- **Unit Tests**: Focus on metadata parsing, path generation, and ID3 tagging logic.

# Development Workflow

After every change in the codebase, the following steps must be performed to ensure quality and consistency:

1.  **Formatting**: Run `cargo fmt` to ensure the code adheres to the standard Rust style.
2.  **Compilation**: Ensure the project compiles without warnings or errors using `cargo check` or `cargo build`.
3.  **Testing**: Execute the test suite using `dbus-run-session -- cargo test` to verify that all integrations and units are functioning correctly.
