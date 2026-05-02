//! cpal-based audio I/O for the speech REPL.
//!
//! `MicCapture` opens the default input device, pushing raw f32 samples
//! through a lock-free ring buffer. `SpeakerSink` opens the default
//! output device, draining f32 samples from a ring buffer. Both wrap
//! cpal streams whose callbacks cannot block or allocate; the SPSC
//! ring buffers from `ringbuf` are the pressure-relief boundary.
//!
//! `Resampler` adapts cpal's device sample rate to whichever rate the
//! consumer expects (16 kHz for silero/whisper, voice-config rate for
//! piper).

// Stub — real implementations land in Phase 2.
