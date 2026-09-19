//! Pipeline event definitions for internal diagnostics.

#[derive(Debug, Clone, PartialEq)]
pub enum PipelineEvent {
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
