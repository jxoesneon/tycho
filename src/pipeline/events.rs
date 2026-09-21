//! Pipeline event definitions — the broadcast feed that drives the orb,
//! the chat transcript, and headless diagnostics.

#[derive(Debug, Clone, PartialEq)]
pub enum PipelineEvent {
    /// VAD detected speech onset.
    SpeechStarted,
    /// Per-frame loudness (RMS) while speech is in progress.
    SpeechProgress {
        rms: f32,
    },
    /// VAD detected speech end; transcription begins.
    SpeechEnded,
    /// TTS audio playback has started.
    SpeakingStarted,
    /// TTS audio playback finished or was interrupted by barge-in.
    SpeakingFinished,
    /// The pipeline settled back to passive listening.
    Idle,
    TranscriptionCompleted(String),
    FastPathRouted {
        intent: String,
        parameter: Option<String>,
        confidence: f32,
    },
    DeliberationRouted {
        query: String,
    },
    DesktopActionExecuted {
        output: String,
        latency_micros: u64,
    },
    GenerationCompleted {
        response: String,
    },
    SynthesisCompleted {
        sample_count: usize,
    },
}
