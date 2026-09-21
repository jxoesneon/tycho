//! Integration test suite for the voice assistant pipeline.

use rust_voice_assistant::audio::{VadState, VoiceActivityDetector};
use rust_voice_assistant::config::TychoConfig;
use rust_voice_assistant::generation::AssistantPersona;
use rust_voice_assistant::pipeline::{PipelineEvent, TychoPipelineCoordinator};

mod common;

fn test_config() -> (TychoConfig, common::TempDir) {
    let (mut config, cache) = common::seeded_cache_config("int");
    config.generation.endpoint = common::openai_server("Acknowledged");
    (config, cache)
}

struct OkTts;

#[async_trait::async_trait]
impl rust_voice_assistant::tts::TextToSpeech for OkTts {
    async fn synthesize(&self, _text: &str) -> rust_voice_assistant::error::Result<Vec<f32>> {
        Ok(vec![0.05f32; 800])
    }
}

/// Injects doubles for every external boundary: mock desktop, stub TTS,
/// null audio output.
fn stub_boundaries(coordinator: &mut TychoPipelineCoordinator) {
    use rust_voice_assistant::audio::AudioPlaybackSink;
    use rust_voice_assistant::desktop::DesktopManager;
    use rust_voice_assistant::execution::DesktopExecutor;
    use std::sync::Arc;
    coordinator.executor = DesktopExecutor::new(DesktopManager::init_mock());
    coordinator.tts = Arc::new(OkTts);
    coordinator.playback_sink = AudioPlaybackSink::new_null();
}

#[test]
fn test_config_defaults() {
    let config = TychoConfig::default();
    assert_eq!(config.audio.sample_rate, 16000);
    assert_eq!(config.vad.energy_threshold, 0.02);
    assert_eq!(config.router.fast_path_confidence_threshold, 0.82);
}

#[test]
fn test_vad_state_transitions() {
    let mut vad = VoiceActivityDetector::new(0.05, 2, 2);
    let silent = vec![0.001f32; 320];
    let loud = vec![0.15f32; 320];

    assert_eq!(vad.process_frame(&silent), VadState::Silence);
    vad.process_frame(&loud);
    assert_eq!(vad.process_frame(&loud), VadState::SpeechStart);
    assert_eq!(vad.process_frame(&loud), VadState::InSpeech);
    vad.process_frame(&silent);
    assert_eq!(vad.process_frame(&silent), VadState::SpeechEnd);
}

#[tokio::test]
async fn test_pipeline_coordinator_end_to_end() {
    let (config, _cache) = test_config();
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);

    let fast_res = coordinator
        .process_query("switch to workspace 2")
        .await
        .unwrap();
    assert_eq!(fast_res, "Switched to workspace 2");

    let delib_res = coordinator
        .process_query("Explain gravitational waves")
        .await
        .unwrap();
    assert!(delib_res.contains("Acknowledged"));
}

#[tokio::test]
async fn test_pipeline_coordinator_events_and_persona() {
    let (config, _cache) = test_config();
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);
    let mut rx = coordinator.event_tx.subscribe();

    coordinator.set_persona(AssistantPersona::Scholar);
    assert_eq!(coordinator.persona, AssistantPersona::Scholar);

    coordinator
        .process_query("switch to workspace 2")
        .await
        .unwrap();
    let mut saw_fast = false;
    let mut saw_exec = false;
    for _ in 0..4 {
        match rx.try_recv() {
            Ok(PipelineEvent::FastPathRouted { intent, .. }) => {
                assert_eq!(intent, "switch_workspace_2");
                saw_fast = true;
            }
            Ok(PipelineEvent::DesktopActionExecuted { output, .. }) => {
                assert!(output.contains("workspace 2"));
                saw_exec = true;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(saw_fast && saw_exec);
}

#[tokio::test]
async fn test_pipeline_coordinator_fast_path_dispatch() {
    use rust_voice_assistant::desktop::DesktopManager;
    use rust_voice_assistant::execution::DesktopExecutor;

    let (config, _cache) = test_config();
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    coordinator.executor = DesktopExecutor::new(DesktopManager::init_mock());
    coordinator.playback_sink = rust_voice_assistant::audio::AudioPlaybackSink::new_null();

    let res = coordinator.process_query("focus nonexistent-window").await;
    let err = res.unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_pipeline_coordinator_run_forever_speech_cycle() {
    use rust_voice_assistant::audio::AudioCaptureStream;
    use std::sync::Arc;

    let (config, _cache) = test_config();
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);
    coordinator.stt = Arc::new(ScriptedStt(Ok(
        rust_voice_assistant::stt::TranscriptionResult {
            text: "switch to workspace 2".into(),
            is_final: true,
            confidence: 90,
        },
    )));

    // Script: silence, sustained loud speech (>12 frames), sustained silence
    // (>30 frames) — drives VAD through SpeechStart/InSpeech/SpeechEnd so the
    // utterance is transcribed, routed, and executed before the stream ends.
    let silent = vec![0.0f32; 512];
    let loud = vec![0.5f32; 512];
    let mut script = vec![silent.clone(); 5];
    script.extend(std::iter::repeat_n(loud, 15));
    script.extend(std::iter::repeat_n(silent, 35));
    coordinator.capture = AudioCaptureStream::new_scripted(script);

    let mut rx = coordinator.event_tx.subscribe();
    coordinator.run_forever().await.unwrap();

    let mut saw_transcription = false;
    let mut saw_exec = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            PipelineEvent::TranscriptionCompleted(_) => saw_transcription = true,
            PipelineEvent::DesktopActionExecuted { output, .. } => {
                assert!(output.contains("workspace 2"));
                saw_exec = true;
            }
            _ => {}
        }
    }
    assert!(saw_transcription && saw_exec);
}

struct ScriptedStt(std::result::Result<rust_voice_assistant::stt::TranscriptionResult, String>);

#[async_trait::async_trait]
impl rust_voice_assistant::stt::SpeechToText for ScriptedStt {
    async fn transcribe_pcm(
        &self,
        _samples: &[f32],
        _rate: u32,
    ) -> rust_voice_assistant::error::Result<rust_voice_assistant::stt::TranscriptionResult> {
        match &self.0 {
            Ok(r) => Ok(r.clone()),
            Err(e) => Err(rust_voice_assistant::Error::Audio(e.clone())),
        }
    }
}

fn speech_script() -> Vec<Vec<f32>> {
    let silent = vec![0.0f32; 512];
    let loud = vec![0.5f32; 512];
    let mut script = vec![silent.clone(); 5];
    script.extend(std::iter::repeat_n(loud, 15));
    script.extend(std::iter::repeat_n(silent, 35));
    script
}

#[tokio::test]
async fn test_pipeline_coordinator_speech_transcription_edges() {
    use rust_voice_assistant::audio::AudioCaptureStream;
    use rust_voice_assistant::stt::TranscriptionResult;
    use std::sync::Arc;

    // Transcription failure arm.
    let (config, _cache) = test_config();
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);
    coordinator.stt = Arc::new(ScriptedStt(Err("no model".into())));
    coordinator.capture = AudioCaptureStream::new_scripted(speech_script());
    coordinator.run_forever().await.unwrap();

    // Empty transcript arm.
    let (config, _cache) = test_config();
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);
    coordinator.stt = Arc::new(ScriptedStt(Ok(TranscriptionResult {
        text: "   ".into(),
        is_final: true,
        confidence: 0,
    })));
    coordinator.capture = AudioCaptureStream::new_scripted(speech_script());
    coordinator.run_forever().await.unwrap();

    // Query-processing failure arm: deliberative text + dead generation endpoint.
    let (mut config, _cache) = test_config();
    config.generation.endpoint = "http://127.0.0.1:1".into();
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);
    coordinator.stt = Arc::new(ScriptedStt(Ok(TranscriptionResult {
        text: "ponder the cosmos".into(),
        is_final: true,
        confidence: 90,
    })));
    coordinator.capture = AudioCaptureStream::new_scripted(speech_script());
    coordinator.run_forever().await.unwrap();
}

struct FailTts;

#[async_trait::async_trait]
impl rust_voice_assistant::tts::TextToSpeech for FailTts {
    async fn synthesize(&self, _text: &str) -> rust_voice_assistant::error::Result<Vec<f32>> {
        Err(rust_voice_assistant::Error::Audio("tts engine down".into()))
    }
}

#[tokio::test]
async fn test_pipeline_coordinator_tts_failure() {
    use std::sync::Arc;

    let (mut config, _cache) = test_config();
    config.router.fast_path_confidence_threshold = 1.5;
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    coordinator.tts = Arc::new(FailTts);
    let err = coordinator
        .process_query("explain the tides")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("tts engine down"));
}

#[tokio::test]
async fn test_pipeline_coordinator_playback_failure() {
    // Synthesis succeeds but the output device cannot be opened → the error
    // propagates honestly instead of pretending the audio played.
    let (mut config, _cache) = test_config();
    config.router.fast_path_confidence_threshold = 1.5;
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);
    coordinator.playback_sink = rust_voice_assistant::audio::AudioPlaybackSink::with_device(
        Some("definitely-no-such-output-xyz".into()),
        16000,
    );
    let err = coordinator
        .process_query("explain the tides")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("definitely-no-such-output-xyz"));
}

#[tokio::test]
async fn test_pipeline_coordinator_init_model_failure() {
    // Unreachable model endpoint → init fails honestly rather than booting
    // a half-provisioned pipeline.
    let mut config = TychoConfig::default();
    config.generation.auto_setup = false;
    config.models.hf_endpoint = "http://127.0.0.1:1".into();
    config.models.cache_dir =
        std::env::temp_dir().join(format!("tycho-init-fail-{}", std::process::id()));
    assert!(TychoPipelineCoordinator::init(config).await.is_err());

    // An STT model path that is a directory → engine construction fails.
    let (config, _cache) = test_config();
    let stt_path = config
        .models
        .cache_dir
        .join("stt")
        .join(&config.models.stt_model.target_filename);
    std::fs::remove_file(&stt_path).unwrap();
    std::fs::create_dir(&stt_path).unwrap();
    assert!(TychoPipelineCoordinator::init(config).await.is_err());
}

#[tokio::test]
async fn test_pipeline_coordinator_run_forever_sigint() {
    use rust_voice_assistant::audio::AudioCaptureStream;

    // SIGINT to self resolves the ctrl_c arm — the daemon's real shutdown.
    let (config, _cache) = test_config();
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);
    // Silent scripted frames that would never end on their own.
    coordinator.capture = AudioCaptureStream::new_scripted(vec![vec![0.001f32; 8]; 120_000]);
    let handle = tokio::spawn(async move { coordinator.run_forever().await });
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    std::process::Command::new("kill")
        .args(["-INT", &std::process::id().to_string()])
        .output()
        .unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_pipeline_coordinator_memory_persistence() {
    // A store seeded before init hydrates the conversation window —
    // prior-session context survives restarts.
    let (config, _cache) = test_config();
    let memory = rust_voice_assistant::memory::MemPalaceClient::new(&config.memory.db_path);
    memory.store("user: earlier question").await.unwrap();
    memory.store("tycho: earlier answer").await.unwrap();
    memory.store("malformed line").await.unwrap();

    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);
    assert_eq!(coordinator.history.len(), 2);
    assert!(coordinator
        .history
        .formatted_dialogue()
        .contains("earlier question"));

    // A completed exchange is appended to the persistent store.
    coordinator
        .process_query("switch to workspace 4")
        .await
        .unwrap();
    let stored = coordinator.memory.query_recent("", 20).await.unwrap();
    assert!(stored.iter().any(|l| l == "user: switch to workspace 4"));
    assert!(stored.iter().any(|l| l.starts_with("tycho: ")));
}

#[tokio::test]
async fn test_pipeline_coordinator_memory_failure_is_nonfatal() {
    // A db_path that is a directory makes every store/query fail: hydration
    // and persistence degrade to warnings, never fatal errors.
    let (mut config, _cache) = test_config();
    let bad_db = config.memory.db_path.with_file_name("db-as-dir");
    std::fs::create_dir_all(&bad_db).unwrap();
    config.memory.db_path = bad_db;

    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);
    assert!(coordinator.history.is_empty());

    coordinator
        .process_query("switch to workspace 6")
        .await
        .unwrap();
    assert_eq!(coordinator.history.len(), 2);
}

#[tokio::test]
async fn test_pipeline_coordinator_deliberative_events() {
    let (mut config, _cache) = test_config();
    // An unreachable threshold forces every query down the deliberative
    // path regardless of how the neural classifiers score it.
    config.router.fast_path_confidence_threshold = 1.5;
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    stub_boundaries(&mut coordinator);
    let mut rx = coordinator.event_tx.subscribe();

    coordinator
        .process_query("Explain gravitational waves")
        .await
        .unwrap();
    let mut kinds = std::collections::HashSet::new();
    for _ in 0..6 {
        match rx.try_recv() {
            Ok(PipelineEvent::TranscriptionCompleted(_)) => {
                kinds.insert("t");
            }
            Ok(PipelineEvent::DeliberationRouted { .. }) => {
                kinds.insert("d");
            }
            Ok(PipelineEvent::GenerationCompleted { .. }) => {
                kinds.insert("g");
            }
            Ok(PipelineEvent::SynthesisCompleted { .. }) => {
                kinds.insert("s");
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(
        kinds.contains("t") && kinds.contains("d") && kinds.contains("g") && kinds.contains("s"),
        "kinds={:?}",
        kinds
    );
}
