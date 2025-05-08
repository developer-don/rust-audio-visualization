use rodio::{Decoder, OutputStream, Sink};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Idle,   // No file loaded or playback finished
    Loaded, // File loaded, ready to play (e.g., after finishing, or explicitly loaded without play)
    Playing,
    Paused,
}

pub struct AudioManager {
    _stream: OutputStream,                    // Keep stream alive for audio output
    stream_handle: rodio::OutputStreamHandle, // To create new sinks if needed
    sink: Sink,
    current_file_path: Option<String>,
    state: PlaybackState,
}

impl AudioManager {
    pub fn new() -> Result<Self, String> {
        let (stream, stream_handle) = OutputStream::try_default()
            .map_err(|e| format!("Failed to open output stream: {}", e))?;
        let sink =
            Sink::try_new(&stream_handle).map_err(|e| format!("Failed to create sink: {}", e))?;

        Ok(AudioManager {
            _stream: stream,
            stream_handle,
            sink,
            current_file_path: None,
            state: PlaybackState::Idle,
        })
    }

    pub fn load_and_play_file(&mut self, file_path: &str) -> Result<(), String> {
        if file_path.trim().is_empty() {
            return Err("File path cannot be empty.".to_string());
        }

        // Stop any current playback and clear the sink's queue before recreating it.
        // Recreating the sink ensures it is clean.
        self.sink.stop();
        self.sink = Sink::try_new(&self.stream_handle)
            .map_err(|e| format!("Failed to re-create sink: {}", e))?;

        let path = Path::new(file_path);
        let file = File::open(path)
            .map_err(|e| format!("Failed to open file '{}': {}", path.display(), e))?;

        let source = Decoder::new(BufReader::new(file))
            .map_err(|e| format!("Failed to decode file '{}': {}", path.display(), e))?;

        self.sink.append(source);
        self.sink.play();

        self.current_file_path = Some(file_path.to_string());
        self.state = PlaybackState::Playing;
        tracing::info!("Playing file: {}", file_path);

        Ok(())
    }

    pub fn pause_playback(&mut self) {
        if self.state == PlaybackState::Playing && !self.sink.is_paused() {
            self.sink.pause();
            self.state = PlaybackState::Paused;
            tracing::info!("Playback paused.");
        }
    }

    pub fn resume_playback(&mut self) {
        if self.state == PlaybackState::Paused && self.sink.is_paused() {
            self.sink.play();
            self.state = PlaybackState::Playing;
            tracing::info!("Playback resumed.");
        } else if self.state == PlaybackState::Loaded && !self.sink.empty() {
            // Handle playing a file that was:
            // 1) loaded but not auto-played
            // 2) finished playing and is now being played again
            self.sink.play();
            self.state = PlaybackState::Playing;
            tracing::info!("Playback started for loaded file.");
        }
    }

    pub fn get_state(&self) -> PlaybackState {
        self.state
    }

    pub fn get_current_file_path(&self) -> Option<&String> {
        self.current_file_path.as_ref()
    }

    // Called periodically in the UI update loop
    pub fn check_and_update_finished_state(&mut self) {
        if self.state == PlaybackState::Playing && self.sink.empty() {
            self.state = PlaybackState::Loaded; // Song finished, mark as Loaded (ready to play again or load new)
            tracing::info!(
                "Playback finished for: {:?}",
                self.current_file_path.as_deref().unwrap_or("unknown file")
            );
        }
    }
}
