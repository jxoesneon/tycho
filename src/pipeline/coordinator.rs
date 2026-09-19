//! Pipeline coordinator orchestrating audio capture, routing, execution, and synthesis.

use crate::audio::{AudioPlaybackSink, VoiceActivityDetector};
use crate::config::TychoConfig;
use crate::desktop::DesktopManager;
use crate::error::{Error, Result};
use crate::execution::DesktopExecutor;
use crate::generation::{AssistantPersona, GenerationClient};
use crate::memory::ConversationHistory;
use crate::models::{ModelInventory, ModelManager};
use crate::pipeline::events::PipelineEvent;
use crate::router::{RoutingTier, UnifiedRouter};
use crate::stt::{SpeechToText, WhisperEngine};
use crate::tts::{KokoroEngine, TextToSpeech};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

pub struct TychoPipelineCoordinator {
    pub config: TychoConfig,
    pub models: ModelInventory,
    pub playback_sink: AudioPlaybackSink,
    pub vad: VoiceActivityDetector,
    pub stt: Arc<dyn SpeechToText>,
    pub router: UnifiedRouter,
    pub executor: DesktopExecutor,
    pub client: GenerationClient,
    pub tts: Arc<dyn TextToSpeech>,
    pub history: ConversationHistory,
    pub persona: AssistantPersona,
    pub event_tx: broadcast::Sender<PipelineEvent>,
}

impl TychoPipelineCoordinator {
    pub async fn init(config: TychoConfig) -> Result<Self> {
        let (event_tx, _) = broadcast::channel(512);

        let model_mgr = ModelManager::new(config.models.clone());
        let models = model_mgr.ensure_models().await?;
        info!("ONNX models verified: STT={:?}, TTS={:?}", models.stt_model_path, models.tts_model_path);

        let playback_sink = AudioPlaybackSink::new();
        let vad = VoiceActivityDetector::new(
            config.vad.energy_threshold,
            (config.vad.min_speech_duration_ms / 20) as usize,
            (config.vad.min_silence_duration_ms / 20) as usize,
        );

        let stt = Arc::new(WhisperEngine::new_with_path(
            &config.stt.engine,
            &config.stt.language,
            models.stt_model_path.clone(),
        ));

        let router = UnifiedRouter::new(
            &config.router.laya_hf_repo,
            config.router.jev_api_key.clone(),
            config.router.fast_path_confidence_threshold,
        );

        let desktop_mgr = DesktopManager::init_auto().await?;
        let executor = DesktopExecutor::new(desktop_mgr);

        let client = GenerationClient::new(&config.generation.model, &config.generation.endpoint, config.generation.api_key.clone());
        let tts = Arc::new(KokoroEngine::new_with_paths(
            &config.tts.voice,
            config.tts.speed,
            models.tts_model_path.clone(),
            models.tts_voices_path.clone(),
        ));
        let history = ConversationHistory::new(config.memory.max_history_turns);
        let persona = config.generation.persona;

        Ok(Self {
            config,
            models,
            playback_sink,
            vad,
            stt,
            router,
            executor,
            client,
            tts,
            history,
            persona,
            event_tx,
        })
    }

    pub fn set_persona(&mut self, persona: AssistantPersona) {
        self.persona = persona;
    }

    pub async fn process_query(&mut self, query: &str) -> Result<String> {
        let _ = self.event_tx.send(PipelineEvent::TranscriptionCompleted(query.to_string()));

        match self.router.route(query).await {
            RoutingTier::System1FastPath { intent, parameter, confidence } => {
                let _ = self.event_tx.send(PipelineEvent::FastPathRouted {
                    intent: intent.clone(),
                    parameter: parameter.clone(),
                    confidence,
                });

                let res = self.executor.execute(&intent, parameter.as_deref()).await
                    .map_err(|e| Error::Desktop(e.to_string()))?;

                let _ = self.event_tx.send(PipelineEvent::DesktopActionExecuted {
                    output: res.output_message.clone(),
                    latency_micros: res.execution_duration_micros,
                });

                self.history.push("user", query);
                self.history.push("tycho", &res.output_message);

                Ok(res.output_message)
            }
            RoutingTier::System2Deliberative { query } => {
                let _ = self.event_tx.send(PipelineEvent::DeliberationRouted { query: query.clone() });
                let desktop_ctx = self.executor.context_summary().await;
                let sys_prompt = self.persona.system_instruction(&desktop_ctx);

                let gen = self.client.generate(&query, &sys_prompt).await?;
                let _ = self.event_tx.send(PipelineEvent::GenerationCompleted { response: gen.content.clone() });

                let pcm = self.tts.synthesize(&gen.content).await?;
                let sample_count = pcm.len();
                self.playback_sink.play_chunk(&pcm).await;

                let _ = self.event_tx.send(PipelineEvent::SynthesisCompleted { sample_count });
                self.history.push("user", &query);
                self.history.push("tycho", &gen.content);

                Ok(gen.content)
            }
        }
    }
}
