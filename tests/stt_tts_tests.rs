//! STT and TTS engine tests.

use rust_voice_assistant::stt::{SpeechToText, WhisperEngine};
use rust_voice_assistant::tts::{SpeechSynthesizer, TextToSpeech};
use std::io::Write;
use std::path::PathBuf;

#[tokio::test]
async fn test_whisper_engine() {
    let engine = WhisperEngine::new("whisper", "en");
    assert_eq!(engine.model_name, "whisper");
    assert_eq!(engine.language, "en");
    assert!(engine.model_path.is_none());

    let empty = engine.transcribe_pcm(&[], 16000).await.unwrap();
    assert!(empty.text.is_empty());
    assert!(empty.is_final);
    assert_eq!(empty.confidence, 0);

    // No model path configured → honest error instead of a canned transcript.
    let err = engine
        .transcribe_pcm(&[0.1, 0.2, 0.3], 16000)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("model"));

    // Missing model file is rejected at construction.
    let missing = WhisperEngine::new_with_path(
        "whisper",
        "en",
        PathBuf::from("/tmp/definitely-missing.bin"),
    );
    assert!(missing.is_err());
}

#[tokio::test]
async fn test_whisper_engine_invalid_model_file() {
    // A file that exists but is not a ggml model fails honestly at load.
    let dir = std::env::temp_dir();
    let path = dir.join(format!("tycho-bad-model-{}.bin", std::process::id()));
    std::fs::write(&path, b"not-a-ggml-model").unwrap();
    let engine = WhisperEngine::new_with_path("whisper", "en", path.clone()).unwrap();
    let err = engine
        .transcribe_pcm(&[0.1; 1600], 16000)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("transcription") || err.to_string().contains("load"));
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn test_whisper_engine_model_deleted_after_construction() {
    // A model file removed between construction and first use reports
    // ModelNotFound rather than a load panic.
    let dir = std::env::temp_dir();
    let path = dir.join(format!("tycho-vanish-{}.bin", std::process::id()));
    std::fs::write(&path, b"temporary").unwrap();
    let engine = WhisperEngine::new_with_path("whisper", "en", path.clone()).unwrap();
    std::fs::remove_file(&path).unwrap();
    let err = engine
        .transcribe_pcm(&[0.1; 1600], 16000)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        rust_voice_assistant::error::Error::ModelNotFound(_)
    ));
}

#[tokio::test]
async fn test_whisper_engine_real_inference() {
    // Real whisper.cpp inference over the cached ggml model. The model is
    // fetched by `pull-models` (or cached under target/models by CI); when
    // absent the test skips rather than fabricating a transcript.
    let model = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/models/ggml-tiny.en.bin");
    if !model.is_file() {
        eprintln!("skipping real inference: no ggml model at {:?}", model);
        return;
    }
    let engine = WhisperEngine::new_with_path("whisper", "en", model).unwrap();
    // ~1s of 16 kHz silence runs full inference; text may be empty.
    let first = engine
        .transcribe_pcm(&[0.0f32; 16000], 16000)
        .await
        .unwrap();
    assert!(first.is_final);
    // Second call reuses the cached context.
    let second = engine.transcribe_pcm(&[0.05f32; 8000], 8000).await.unwrap();
    assert!(second.is_final);
    // Near-empty input: may decode to zero segments — the no-token
    // confidence arm. Either outcome is valid; the call must not panic.
    let _ = engine.transcribe_pcm(&[0.0f32; 64], 16000).await;
}

#[tokio::test]
async fn test_speech_synthesizer_surface() {
    let engine = SpeechSynthesizer::new("auto", "en_US-amy-medium", 1.0);
    assert_eq!(engine.voice, "en_US-amy-medium");
    assert_eq!(engine.engine, "auto");
    assert!(engine.model_path.is_none());

    let empty = engine.synthesize("").await.unwrap();
    assert!(empty.is_empty());

    let with_paths = SpeechSynthesizer::with_paths(
        "piper",
        "en_US-amy-medium",
        1.0,
        PathBuf::from("/tmp/voice.onnx"),
        PathBuf::from("/tmp/voice.onnx.json"),
        16000,
    );
    assert_eq!(with_paths.engine, "piper");
    assert_eq!(
        with_paths.model_path.as_deref(),
        Some(std::path::Path::new("/tmp/voice.onnx"))
    );
    assert_eq!(
        with_paths.voices_path.as_deref(),
        Some(std::path::Path::new("/tmp/voice.onnx.json"))
    );
    assert_eq!(with_paths.output_rate, 16000);
}

#[tokio::test]
async fn test_speech_synthesizer_missing_binary_errors() {
    // Explicit engines report honest errors when the tool is absent.
    for engine in ["espeak-ng", "espeak", "flite"] {
        let synth = SpeechSynthesizer::new(engine, "v", 1.0);
        match synth.synthesize("hello").await {
            Ok(_) => {} // tool genuinely installed on this host
            Err(e) => {
                assert!(e.to_string().contains("synthesis") || e.to_string().contains(engine))
            }
        }
    }
    // piper without a model path fails fast.
    let synth = SpeechSynthesizer::new("piper", "v", 1.0);
    assert!(synth.synthesize("hello").await.is_err());
}

/// Writes a minimal 8 kHz mono s16le WAV for the shim binaries to emit.
fn fixture_wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
    for &s in samples {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
    cursor.into_inner()
}

/// WAV fixtures for the decode arms: bit depths, float, and multichannel.
fn wav_variants(dir: &std::path::Path) {
    let write = |name: &str, spec: hound::WavSpec, samples: &[i32]| {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
        match spec.bits_per_sample {
            8 => samples
                .iter()
                .for_each(|&s| w.write_sample(s as i8).unwrap()),
            16 => samples
                .iter()
                .for_each(|&s| w.write_sample(s as i16).unwrap()),
            _ => samples.iter().for_each(|&s| w.write_sample(s).unwrap()),
        }
        w.finalize().unwrap();
        std::fs::write(dir.join(name), cursor.into_inner()).unwrap();
    };
    let int = |bits: u16, channels: u16| hound::WavSpec {
        channels,
        sample_rate: 8000,
        bits_per_sample: bits,
        sample_format: hound::SampleFormat::Int,
    };
    write("w32.wav", int(32, 1), &[1000, -2000]);
    write("w8.wav", int(8, 1), &[100, -100]);
    write("w24.wav", int(24, 1), &[1000]);
    write("stereo.wav", int(16, 2), &[1000, -1000, 500, 500]);

    // 32-bit float WAV.
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 8000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
    w.write_sample(0.5f32).unwrap();
    w.write_sample(-0.5f32).unwrap();
    w.finalize().unwrap();
    std::fs::write(dir.join("float.wav"), cursor.into_inner()).unwrap();

    std::fs::write(dir.join("garbage.wav"), b"not a wav stream").unwrap();
}

#[tokio::test]
async fn test_speech_synthesizer_real_subprocesses() {
    let dir = std::env::temp_dir().join(format!("tycho-tts-shims-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Fixture WAV (8 kHz, ~200 samples of a sine-ish wave).
    let wav = fixture_wav(
        &(0..200)
            .map(|i| ((i as f32 * 0.2).sin() * 10000.0) as i16)
            .collect::<Vec<_>>(),
        8000,
    );
    let wav_path = dir.join("fixture.wav");
    std::fs::write(&wav_path, &wav).unwrap();

    let shim = r#"#!/bin/sh
echo "$(basename "$0") $*" >> "$TYCHO_TTS_LOG"
if [ -n "$TYCHO_TTS_FAIL" ]; then
    echo "shim failure" >&2
    exit 1
fi
if [ "$(basename "$0")" = "piper" ]; then
    cat > /dev/null
    # raw s16le 22050 Hz silence-ish
    head -c 2000 /dev/zero
    exit 0
fi
cat "$TYCHO_TTS_WAV"
"#;
    let log = dir.join("calls.log");
    for bin in ["espeak", "espeak-ng", "flite", "piper", "mytts"] {
        let path = dir.join(bin);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(shim.as_bytes()).unwrap();
        drop(f);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    let old_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", dir.display(), old_path));
    std::env::set_var("TYCHO_TTS_WAV", &wav_path);
    std::env::set_var("TYCHO_TTS_LOG", &log);

    // espeak engine: WAV output decoded and resampled to output_rate.
    let synth = SpeechSynthesizer::new("espeak", "v", 1.0);
    let samples = synth.synthesize("hello world").await.unwrap();
    assert!(!samples.is_empty());
    assert!(samples.iter().any(|&s| s != 0.0));

    // auto: piper is preferred when a model path exists — raw s16le decoded.
    std::fs::write(dir.join("voice.onnx"), b"model").unwrap();
    std::fs::write(dir.join("voice.onnx.json"), b"{}").unwrap();
    let synth = SpeechSynthesizer::with_paths(
        "auto",
        "v",
        1.0,
        dir.join("voice.onnx"),
        dir.join("voice.onnx.json"),
        16000,
    );
    let samples = synth.synthesize("hello world").await.unwrap();
    assert!(!samples.is_empty());
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("piper --model"));
    assert!(calls.contains("--output-raw"));

    // auto without a model path prefers the best provisioned engine;
    // with an empty HOME no sidecars exist, so it reaches espeak-ng.
    let fake_home = dir.join("home");
    std::fs::create_dir_all(&fake_home).unwrap();
    let old_home = std::env::var("HOME").unwrap_or_default();
    std::env::set_var("HOME", &fake_home);
    let synth = SpeechSynthesizer::new("auto", "v", 1.0);
    // candidates() resolves lazily inside synthesize, so HOME stays
    // faked through the call.
    let samples = synth.synthesize("hello world").await.unwrap();
    std::env::set_var("HOME", &old_home);
    assert!(!samples.is_empty());
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("espeak-ng"));

    // flite path also decodes WAV.
    let synth = SpeechSynthesizer::new("flite", "v", 1.0);
    let samples = synth.synthesize("hello").await.unwrap();
    assert!(!samples.is_empty());
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("flite"));

    // Unknown engine name → treated as a WAV-stdout binary; missing → error.
    let synth = SpeechSynthesizer::new("definitely-missing-tts-xyz", "v", 1.0);
    let err = synth.synthesize("hello").await.unwrap_err();
    assert!(err.to_string().contains("no TTS engine"));

    // WavStdout arm: an arbitrary binary emitting WAV on --stdout.
    let synth = SpeechSynthesizer::new("mytts", "v", 1.0);
    let samples = synth.synthesize("hello").await.unwrap();
    assert!(!samples.is_empty());
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("mytts --stdout"));

    // Non-zero exits report honest synthesis errors per candidate.
    std::env::set_var("TYCHO_TTS_FAIL", "1");
    for (engine, name) in [("espeak", "espeak"), ("flite", "flite"), ("mytts", "mytts")] {
        let synth = SpeechSynthesizer::new(engine, "v", 1.0);
        let err = synth.synthesize("hello").await.unwrap_err();
        assert!(err.to_string().contains(name), "{}: {}", engine, err);
    }
    let synth = SpeechSynthesizer::with_paths(
        "piper",
        "v",
        1.0,
        dir.join("voice.onnx"),
        dir.join("voice.onnx.json"),
        16000,
    );
    let err = synth.synthesize("hello").await.unwrap_err();
    assert!(err.to_string().contains("piper failed"));
    std::env::remove_var("TYCHO_TTS_FAIL");

    // speed <= 0 clamps the piper length scale to 1.0.
    let synth = SpeechSynthesizer::with_paths(
        "piper",
        "v",
        0.0,
        dir.join("voice.onnx"),
        dir.join("voice.onnx.json"),
        16000,
    );
    assert!(!synth.synthesize("hi").await.unwrap().is_empty());
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("--length-scale 1.00"));

    // Decode arms: every WAV variant the engines can emit.
    wav_variants(&dir);
    for (file, expect_ok) in [
        ("w32.wav", true),
        ("w8.wav", true),
        ("stereo.wav", true),
        ("float.wav", true),
        ("w24.wav", false),
        ("garbage.wav", false),
    ] {
        std::env::set_var("TYCHO_TTS_WAV", dir.join(file));
        let synth = SpeechSynthesizer::new("espeak", "v", 1.0);
        let res = synth.synthesize("hello").await;
        match (res, expect_ok) {
            (Ok(samples), true) => assert!(!samples.is_empty(), "{}", file),
            (Err(e), false) => assert!(
                e.to_string().contains("WAV") || e.to_string().contains("bit depth"),
                "{}: {}",
                file,
                e
            ),
            (res, _) => panic!("{}: unexpected {:?}", file, res.map(|v| v.len())),
        }
    }

    std::env::set_var("PATH", &old_path);
    std::env::remove_var("TYCHO_TTS_WAV");
    std::env::remove_var("TYCHO_TTS_LOG");

    // With shims out of PATH, piper's not-found is collected into `tried`
    // (real piper on a host would simply succeed instead).
    let synth = SpeechSynthesizer::with_paths(
        "piper",
        "v",
        1.0,
        dir.join("voice.onnx"),
        dir.join("voice.onnx.json"),
        16000,
    );
    match synth.synthesize("hello").await {
        Ok(_) => {}
        Err(e) => assert!(
            e.to_string().contains("no TTS engine") || e.to_string().contains("piper"),
            "{}",
            e
        ),
    }

    // `auto` with no model and no binaries on PATH aggregates every
    // candidate name into the honest "no engine" error.
    let synth = SpeechSynthesizer::new("auto", "v", 1.0);
    match synth.synthesize("hello").await {
        Ok(_) => {} // a real engine exists on this host
        Err(e) => assert!(
            e.to_string().contains("tried") || e.to_string().contains("no TTS engine"),
            "{}",
            e
        ),
    }

    let _ = std::fs::remove_dir_all(&dir);
}
