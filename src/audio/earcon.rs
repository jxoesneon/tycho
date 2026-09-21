//! Synthesized earcons — short nonverbal audio cues for listen/wake
//! events. Pure PCM tone synthesis, no assets or TTS dependency.

/// The kinds of audio signifiers the pipeline can emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Earcon {
    /// Rising two-tone: capture opened (wake word or orb click).
    Listen,
    /// Low single blip: deliberate processing started.
    Thinking,
}

/// Renders an earcon to mono f32 PCM at `rate` Hz.
pub fn render(kind: Earcon, rate: u32) -> Vec<f32> {
    match kind {
        Earcon::Listen => render_notes(&[(660.0, 90), (880.0, 120)], rate),
        Earcon::Thinking => render_notes(&[(392.0, 140)], rate),
    }
}

/// Concatenates soft sine notes with 8 ms attack/release envelopes and
/// a 20 ms gap between them.
fn render_notes(notes: &[(f32, u32)], rate: u32) -> Vec<f32> {
    let gap = (rate as usize * 20) / 1000;
    let mut out = Vec::new();
    for (freq, ms) in notes {
        let n = (rate as usize * *ms as usize) / 1000;
        let env = (rate as usize * 8) / 1000;
        for i in 0..n {
            let t = i as f32 / rate as f32;
            let attack = (i as f32 / env as f32).min(1.0);
            let release = ((n - i) as f32 / env as f32).min(1.0);
            out.push((t * freq * std::f32::consts::TAU).sin() * 0.18 * attack.min(release));
        }
        out.extend(std::iter::repeat_n(0.0, gap));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earcons_produce_bounded_audio() {
        for kind in [Earcon::Listen, Earcon::Thinking] {
            let pcm = render(kind, 16000);
            assert!(!pcm.is_empty());
            assert!(pcm.iter().all(|s| s.abs() <= 0.2));
            assert!(pcm.iter().any(|s| s.abs() > 0.05));
        }
    }

    #[test]
    fn listen_is_a_rising_two_tone() {
        let pcm = render(Earcon::Listen, 16000);
        // 90ms + 120ms notes + a 20ms gap after each ≈ 4k samples.
        assert!(pcm.len() > 3000 && pcm.len() <= 4100);
    }
}
