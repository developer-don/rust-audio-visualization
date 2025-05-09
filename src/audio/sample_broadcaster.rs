use rodio::Source;
use std::sync::mpsc;
use std::time::Duration;

// TODO: `std::sync::mpsc` is not the most performant option, but for now it works.
// Can look at using `crossbeam-channel` (or `flume`?) in the future.

// Wrap audio source and send clones of sample chunks through a channel.
#[allow(dead_code)]
pub struct SampleBroadcaster<S>
where
    S: Source<Item = f32> + Send + 'static,
{
    source: S,
    // Bounded channel is essential to prevent unbounded memory use if receiver lags
    sample_chunk_sender: mpsc::SyncSender<Vec<f32>>,
    buffer: Vec<f32>,
    // TODO: Remove this if we're not going to do something with it elsewhere
    // Store sample rate for context if needed elsewhere
    sample_rate: u32,
}

impl<S> SampleBroadcaster<S>
where
    S: Source<Item = f32> + Send + 'static,
{
    pub fn new(
        source: S,                                       // Audio source (must yield f32 samples)
        sample_chunk_sender: mpsc::SyncSender<Vec<f32>>, // Bounded sender for sending chunks of samples
        buffer_capacity: usize, // The size of chunks to send (e.g., FFT size)
    ) -> Self {
        let sample_rate = source.sample_rate();

        SampleBroadcaster {
            source,
            sample_chunk_sender,
            buffer: Vec::with_capacity(buffer_capacity),
            sample_rate,
        }
    }
}

// Required for use with rodio::Sink
// TODO: Evaluate `#[inline]` on these methods to see if it impacts performance.
impl<S> Source for SampleBroadcaster<S>
where
    S: Source<Item = f32> + Send + 'static,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.source.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.source.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.source.total_duration()
    }
}

// TODO: Evaluate `#[inline]` on `next` below to see if it impacts performance.
impl<S> Iterator for SampleBroadcaster<S>
where
    S: Source<Item = f32> + Send + 'static,
{
    // We expect f32 elsewhere for SampleBroadcaster definitions.
    // Yield the same sample signature as the underlying source.
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        match self.source.next() {
            Some(sample) => {
                self.buffer.push(sample);

                // If the buffer is full, send a clone to the processing thread
                if self.buffer.len() == self.buffer.capacity() {
                    // Use try_send for non-blocking behavior.
                    // Drop chunks when / if the processing thread bogs down
                    match self.sample_chunk_sender.try_send(self.buffer.clone()) {
                        Ok(_) => {}
                        Err(mpsc::TrySendError::Full(_)) => {
                            tracing::trace!(
                                "Sample chunk channel full. Dropping audio analysis chunk."
                            );
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => {
                            // Processing thread might have panicked if we get here... for some reason.
                            // Could potentially stop the source here, log for now.
                            tracing::error!("Sample chunk channel disconnected.");
                        }
                    }
                    // Reset buffer for the next chunk
                    self.buffer.clear();
                }

                // Return the original sample for `rodio` playback
                Some(sample)
            }
            None => {
                // We've reached the end of the audio file.
                // Send remaining samples in the buffer if any.
                if !self.buffer.is_empty() {
                    // Blocking send here is to ensure the last partial chunk is processed.
                    match self.sample_chunk_sender.send(self.buffer.clone()) {
                        Ok(_) => {}
                        Err(e) => tracing::error!("Failed to send final sample chunk: {}", e),
                    }

                    // Reset buffer, we're done
                    self.buffer.clear();
                }

                // Send end signal to `rodio`
                None
            }
        }
    }
}
