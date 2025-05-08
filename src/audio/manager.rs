use crate::audio::{
    processor::{AudioAnalysisData, AudioProcessor},
    sample_broadcaster::SampleBroadcaster,
};
use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
// TODO: `crossbeam-channel` appears to be preferred for performance over `mpsc` from std...
//   we can look at swapping that out eventually ™
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const DEFAULT_FFT_SIZE: usize = 1024;
const SAMPLES_PER_CHUNK: usize = DEFAULT_FFT_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Idle,
    Loaded,
    Playing,
    Paused,
}

#[allow(dead_code)]
pub struct AudioManager {
    // TODO: If this is only here to keep the stream alive maybe move it out somewhere.
    //   Not used by anything in the impl logic blocks.
    stream: OutputStream,
    stream_handle: rodio::OutputStreamHandle,
    sink: Option<Sink>,
    processing_thread_handle: Option<thread::JoinHandle<()>>,
    stop_signal_sender: Option<mpsc::Sender<()>>,
    current_file_path: Option<String>,
    state: PlaybackState,
    current_volume: f32,
}

impl AudioManager {
    // Creates a new AudioManager.
    pub fn new(volume: Option<f32>) -> Result<Self, String> {
        let (stream, stream_handle) = OutputStream::try_default()
            .map_err(|e| format!("Failed to open output stream: {}", e))?;

        Ok(AudioManager {
            stream: stream,
            stream_handle,
            sink: None,
            processing_thread_handle: None,
            stop_signal_sender: None,
            current_file_path: None,
            state: PlaybackState::Idle,
            current_volume: volume.unwrap_or(0.0).clamp(0.0, 1.0),
        })
    }

    pub fn set_output_volume(&mut self, volume: f32) {
        // Clamp volume to a reasonable range (e.g., 0.0 to 1.0)
        self.current_volume = volume.clamp(0.0, 1.0);
        tracing::debug!("Setting output volume to: {}", self.current_volume);

        // Apply to the current sink if it exists
        if let Some(sink) = &self.sink {
            sink.set_volume(self.current_volume);
        }
    }

    // Loads and plays the specified MP3 file, begins audio processing.
    // Uses the `analysis_sender` channel for analysis results
    pub fn load_and_play_file(
        &mut self,
        file_path: &str,
        // TODO: `analysis_sender` should really be created/owned by AudioManager itself instead of
        //   requiring external logic deal with it... it's all done in here as it is...
        //   It's currently owned by `AudioVisualizerApp` and passed in here ... moving it would
        //   require rework of `new`
        analysis_sender: mpsc::SyncSender<AudioAnalysisData>,
    ) -> Result<(), String> {
        if file_path.trim().is_empty() {
            return Err("File path cannot be empty.".to_string());
        }

        // Cleanup previous state
        self.stop_playback_and_processing();

        // Load and decode file
        let path = Path::new(file_path);
        let file = File::open(path)
            .map_err(|e| format!("Failed to open file '{}': {}", path.display(), e))?;
        let decoder_raw = Decoder::new(BufReader::new(file))
            .map_err(|e| format!("Failed to decode file '{}': {}", path.display(), e))?;

        // Convert samples to f32 - expected by the SampleBroadcaster
        let decoder_f32 = decoder_raw.convert_samples::<f32>();

        // Store source properties *after* conversion if needed, ensure it's from the f32 source
        let source_sample_rate = decoder_f32.sample_rate();
        let source_channels = decoder_f32.channels();
        self.current_file_path = Some(file_path.to_string());
        tracing::info!(
            "Source properties: Rate={}, Channels={}",
            source_sample_rate,
            source_channels
        );

        // Setup processing thread
        // Use bounded channel for sample data things
        let (sample_chunk_sender, sample_chunk_receiver) = mpsc::sync_channel::<Vec<f32>>(5);
        // Use unbounded channel for simple signals
        let (stop_sender, stop_receiver) = mpsc::channel::<()>();
        self.stop_signal_sender = Some(stop_sender);

        let processing_handle = thread::Builder::new()
            .name("audio-processor".to_string())
            .spawn(move || {
                tracing::info!("Audio processing thread started.");

                let mut processor = AudioProcessor::new(DEFAULT_FFT_SIZE);
                loop {
                    // Check for stop signal first
                    match stop_receiver.try_recv() {
                        Ok(_) | Err(mpsc::TryRecvError::Disconnected) => {
                            tracing::info!("Stop signal received or channel disconnected. Exiting processing thread.");
                            break;
                        }
                        // No signal, continue
                        Err(mpsc::TryRecvError::Empty) => {}
                    }

                    // Wait for the next chunk of samples with a timeout
                    match sample_chunk_receiver.recv_timeout(Duration::from_millis(200)) {
                        Ok(samples) => {
                            // Stereo to Mono Conversion
                            // TODO: Currently this just takes the first channel. Look into
                            //   averaging channels or find some crate to handle stereo
                            let mono_samples: Option<AudioAnalysisData> = if source_channels == 1 {
                                processor.process_samples(&samples)
                            } else if source_channels > 1 && !samples.is_empty() {
                                // Process first channel directly from input slice:
                                // This is a naive approach - taking the first channel's data, assuming
                                // interleaved samples [L, R, L, R, ...].
                                // This allocation creates a temporary Vec owned by the thread for this iteration.
                                // Using the collected Vec directly causes lifetime issues...
                                // The borrowed `samples` slice must be processed directly...
                                // TODO: Consider processing in place, or passing a mutable buffer maybe
                                let first_channel_samples: Vec<f32> = samples.iter().step_by(source_channels as usize).cloned().collect();
                                processor.process_samples(&first_channel_samples)
                            } else {
                                // No samples or 0 channels
                                None
                            };

                            if let Some(data) = mono_samples {
                                if let Err(e) = analysis_sender.try_send(data) {
                                    if matches!(e, mpsc::TrySendError::Disconnected(_)) {
                                        // Exit if receiver is gone
                                        tracing::error!("Analysis data channel disconnected: {}", e);
                                        break;
                                    }
                                    // TODO: Could add else here to log `Channel Full` or `data dropped`
                                }
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            // Timeouts are expected if playback is paused or buffer underruns occur.
                            // Continue loop to check for stop signal...
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            // Broadcaster source (SampleBroadcaster) has ended or dropped its sender
                            tracing::info!("Sample chunk channel disconnected. Exiting processing thread.");
                            break;
                        }
                    }
                }

                tracing::info!("Audio processing thread finished.");
            }).map_err(|e| format!("Failed to spawn processing thread: {}", e))?;

        self.processing_thread_handle = Some(processing_handle);

        // Setup playback sink
        let broadcaster =
            SampleBroadcaster::new(decoder_f32, sample_chunk_sender, SAMPLES_PER_CHUNK);
        let sink = Sink::try_new(&self.stream_handle)
            .map_err(|e| format!("Failed to create sink: {}", e))?;

        // Apply the current volume to the new sink
        sink.set_volume(self.current_volume);

        // Play the broadcaster source
        sink.append(broadcaster);
        sink.play();
        self.sink = Some(sink);

        self.state = PlaybackState::Playing;
        tracing::info!("Playing file: {}", file_path);

        Ok(())
    }

    // Stops playback and joins the processing thread gracefully
    fn stop_playback_and_processing(&mut self) {
        tracing::debug!("Stopping playback and processing thread...");

        // 1. Stop the sink, and prevent AudioManager from asking SampleBroadcaster for more samples
        if let Some(sink) = self.sink.take() {
            // take() removes the sink from Option
            sink.stop();
            drop(sink); // Explicitly drop sink here
            tracing::debug!("Audio sink stopped and dropped.");
        }
        // The SampleBroadcaster and its `sample_chunk_sender` are dropped when the sink drops.
        // This causes the `sample_chunk_receiver` in the processing thread to eventually
        // receive `RecvTimeoutError::Disconnected`, allowing it to exit gracefully.

        // 2. Signal the processing thread to stop
        // TODO: This might be redundant with the `stop_signal_sender` being dropped... keeping for now
        if let Some(sender) = self.stop_signal_sender.take() {
            match sender.send(()) {
                Ok(_) => tracing::debug!("Stop signal sent to processing thread."),
                Err(_) => tracing::debug!("Processing thread stop channel already closed."),
            }
        }

        // 3. Wait for the processing thread to finish
        if let Some(handle) = self.processing_thread_handle.take() {
            match handle.join() {
                Ok(_) => tracing::debug!("Processing thread joined successfully."),
                Err(e) => tracing::error!("Failed to join processing thread: {:?}", e),
            }
        } else {
            tracing::debug!("No processing thread handle found to join.");
        }

        // Reset state
        // TODO: Could set state to `Loaded` if we want to replay without reload...
        // self.current_file_path is available at this point for potential replay
        self.state = PlaybackState::Idle;
        tracing::debug!("Playback and processing stopped.");
    }

    // Processing thread will keep running after this call until `recv_timeout` times out.
    pub fn pause_playback(&mut self) {
        if self.state == PlaybackState::Playing {
            if let Some(sink) = &self.sink {
                if !sink.is_paused() {
                    sink.pause();
                    self.state = PlaybackState::Paused;
                    tracing::info!("Playback paused.");
                }
            }
        }
    }

    // Note: Resuming from Loaded state might require re-running `load_and_play_file`
    // This depends on whether the processing thread needs restarting...
    // TODO: Test if we have issues resuming from 'Loaded'... should probably do it in a more careful way.
    pub fn resume_playback(&mut self) {
        if self.state == PlaybackState::Paused {
            if let Some(sink) = &self.sink {
                if sink.is_paused() {
                    sink.play();
                    self.state = PlaybackState::Playing;
                    tracing::info!("Playback resumed.");
                }
            }
        }
    }

    // Gets the current playback state.
    // Does not query the sink actively.
    pub fn get_state(&self) -> PlaybackState {
        self.state
    }

    pub fn get_current_file_path(&self) -> Option<&String> {
        self.current_file_path.as_ref()
    }

    // Checks if the underlying sink has finished playing
    // Used in `update` of AudioVisualizerApp
    pub fn check_and_update_finished_state(&mut self) {
        let mut sink_finished = false;
        if let Some(sink) = &self.sink {
            if sink.empty() && self.state == PlaybackState::Playing {
                sink_finished = true;
            }
        } else {
            // No sink - No need to Play / Pause
            if matches!(self.state, PlaybackState::Playing | PlaybackState::Paused) {
                // If sink disappeared this will prevent the player from entering some unknown state
                self.state = PlaybackState::Idle;
            }
        }

        if sink_finished {
            tracing::info!(
                "Playback sink finished for: {:?}",
                self.current_file_path.as_deref().unwrap_or("unknown file")
            );
            // If the sink is empty the SampleBroadcaster has reached the end of the audio source.
            // We expect SampleBroadcaster to have sent its final chunk, and dropped the sender.
            // Processing thread should detect that sender disconnect and exit.
            // Transition state at this point to Loaded for potential replay.
            self.state = PlaybackState::Loaded;

            // We don't necessarily need to call stop_playback_and_processing here,
            // as the threads should naturally wind down. Joining happens on next load or Drop.
            // However, explicitly cleaning up the thread handle might be good practice.
            if let Some(handle) = self.processing_thread_handle.take() {
                tracing::debug!("Playback finished, attempting to join processing thread (non-blocking check)...");
                // TODO: Optionally check if finished without blocking UI thread?
                // A blocking join might be okay if playback truly ended...
                // For now, Drop ...or on next load handle the join...
                // return if not joining now.
                self.processing_thread_handle = Some(handle);
            }
            if let Some(sender) = self.stop_signal_sender.take() {
                // Attempt to send signal in case thread gets stuck on timeout... or something...
                // TODO: Log `SendError` here instead of swallowing it?
                let _ = sender.send(());
                self.stop_signal_sender = Some(sender);
            }
        }
    }
}

// Ensure graceful shutdown when AudioManager is dropped
// TODO: This might be redundant with the stop_playback_and_processing method.
impl Drop for AudioManager {
    fn drop(&mut self) {
        tracing::info!("Dropping AudioManager, stopping playback and processing...");
        self.stop_playback_and_processing();
    }
}
