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
