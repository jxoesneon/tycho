//! Energy-based Voice Activity Detector (VAD) state machine.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Silence,
    SpeechStart,
    InSpeech,
    SpeechEnd,
}

pub struct VoiceActivityDetector {
    energy_threshold: f32,
    min_speech_frames: usize,
    min_silence_frames: usize,
    speech_frame_count: usize,
    silence_frame_count: usize,
    current_state: VadState,
}

impl VoiceActivityDetector {
    pub fn new(energy_threshold: f32, min_speech_frames: usize, min_silence_frames: usize) -> Self {
        Self {
            energy_threshold,
            min_speech_frames,
            min_silence_frames,
            speech_frame_count: 0,
            silence_frame_count: 0,
            current_state: VadState::Silence,
        }
    }

    /// Number of recent frames worth buffering before a confirmed speech
    /// onset so the beginning of an utterance is not clipped.
    pub fn preroll_frames(&self) -> usize {
        self.min_speech_frames
    }

    /// Live-adjustable voice energy gate (settings UI writes this while
    /// the loop runs).
    pub fn set_energy_threshold(&mut self, threshold: f32) {
        self.energy_threshold = threshold;
    }

    /// Live-adjustable end-of-utterance silence window in milliseconds.
    pub fn set_min_silence_ms(&mut self, ms: u64) {
        self.min_silence_frames = (ms / 20) as usize;
    }

    /// Live-adjustable minimum speech duration before an utterance is
    /// confirmed, in milliseconds (20 ms frames).
    pub fn set_min_speech_ms(&mut self, ms: u64) {
        self.min_speech_frames = (ms / 20) as usize;
    }

    /// Clears in-progress detection state — used when buffered frames
    /// are discarded after processing so stale audio cannot leave the
    /// detector mid-utterance.
    pub fn reset(&mut self) {
        self.speech_frame_count = 0;
        self.silence_frame_count = 0;
        self.current_state = VadState::Silence;
    }

    /// True while the detector is between onset and end-of-utterance —
    /// used to keep a wake-word hit from stacking a second capture on
    /// top of speech already in progress.
    pub fn in_speech(&self) -> bool {
        matches!(
            self.current_state,
            VadState::SpeechStart | VadState::InSpeech
        )
    }

    pub fn calculate_energy(frame: &[f32]) -> f32 {
        if frame.is_empty() {
            return 0.0;
        }
        let sum: f32 = frame.iter().map(|&s| s * s).sum();
        (sum / frame.len() as f32).sqrt()
    }

    pub fn process_frame(&mut self, frame: &[f32]) -> VadState {
        let energy = Self::calculate_energy(frame);
        let is_voice = energy >= self.energy_threshold;

        match self.current_state {
            VadState::Silence => {
                if is_voice {
                    self.speech_frame_count += 1;
                    if self.speech_frame_count >= self.min_speech_frames {
                        self.current_state = VadState::SpeechStart;
                        self.silence_frame_count = 0;
                    }
                } else {
                    self.speech_frame_count = 0;
                }
            }
            VadState::SpeechStart => {
                self.current_state = VadState::InSpeech;
            }
            VadState::InSpeech => {
                if is_voice {
                    self.silence_frame_count = 0;
                } else {
                    self.silence_frame_count += 1;
                    if self.silence_frame_count >= self.min_silence_frames {
                        self.current_state = VadState::SpeechEnd;
                        self.speech_frame_count = 0;
                    }
                }
            }
            VadState::SpeechEnd => {
                self.current_state = VadState::Silence;
                self.speech_frame_count = 0;
                self.silence_frame_count = 0;
            }
        }

        self.current_state
    }
}
