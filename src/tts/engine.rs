//! Text-to-speech via real synthesizer subprocesses: piper (neural, when a
//! voice model is configured), espeak-ng/espeak, or flite. Output is
//! decoded to mono f32 and resampled to the configured output rate.

use super::catalog;
use super::traits::TextToSpeech;
use crate::audio::resample_linear;
use crate::error::{Error, Result};
use async_trait::async_trait;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Sample rates reported by the supported engines.
const PIPER_RATE: u32 = 22050;

pub struct SpeechSynthesizer {
    pub engine: String,
    pub voice: String,
    pub speed: f32,
    pub model_path: Option<PathBuf>,
    pub voices_path: Option<PathBuf>,
    pub output_rate: u32,
}

impl SpeechSynthesizer {
    pub fn new(engine: impl Into<String>, voice: impl Into<String>, speed: f32) -> Self {
        Self {
            engine: engine.into(),
            voice: voice.into(),
            speed,
            model_path: None,
            voices_path: None,
            output_rate: 16000,
        }
    }

    pub fn with_paths(
        engine: impl Into<String>,
        voice: impl Into<String>,
        speed: f32,
        model_path: PathBuf,
        voices_path: PathBuf,
        output_rate: u32,
    ) -> Self {
        Self {
            engine: engine.into(),
            voice: voice.into(),
            speed,
            model_path: Some(model_path),
            voices_path: Some(voices_path),
            output_rate,
        }
    }

    /// Ordered candidates for the configured engine. Explicit engines
    /// get the espeak chain appended as a safety net so a missing
    /// binary degrades to any speech instead of silence.
    fn candidates(&self) -> Vec<Candidate> {
        fn fallback() -> [Candidate; 3] {
            [
                Candidate::Espeak("espeak-ng"),
                Candidate::Espeak("espeak"),
                Candidate::Flite,
            ]
        }
        let mut v = Vec::new();
        match self.engine.as_str() {
            "auto" => {
                if self.model_path.as_ref().is_some_and(|p| p.is_file()) {
                    v.push(Candidate::Piper);
                }
                if catalog::kokoro_python().is_some() && catalog::kokoro_script().is_some() {
                    v.push(Candidate::Kokoro);
                }
                v.extend(fallback());
            }
            "piper" => {
                v.push(Candidate::Piper);
                v.extend(fallback());
            }
            "kokoro" => {
                v.push(Candidate::Kokoro);
                v.extend(fallback());
            }
            "vibevoice" => {
                v.push(Candidate::VibeVoice);
                v.extend(fallback());
            }
            "espeak" => v.extend([Candidate::Espeak("espeak"), Candidate::Flite]),
            "espeak-ng" => v.extend(fallback()),
            "flite" => v.push(Candidate::Flite),
            other => {
                // Unknown engine names are treated as an external binary
                // emitting WAV on stdout via `--stdout` — strictly, so a
                // typo'd engine reports an honest error instead of
                // silently downgrading.
                v.push(Candidate::WavStdout(other.to_string()));
            }
        }
        v
    }
}

enum Candidate {
    Piper,
    /// Python sidecar (`~/.local/share/tycho/kokoro`) emitting WAV.
    Kokoro,
    /// Python sidecar (`~/.local/share/tycho/vibevoice`) emitting WAV.
    VibeVoice,
    Espeak(&'static str),
    Flite,
    WavStdout(String),
}

impl Candidate {
    fn binary(&self) -> &str {
        match self {
            Self::Piper => "piper",
            Self::Kokoro => "kokoro",
            Self::VibeVoice => "vibevoice",
            Self::Espeak(bin) => bin,
            Self::Flite => "flite",
            Self::WavStdout(bin) => bin,
        }
    }
}

#[async_trait]
impl TextToSpeech for SpeechSynthesizer {
    async fn synthesize(&self, text: &str) -> Result<Vec<f32>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        let mut tried = Vec::new();
        for cand in self.candidates() {
            match self.run_candidate(&cand, text).await {
                Ok(pcm) => return Ok(pcm),
                Err(e) if not_found(&e) => tried.push(cand.binary().to_string()),
                Err(e) => return Err(e),
            }
        }
        Err(Error::Synthesis(format!(
            "no TTS engine available for '{}': tried {}",
            self.engine,
            tried.join(", ")
        )))
    }
}

impl SpeechSynthesizer {
    async fn run_candidate(&self, cand: &Candidate, text: &str) -> Result<Vec<f32>> {
        match cand {
            Candidate::Piper => {
                let model = self.model_path.clone().ok_or_else(|| {
                    Error::Synthesis("piper requires a voice model path".to_string())
                })?;
                let bin = catalog::piper_bin().ok_or_else(|| {
                    Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "piper binary not found",
                    ))
                })?;
                let length_scale = if self.speed > 0.0 {
                    1.0 / self.speed
                } else {
                    1.0
                };
                let mut child = Command::new(bin)
                    .arg("--model")
                    .arg(&model)
                    .arg("--length-scale")
                    .arg(format!("{:.2}", length_scale))
                    .arg("--output-raw")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(Error::Io)?;
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes()).await;
                    let _ = stdin.shutdown().await;
                }
                let out = child.wait_with_output().await.map_err(Error::Io)?;
                if !out.status.success() {
                    return Err(Error::Synthesis(format!(
                        "piper failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
                let pcm = decode_s16le(&out.stdout);
                Ok(resample_linear(&pcm, PIPER_RATE, self.output_rate))
            }
            Candidate::Kokoro => {
                let py = catalog::kokoro_python().ok_or_else(|| not_found_err("kokoro sidecar"))?;
                let script =
                    catalog::kokoro_script().ok_or_else(|| not_found_err("kokoro wrapper"))?;
                let tts_dir = self.tts_dir();
                let model = catalog::find_kokoro_model(&tts_dir)
                    .ok_or_else(|| not_found_err("kokoro model"))?;
                let voices = catalog::find_kokoro_voices(&tts_dir)
                    .ok_or_else(|| not_found_err("kokoro voices"))?;
                let voice = if catalog::is_kokoro_voice(&self.voice) {
                    self.voice.clone()
                } else {
                    "af_heart".to_string()
                };
                let out = Command::new(py)
                    .arg(script)
                    .arg("--model")
                    .arg(&model)
                    .arg("--voices")
                    .arg(&voices)
                    .arg("--voice")
                    .arg(voice)
                    .arg("--speed")
                    .arg(format!("{:.2}", self.speed.max(0.5)))
                    .arg("--text")
                    .arg(text)
                    .output()
                    .await
                    .map_err(Error::Io)?;
                if !out.status.success() {
                    return Err(Error::Synthesis(format!(
                        "kokoro failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
                let (pcm, rate) = decode_wav(&out.stdout)?;
                Ok(resample_linear(&pcm, rate, self.output_rate))
            }
            Candidate::VibeVoice => {
                let py = catalog::vibevoice_python()
                    .ok_or_else(|| not_found_err("vibevoice sidecar"))?;
                let script = catalog::vibevoice_script()
                    .ok_or_else(|| not_found_err("vibevoice wrapper"))?;
                let repo = catalog::vibevoice_dir()
                    .map(|d| d.join("repo"))
                    .filter(|d| d.is_dir())
                    .ok_or_else(|| not_found_err("vibevoice repo"))?;
                let speaker = match self.voice.to_ascii_lowercase().as_str() {
                    "davis" => "Davis",
                    "emma" => "Emma",
                    "frank" => "Frank",
                    "grace" => "Grace",
                    "mike" => "Mike",
                    _ => "Carter",
                };
                let out = Command::new(py)
                    .arg(script)
                    .arg("--repo")
                    .arg(&repo)
                    .arg("--speaker")
                    .arg(speaker)
                    .arg("--text")
                    .arg(text)
                    .output()
                    .await
                    .map_err(Error::Io)?;
                if !out.status.success() {
                    return Err(Error::Synthesis(format!(
                        "vibevoice failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
                let (pcm, rate) = decode_wav(&out.stdout)?;
                Ok(resample_linear(&pcm, rate, self.output_rate))
            }
            Candidate::Espeak(bin) => {
                let wpm = (175.0 * self.speed.max(0.25)) as u32;
                let out = Command::new(bin)
                    .arg("-s")
                    .arg(wpm.to_string())
                    .arg("--stdout")
                    .arg(text)
                    .output()
                    .await
                    .map_err(Error::Io)?;
                if !out.status.success() {
                    return Err(Error::Synthesis(format!(
                        "{} failed: {}",
                        bin,
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
                let (pcm, rate) = decode_wav(&out.stdout)?;
                Ok(resample_linear(&pcm, rate, self.output_rate))
            }
            Candidate::Flite => {
                let out = Command::new("flite")
                    .arg("-t")
                    .arg(text)
                    .arg("-o")
                    .arg("/dev/stdout")
                    .output()
                    .await
                    .map_err(Error::Io)?;
                if !out.status.success() {
                    return Err(Error::Synthesis(format!(
                        "flite failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
                let (pcm, rate) = decode_wav(&out.stdout)?;
                Ok(resample_linear(&pcm, rate, self.output_rate))
            }
            Candidate::WavStdout(bin) => {
                let out = Command::new(bin)
                    .arg("--stdout")
                    .arg(text)
                    .output()
                    .await
                    .map_err(Error::Io)?;
                if !out.status.success() {
                    return Err(Error::Synthesis(format!(
                        "{} failed: {}",
                        bin,
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
                let (pcm, rate) = decode_wav(&out.stdout)?;
                Ok(resample_linear(&pcm, rate, self.output_rate))
            }
        }
    }
}

impl SpeechSynthesizer {
    /// Directory holding managed TTS assets (piper voices, kokoro
    /// models): the parent of the configured piper voice model, or the
    /// default `~/.local/share/tycho/models/tts` location.
    fn tts_dir(&self) -> PathBuf {
        self.model_path
            .as_deref()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .or_else(|| catalog::data_dir().map(|d| d.join("models/tts")))
            .unwrap_or_else(|| PathBuf::from("models/tts"))
    }
}

fn not_found_err(what: &str) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("{what} not found"),
    ))
}

fn not_found(e: &Error) -> bool {
    matches!(e, Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound)
}

/// Decodes a WAV byte stream to mono f32 samples and its sample rate.
fn decode_wav(bytes: &[u8]) -> Result<(Vec<f32>, u32)> {
    let reader = hound::WavReader::new(Cursor::new(bytes))
        .map_err(|e| Error::Synthesis(format!("invalid WAV from engine: {}", e)))?;
    let spec = reader.spec();
    let channels = (spec.channels as usize).max(1);
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => match spec.bits_per_sample {
            16 => reader
                .into_samples::<i16>()
                .map(|s| s.map(|v| v as f32 / i16::MAX as f32).unwrap_or(0.0))
                .collect(),
            32 => reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 / i32::MAX as f32).unwrap_or(0.0))
                .collect(),
            8 => reader
                .into_samples::<i8>()
                .map(|s| s.map(|v| v as f32 / i8::MAX as f32).unwrap_or(0.0))
                .collect(),
            bits => {
                return Err(Error::Synthesis(format!(
                    "unsupported WAV bit depth: {}",
                    bits
                )))
            }
        },
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| s.unwrap_or(0.0))
            .collect(),
    };
    let mono = raw
        .chunks(channels)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect();
    Ok((mono, spec.sample_rate))
}

/// Decodes little-endian signed 16-bit PCM to f32.
fn decode_s16le(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b) as f32 / i16::MAX as f32)
        .collect()
}
