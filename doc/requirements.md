
- The service shall not require root privileges
- The service shall run as a systemd service (user session) in the background
- The service shall support a TOML config file in a well known location like ~/.config/
- The service shall provide a CLI option to define a custom location for the config file
- The service configuration shall hold values such as the destination directory and the naming pattern for exported files
- The service shall provide DBus interfaces for commands like start and stop recording
- The service shall provide status updates via DBus (like Spotify connection status, currently recording song name (if any) etc.)

- The service may use `pactl` for managing virtual sinks
- The service may use `wpctl` for managing connections between audio sources and sinks (routing)
- The service may use `pw-loopback` for routing audio to the default sink while recording if background recording mode wasn't configured
- The service may use `pw-record` for recording audio

- During startup, the service shall create a virtual sink as a pipewire node that can be used for recording audio streams later on
- During shutdown, the service shall clean up all created virtual sinks and restore default sinks for the sources it has manipulated

- The service shall subscribe to DBus events signalling Spotify appearing or disappearing on the session bus
- After Spotify appeared on DBus, the service shall check the presence of the respective PipeWire node
- After Spotify appeared on DBus, but no respective PipeWire node was found, the service shall subscribe to node graph changes and wait for Spotify appearing in PipeWire
- As soon as the Spotify node is available in the PipeWire node graph, the service shall reroute the Spotify audio stream to the virtual sink intended for recording audio

- The service shall support a recording mode, which shall only be entered if all preconditions are met
- For entering recording mode, the user needs to have requested it by using the Start interface on DBus (the Stop interface would revoke this request)
- For entering recording mode, Spotify needs to be available on DBus
- For entering recording mode, Spotify needs to be available in the PipeWire node graph
- For entering recording mode, the virtual sink needs to be present
- For entering recording mode, the Spotify audio stream needs to be rerouted to the virtual sink

- In recording mode, the service shall wait for a metadata update from spotify
- If a metadata update has been recieved, the service shall start to record the audio on the virtual sink and keep the metadata stored for exporting later
- If another metadata update has been recieved during recording, the currently ongoing recording shall be stopped and a new recording shall be started
- A stopped recording shall be exported as MP3 to the configured destination directory
- The metadata recieved at the beginning of a recording shall be written to the exported MP3 file
- If the metadata contains a link to an album cover, it shall be downloaded to a temporary location and added to the ID3 metadata of the exported file as well


- I don't want to make any assumption of the order of startup, like if the spotify-recorder service or Spotify itself was started first, and also handle the case that Spotify was closed and reopened in the middle of a session.
- I think it might be good to subscribe to DBus and pipewire events to keep track of the readiness of Spotify. It might be a good idea to model the service as a state machine.
- The service needs to create a virtual sink and reroute the Spotify audio to it for recording.
- A new recording should start on every metadata update and the previous recording (if any) should be exported while the next recording already starts because exporting the files may take several seconds. Because of this, we may need a dedicated thread for exporting.
- Even while recording (and maybe even exporting) the service should remain responsive for metadata updates and other events.
- The exported files should have ID3 tags containing the metadata that was sent from Spotify at the beginning of the track.
- The service should not require root privileges.
- The service may make direct use of CLI tools like pactl, wpctl, pw-loopback, pw-dump etc., if possible, we don't want to use a pipewire crate (as it seems unmaintained at the moment)
- The service shall use zbus for dbus integration and tokio for eventgueue, threads etc.
- The service should provide good logging output.
- Not mandatory right now (but may be important to have in mind for later on), it would be cool if the service provided a DBus interface itself, over which recording can be started and stopped, and maybe even the destination directory could be configured.
- Also not mandatory, but it would be even cooler if that DBus intercae would even support status updates indicating the connection status to Spotify and the song name of the currently recording song etc.

- It is EXTREMELY important to rely heavily on testing right from the start. We should even start with implementing mocks / fakes for the important event sources for this service so that we can have a self contained and quick test suite for future feature development.


- The service must subscribe to metadata updates sent by Spotify via DBus

- The service must monitor the availablity of Spotify on pipewire
- If Spotify not available as pipewire node at startup, wait for it to come up (in async fashion)
- If Spotify becomes available as pipewire node (or was already available at startup time), create a virtual sink for recording and reroute to Spotify audio to that sink
- If the user wants to hear Spotify output while recording, also route the Spotify audio stream to the default sink
- If Spotify becomes unavailable (which may happen any time), stop the ongoing recording (if any), don't export the currently recorded track and wait for it to come up again
- If Spotify comes back after being unavailable, reroute its audio stream to the virtual sink created before

- On service shutdown, stop the ongoing recording (if any) but don't export the file, then perform a proper clean up (virtual sink etc.)


- After recording mode is started, the service starts to record right away, but we want to start recording after the next metadata update only in order to ensure that we record full tracks only!
- When recording mode is stopped, the recording continues until the next metadata update. But we want the recording to stop immediately and we don't want this half-finished recording to be exported, just drop it.

- The application must connect to PipeWire via the host socket
- It must list and monitor audio nodes
- It must react to node changes in real time
- It must run correctly inside a containerized environment


- Must not require root privileges
- Must be implemented in Rust, using zbus for DBus integration and Tokio for multithreading

- Must work without systemd inside the container
- Must not require root privileges
- Must handle PipeWire restarts gracefully
- Low-latency interaction with PipeWire
- Minimal CPU overhead
- No hard dependency on PulseAudio compatibility layer

See: `docs/requirements.md` for full specification, 
