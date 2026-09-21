//! Pipeline coordinator orchestrating audio capture, routing, execution, and synthesis.

use crate::audio::{AudioCaptureStream, AudioPlaybackSink, VadState, VoiceActivityDetector};
use crate::config::TychoConfig;
use crate::desktop::DesktopManager;
use crate::error::Result;
use crate::execution::DesktopExecutor;
use crate::generation::{AssistantPersona, GenerationClient};
use crate::memory::{ConversationHistory, MemPalaceClient};
use crate::models::manager::expand_tilde;
use crate::models::{ModelInventory, ModelManager};
use crate::pipeline::events::PipelineEvent;
use crate::router::{RoutingTier, UnifiedRouter};
use crate::stt::{SpeechToText, WhisperEngine};
use crate::tts::{SpeechSynthesizer, TextToSpeech};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

pub struct TychoPipelineCoordinator {
    pub config: TychoConfig,
    pub models: ModelInventory,
    pub capture: AudioCaptureStream,
    pub playback_sink: AudioPlaybackSink,
    pub vad: VoiceActivityDetector,
    pub stt: Arc<dyn SpeechToText>,
    pub router: UnifiedRouter,
    pub executor: DesktopExecutor,
    pub client: GenerationClient,
    pub tts: Arc<dyn TextToSpeech>,
    pub history: ConversationHistory,
    pub memory: MemPalaceClient,
    pub persona: AssistantPersona,
    pub event_tx: broadcast::Sender<PipelineEvent>,
    /// Sender cloned back into the run loop so background tasks (TTS
    /// provisioning) can deliver commands to `handle_ui_command`.
    #[cfg(feature = "orb")]
    pub ui_cmd_tx: Option<std::sync::mpsc::Sender<crate::ui::UiCommand>>,
    /// Live openWakeWord sidecar; `Some` means mic input is gated
    /// behind the configured wake model.
    pub wake: Option<crate::wake::WakeWordEngine>,
    /// Request channel into the run loop: `(model, spawn_result)` —
    /// the blocking sidecar spawn (python import takes seconds) runs
    /// on a worker thread so the audio loop never stalls.
    wake_spawn_tx:
        Option<std::sync::mpsc::Sender<(String, std::io::Result<crate::wake::WakeWordEngine>)>>,
    /// A spawn request is in flight; suppresses duplicate requests.
    wake_spawning: bool,
    /// `SpeakingStarted` was emitted and the output queue has not yet
    /// drained — the run loop emits `SpeakingFinished` only on the
    /// real playing→empty transition, so the orb/headless listeners
    /// see an honest playback state.
    speaking_reported: bool,
    /// Last assistant utterance, for the "repeat that" intent.
    last_response: Option<String>,
}

impl TychoPipelineCoordinator {
    pub async fn init(config: TychoConfig) -> Result<Self> {
        let (event_tx, _) = broadcast::channel(512);

        let model_mgr = ModelManager::new(config.models.clone());
        let models = model_mgr.ensure_models().await?;
        info!(
            "models verified: STT={:?}, TTS={:?}",
            models.stt_model_path, models.tts_model_path
        );

        let capture = AudioCaptureStream::with_device(
            config.audio.buffer_size,
            config.audio.input_device.clone(),
        );
        let playback_sink = AudioPlaybackSink::with_device(
            config.audio.output_device.clone(),
            config.audio.sample_rate,
        );
        let vad = VoiceActivityDetector::new(
            config.vad.energy_threshold,
            (config.vad.min_speech_duration_ms / 20) as usize,
            (config.vad.min_silence_duration_ms / 20) as usize,
        );

        let stt = Arc::new(WhisperEngine::new_with_path(
            &config.stt.engine,
            &config.stt.language,
            config
                .stt
                .model_path
                .clone()
                .unwrap_or_else(|| models.stt_model_path.clone()),
        )?);

        let mut router = UnifiedRouter::new(
            &config.router.laya_hf_repo,
            config.router.jev_api_key.clone(),
            &config.router.jev_endpoint,
            &config.router.jev_model,
            config.router.fast_path_confidence_threshold,
        );
        router.laya_enabled = config.router.fallback_to_laya;

        let desktop_mgr =
            DesktopManager::init_with_config(&config.desktop.backend, config.desktop.auto_detect)
                .await;
        info!("desktop backend bound: {}", desktop_mgr.backend_name());
        let executor = DesktopExecutor::new(desktop_mgr);

        match crate::generation::bootstrap::ensure_local_backend(
            &config.generation,
            Some(&config.router.jev_model),
        )
        .await
        {
            crate::generation::bootstrap::BackendStatus::Ready => {
                info!("local generation backend ready");
            }
            crate::generation::bootstrap::BackendStatus::Skipped(_) => {}
            crate::generation::bootstrap::BackendStatus::Unavailable(r) => {
                tracing::warn!(
                    "generation backend unavailable ({r}); fast-path desktop intents still work"
                );
            }
        }
        let client = GenerationClient::new(
            &config.generation.model,
            &config.generation.endpoint,
            config.generation.api_key.clone(),
        );
        match crate::generation::bootstrap::ensure_tts_engine(
            &config.tts.engine,
            config.generation.auto_setup,
        ) {
            crate::generation::bootstrap::BackendStatus::Ready => {
                info!("speech engine ready");
            }
            crate::generation::bootstrap::BackendStatus::Skipped(_) => {}
            crate::generation::bootstrap::BackendStatus::Unavailable(r) => {
                tracing::warn!("no speech engine available ({r}); responses will be silent");
            }
        }
        if !crate::wake::is_disabled(&config.wake.model) {
            match crate::wake::ensure_wake() {
                Ok(()) if crate::wake::sidecar_ready() => info!("wake-word sidecar ready"),
                Ok(()) => {
                    tracing::warn!("wake-word provisioning finished but no sidecar found")
                }
                Err(e) => {
                    tracing::warn!("wake-word sidecar unavailable ({e}); VAD activation applies")
                }
            }
        }
        let tts = Arc::new(SpeechSynthesizer::with_paths(
            &config.tts.engine,
            &config.tts.voice,
            config.tts.speed,
            models.tts_model_path.clone(),
            models.tts_voices_path.clone(),
            config.audio.sample_rate,
        ));
        let memory = MemPalaceClient::new(expand_tilde(&config.memory.db_path));
        let mut history = ConversationHistory::new(config.memory.max_history_turns);
        // Hydrate the session window with persisted turns from prior
        // runs (only when long-term memory is enabled).
        if config.memory.enable_long_term {
            match memory
                .query_recent("", config.memory.max_history_turns * 2)
                .await
            {
                Ok(prior) => {
                    for line in prior {
                        if let Some((speaker, text)) = line.split_once(": ") {
                            history.push(speaker, text);
                        }
                    }
                }
                Err(e) => tracing::warn!("memory hydration failed: {}", e),
            }
        }
        let persona = config.generation.persona;

        Ok(Self {
            config,
            models,
            capture,
            playback_sink,
            vad,
            stt,
            router,
            executor,
            client,
            tts,
            history,
            memory,
            persona,
            event_tx,
            #[cfg(feature = "orb")]
            ui_cmd_tx: None,
            wake: None,
            wake_spawn_tx: None,
            wake_spawning: false,
            speaking_reported: false,
            last_response: None,
        })
    }

    pub fn set_persona(&mut self, persona: AssistantPersona) {
        self.persona = persona;
    }

    /// Runs the capture -> VAD -> STT -> route -> execute -> TTS loop until
    /// the capture stream ends or SIGINT arrives. Speech onset during
    /// playback triggers barge-in interruption. Per-utterance failures are
    /// logged and the loop continues listening.
    ///
    /// With the `orb` feature an overlay provides pointer-driven control:
    /// `ui.mode` selects `auto` (continuous VAD), `manual` (click to
    /// capture one utterance), or `off` (audio ignored entirely).
    #[cfg_attr(not(feature = "orb"), allow(unused_labels))]
    pub async fn run_forever(&mut self) -> Result<()> {
        let mut frames = self.capture.start()?;

        #[cfg(feature = "orb")]
        let (ui_cmd_tx, ui_cmd_rx) = std::sync::mpsc::channel::<crate::ui::UiCommand>();
        #[cfg(feature = "orb")]
        {
            self.ui_cmd_tx = Some(ui_cmd_tx.clone());
        }
        #[cfg(feature = "orb")]
        let ui_mode = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
            crate::ui::ActivationMode::parse(&self.config.ui.mode).as_u8(),
        ));
        #[cfg(feature = "orb")]
        let _orb = if self.config.ui.orb {
            let pos = if self.config.ui.position.trim().eq_ignore_ascii_case("auto") {
                crate::ui::OrbPosition::auto_detect()
            } else {
                crate::ui::OrbPosition::parse(&self.config.ui.position)
            };
            crate::ui::orb::OrbHandle::spawn(
                self.event_tx.subscribe(),
                ui_cmd_tx,
                std::sync::Arc::clone(&ui_mode),
                crate::ui::orb::StartupPlacement {
                    position: pos,
                    margin_x: self.config.ui.margin_x,
                    margin_y: self.config.ui.margin_y,
                },
                crate::ui::settings_snapshot(&self.config),
            )
        } else {
            None
        };
        // In-progress triggered capture: `(quiet_frames, total_frames)`.
        // Used by orb clicks (manual mode) and by wake-word detections.
        let mut manual: Option<(usize, usize)> = None;

        // Wake-word gating runs headless too — the sidecar only needs
        // mic frames; the settings surface is just how it's toggled.
        // The blocking spawn (python + onnx import) runs on a worker
        // thread and reports back through this channel.
        let (wake_spawn_tx, wake_spawn_rx) =
            std::sync::mpsc::channel::<(String, std::io::Result<crate::wake::WakeWordEngine>)>();
        self.wake_spawn_tx = Some(wake_spawn_tx);
        self.start_wake();

        let mut speech_buf: Vec<f32> = Vec::new();
        let mut preroll: std::collections::VecDeque<Vec<f32>> =
            std::collections::VecDeque::with_capacity(self.vad.preroll_frames() + 1);
        // Wake-word-free listening window armed after each completed
        // turn (Alexa-style follow-up); `None` while inactive.
        let mut follow_up_until: Option<std::time::Instant> = None;
        info!("tycho daemon listening on audio inputs");

        'outer: loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("shutdown signal received");
                    break;
                }
                frame = frames.recv() => {
                    let Some(frame) = frame else { break };

                    // Completed wake-sidecar spawns arrive on this
                    // channel; a stale model (config changed again
                    // while spawning) is dropped instead of installed.
                    while let Ok((model, res)) = wake_spawn_rx.try_recv() {
                        self.wake_spawning = false;
                        match res {
                            Ok(engine) if model == self.config.wake.model => {
                                info!("wake-word engine live: {model}");
                                self.wake = Some(engine);
                            }
                            Ok(_) => {
                                info!("discarding stale wake-word engine for {model}");
                                self.start_wake();
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "wake-word engine failed to start ({e}); VAD activation applies"
                                );
                                // Stale-model failure: retry for the
                                // currently configured model.
                                if model != self.config.wake.model {
                                    self.start_wake();
                                }
                            }
                        }
                    }

                    // Honest playback state: `SpeakingFinished` fires on
                    // the real playing→empty transition (natural drain
                    // or interrupt), not at enqueue time.
                    let playing = self.playback_sink.is_playing();
                    if self.speaking_reported && !playing {
                        self.speaking_reported = false;
                        let _ = self.event_tx.send(PipelineEvent::SpeakingFinished);
                    }

                    #[cfg(feature = "orb")]
                    {
                        while let Ok(cmd) = ui_cmd_rx.try_recv() {
                            if self
                                .handle_ui_command(
                                    cmd,
                                    &ui_mode,
                                    &mut manual,
                                    &mut speech_buf,
                                    &mut preroll,
                                )
                                .await
                            {
                                break 'outer;
                            }
                        }
                        // Off mode: no audio is buffered or processed at all.
                        if crate::ui::ActivationMode::load(&ui_mode)
                            == crate::ui::ActivationMode::Off
                        {
                            continue;
                        }
                    }

                    preroll.push_back(frame.clone());
                    if preroll.len() > self.vad.preroll_frames() {
                        preroll.pop_front();
                    }

                    // Echo gate: while Tycho is audibly speaking — or in
                    // the short refractory after playback drains — most
                    // mic energy is the room's copy of our own TTS, and
                    // with no AEC the safe move is to keep it out of the
                    // VAD and wake sidecar. Frames clearly louder than
                    // the playback level pass through: that is a person
                    // talking over us (barge-in), and the normal paths
                    // below will interrupt playback.
                    let echo_active =
                        playing || self.playback_sink.in_refractory(350);
                    if manual.is_none() && echo_active {
                        let echo_floor =
                            (self.playback_sink.played_rms() * 1.5).max(0.03);
                        let frame_rms =
                            VoiceActivityDetector::calculate_energy(&frame);
                        if frame_rms < echo_floor {
                            self.vad.reset();
                            continue;
                        }
                    }

                    let follow_up = follow_up_until
                        .map(|t| std::time::Instant::now() < t)
                        .unwrap_or(false);

                    // Wake-word gate: feed the sidecar and turn a
                    // detection into a triggered capture. While the
                    // engine is active the ambient VAD path below never
                    // fires — only a detection (or orb click) opens an
                    // utterance — unless a follow-up window is armed,
                    // which restores VAD activation until it expires.
                    let mut wake_hit = None;
                    if let Some(w) = &mut self.wake {
                        if manual.is_none() && !follow_up {
                            w.feed_f32(&frame, self.config.audio.sample_rate);
                        }
                        while let Some(hit) = w.try_detect() {
                            if manual.is_none()
                                && !follow_up
                                && !self.vad.in_speech()
                                && !w.cooling_down()
                            {
                                w.note_trigger();
                                wake_hit = Some(hit);
                                break;
                            }
                        }
                    }
                    if let Some(hit) = wake_hit {
                        manual = Some((0, 0));
                        self.interrupt_playback();
                        self.play_earcon(crate::audio::earcon::Earcon::Listen);
                        let _ = self.event_tx.send(PipelineEvent::SpeechStarted);
                        for f in preroll.drain(..) {
                            speech_buf.extend_from_slice(&f);
                        }
                        info!("wake word detected ({hit}) — capturing utterance");
                    }
                    if self.wake.is_some() && manual.is_none() && !follow_up {
                        continue;
                    }

                    // Manual mode waits for an explicit orb click.
                    #[cfg(feature = "orb")]
                    if crate::ui::ActivationMode::load(&ui_mode)
                        == crate::ui::ActivationMode::Manual
                        && manual.is_none()
                    {
                        continue;
                    }

                    if let Some((quiet, total)) = manual.as_mut() {
                        speech_buf.extend_from_slice(&frame);
                        let rms = VoiceActivityDetector::calculate_energy(&frame);
                        let _ = self
                            .event_tx
                            .send(PipelineEvent::SpeechProgress { rms });
                        *total += 1;
                        if rms < self.config.vad.energy_threshold {
                            *quiet += 1;
                        } else {
                            *quiet = 0;
                        }
                        let min_silence =
                            (self.config.vad.min_silence_duration_ms / 20) as usize;
                        // Hard cap: a stuck-noise manual capture ends after 10s.
                        let max_frames =
                            (self.config.audio.sample_rate as usize * 10) / frame.len().max(1);
                        if *quiet >= min_silence || *total >= max_frames {
                            manual = None;
                            if let Some(w) = &mut self.wake {
                                w.drain();
                                w.note_trigger();
                            }
                            let _ = self.event_tx.send(PipelineEvent::SpeechEnded);
                            let responded =
                                self.finish_utterance(&mut speech_buf, true).await;
                            self.post_utterance(&mut frames, responded, &mut follow_up_until);
                        }
                        continue;
                    }

                    match self.vad.process_frame(&frame) {
                        VadState::SpeechStart => {
                            self.interrupt_playback();
                            let _ = self.event_tx.send(PipelineEvent::SpeechStarted);
                            for f in preroll.drain(..) {
                                speech_buf.extend_from_slice(&f);
                            }
                        }
                        VadState::InSpeech => {
                            speech_buf.extend_from_slice(&frame);
                            let rms = (frame.iter().map(|s| s * s).sum::<f32>()
                                / frame.len() as f32)
                                .sqrt();
                            let _ = self
                                .event_tx
                                .send(PipelineEvent::SpeechProgress { rms });
                        }
                        VadState::SpeechEnd => {
                            speech_buf.extend_from_slice(&frame);
                            let _ = self.event_tx.send(PipelineEvent::SpeechEnded);
                            let responded =
                                self.finish_utterance(&mut speech_buf, false).await;
                            self.post_utterance(&mut frames, responded, &mut follow_up_until);
                        }
                        VadState::Silence => {}
                    }
                }
            }
        }

        self.capture.stop();
        Ok(())
    }

    /// Transcribes the buffered utterance and routes it through the
    /// query pipeline. `explicit_trigger` marks wake-word/orb-triggered
    /// captures — an empty transcript there earns a reprompt (silent
    /// idling reads as a missed trigger), while VAD false starts stay
    /// quiet. Returns true when Tycho spoke a response. Emits `Idle`
    /// only when playback is not pending — an in-flight response keeps
    /// the orb in Speaking until the queue drains.
    async fn finish_utterance(
        &mut self,
        speech_buf: &mut Vec<f32>,
        explicit_trigger: bool,
    ) -> bool {
        let heard = self
            .stt
            .transcribe_pcm(speech_buf, self.config.audio.sample_rate)
            .await;
        speech_buf.clear();
        self.playback_sink.reset();

        let mut responded = false;
        match heard {
            Ok(result) => {
                let text = result.text.trim().to_string();
                if text.is_empty() {
                    if explicit_trigger {
                        self.speak_soft("Sorry, I didn't catch that.").await;
                        responded = true;
                    }
                } else if is_noise_transcript(&text) || result.confidence < 20 {
                    // Noise captions and near-zero-confidence text never
                    // reach the router — an uncalibrated classifier will
                    // happily map "wind blowing" onto launch_browser.
                    info!(
                        "dropping noise transcript (confidence {}%): {:?}",
                        result.confidence, text
                    );
                } else {
                    info!("heard (confidence {}%): {}", result.confidence, text);
                    self.play_earcon(crate::audio::earcon::Earcon::Thinking);
                    match self.process_query(&text).await {
                        Ok(_) => responded = true,
                        Err(e) => {
                            tracing::warn!("query processing failed: {}", e);
                            self.speak_soft("Sorry, something went wrong.").await;
                            responded = true;
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("transcription failed: {}", e),
        }
        if !self.playback_sink.is_playing() {
            let _ = self.event_tx.send(PipelineEvent::Idle);
        }
        responded
    }

    /// Post-utterance bookkeeping: frames that accumulated while
    /// transcribing/generating are stale (recorded before the user
    /// could react) and get dropped — but only when playback is not
    /// active. While Tycho is speaking the buffered frames stay: they
    /// flow through the echo gate, which suppresses quiet playback
    /// echo yet still lets a loud barge-in reach the VAD. A spoken
    /// response arms the wake-word-free follow-up window.
    fn post_utterance(
        &mut self,
        frames: &mut mpsc::Receiver<Vec<f32>>,
        responded: bool,
        follow_up_until: &mut Option<std::time::Instant>,
    ) {
        if !self.playback_sink.is_playing() {
            while frames.try_recv().is_ok() {}
            self.vad.reset();
        }
        if responded && self.config.wake.follow_up_seconds > 0 {
            *follow_up_until = Some(
                std::time::Instant::now()
                    + std::time::Duration::from_secs(self.config.wake.follow_up_seconds),
            );
        }
    }

    /// Stops queued playback and reports the interruption truthfully:
    /// `SpeakingFinished` is emitted only if a spoken response was
    /// actually in flight.
    fn interrupt_playback(&mut self) {
        self.playback_sink.interrupt();
        if self.speaking_reported {
            self.speaking_reported = false;
            let _ = self.event_tx.send(PipelineEvent::SpeakingFinished);
        }
    }

    /// Enqueues a short synthesized cue (wake/listen, thinking) when
    /// `ui.earcons` is on. Earcons do not set `speaking_reported` —
    /// their drain must not emit `SpeakingFinished`.
    fn play_earcon(&self, kind: crate::audio::earcon::Earcon) {
        if !self.config.ui.earcons {
            return;
        }
        let pcm = crate::audio::earcon::render(kind, self.config.audio.sample_rate);
        let sink = self.playback_sink.clone();
        tokio::spawn(async move {
            let _ = sink.play_chunk(&pcm).await;
        });
    }

    /// Applies one orb UI command. Returns true when the run loop should
    /// shut down (`UiCommand::Shutdown`).
    #[cfg(feature = "orb")]
    async fn handle_ui_command(
        &mut self,
        cmd: crate::ui::UiCommand,
        ui_mode: &std::sync::atomic::AtomicU8,
        manual: &mut Option<(usize, usize)>,
        speech_buf: &mut Vec<f32>,
        preroll: &mut std::collections::VecDeque<Vec<f32>>,
    ) -> bool {
        use crate::ui::UiCommand;
        match cmd {
            UiCommand::ListenNow => {
                // Off mode must not arm a capture — it would sit stale
                // and fire a ghost utterance the moment the mode flips.
                let off =
                    crate::ui::ActivationMode::load(ui_mode) == crate::ui::ActivationMode::Off;
                if !off && manual.is_none() {
                    *manual = Some((0, 0));
                    self.interrupt_playback();
                    self.play_earcon(crate::audio::earcon::Earcon::Listen);
                    let _ = self.event_tx.send(PipelineEvent::SpeechStarted);
                    for f in preroll.drain(..) {
                        speech_buf.extend_from_slice(&f);
                    }
                    info!("manual listen triggered from orb");
                }
            }
            UiCommand::CancelListen => {
                // Double-click companion: drop a pending orb capture
                // silently — no reprompt, no response.
                if manual.take().is_some() {
                    speech_buf.clear();
                    self.vad.reset();
                    let _ = self.event_tx.send(PipelineEvent::Idle);
                    info!("manual listen cancelled (chat opened)");
                }
            }
            UiCommand::SubmitQuery(text) => {
                let q = text.trim().to_string();
                if !q.is_empty() {
                    match self.process_query(&q).await {
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!("chat query failed: {}", e);
                            self.speak_soft("Sorry, something went wrong.").await;
                        }
                    }
                }
            }
            UiCommand::SetMode(m) => {
                ui_mode.store(m.as_u8(), std::sync::atomic::Ordering::Relaxed);
                self.config.ui.mode = m.as_str().to_string();
                info!("activation mode set to {}", m.as_str());
                if let Err(e) = crate::config::save_ui_patch(Some(m.as_str()), None, None, None) {
                    tracing::warn!("could not persist ui.mode: {}", e);
                }
            }
            UiCommand::SetPlacement {
                position,
                margin_x,
                margin_y,
            } => {
                self.config.ui.position = position.as_str().to_string();
                let (mx, my) = if margin_x >= 0 && margin_y >= 0 {
                    self.config.ui.margin_x = margin_x;
                    self.config.ui.margin_y = margin_y;
                    (Some(margin_x), Some(margin_y))
                } else {
                    (None, None)
                };
                if let Err(e) = crate::config::save_ui_patch(None, Some(position.as_str()), mx, my)
                {
                    tracing::warn!("could not persist ui.position: {}", e);
                }
            }
            UiCommand::SetConfig { key, value } => {
                self.apply_config_value(&key, &value);
            }
            UiCommand::ReloadTts => {
                self.rebuild_tts();
            }
            UiCommand::ReloadWake => {
                self.start_wake();
            }
            UiCommand::OpenSettings | UiCommand::ToggleChat => {}
            UiCommand::OpenConfig => {
                if let Some(path) = crate::config::config_file_path() {
                    if !path.exists() {
                        let _ = crate::config::save_ui_patch(None, None, None, None);
                    }
                    if let Ok(mut child) = std::process::Command::new("xdg-open")
                        .arg(&path)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                    {
                        std::thread::spawn(move || {
                            let _ = child.wait();
                        });
                    }
                }
            }
            UiCommand::Shutdown => {
                info!("shutdown requested from orb menu");
                return true;
            }
        }
        false
    }

    /// Applies a settings-window write: updates the live components that
    /// honor the key immediately, mirrors it into `self.config`, and
    /// persists it to `tycho.toml` with its proper TOML type.
    #[cfg(feature = "orb")]
    fn apply_config_value(&mut self, key: &str, value: &str) {
        // Typed TOML representation per key family.
        let toml_value = crate::config::settings_toml_value(key, value);

        // Live application: components that read the value per use.
        match key {
            "vad.energy_threshold" => {
                if let Ok(v) = value.parse::<f32>() {
                    self.config.vad.energy_threshold = v;
                    self.vad.set_energy_threshold(v);
                }
            }
            "vad.min_silence_duration_ms" => {
                if let Ok(v) = value.parse::<u64>() {
                    self.config.vad.min_silence_duration_ms = v;
                    self.vad.set_min_silence_ms(v);
                }
            }
            "vad.min_speech_duration_ms" => {
                if let Ok(v) = value.parse::<u64>() {
                    self.config.vad.min_speech_duration_ms = v;
                    self.vad.set_min_speech_ms(v);
                }
            }
            "generation.persona" => {
                use crate::generation::AssistantPersona as P;
                let p = match value {
                    "sentinel" => Some(P::Sentinel),
                    "scholar" => Some(P::Scholar),
                    "consigliere" => Some(P::Consigliere),
                    "companion" => Some(P::Companion),
                    _ => None,
                };
                if let Some(p) = p {
                    self.config.generation.persona = p;
                    self.set_persona(p);
                }
            }
            "router.fast_path_confidence_threshold" => {
                if let Ok(v) = value.parse::<f32>() {
                    self.config.router.fast_path_confidence_threshold = v;
                    self.router.fast_path_threshold = v;
                }
            }
            "tts.speed" => {
                if let Ok(v) = value.parse::<f32>() {
                    self.config.tts.speed = v;
                    self.queue_tts_refresh();
                }
            }
            "tts.voice" => {
                self.config.tts.voice = value.to_string();
                self.queue_tts_refresh();
            }
            "tts.engine" => {
                self.config.tts.engine = value.to_string();
                self.queue_tts_refresh();
            }
            "stt.language" => self.config.stt.language = value.to_string(),
            "wake.model" => {
                self.config.wake.model = value.to_string();
                self.refresh_wake();
            }
            "wake.threshold" => {
                if let Ok(v) = value.parse::<f32>() {
                    self.config.wake.threshold = v;
                    self.refresh_wake();
                }
            }
            "wake.vad_gate" => {
                if let Ok(v) = value.parse::<f32>() {
                    self.config.wake.vad_gate = v;
                    self.refresh_wake();
                }
            }
            "wake.follow_up_seconds" => {
                if let Ok(v) = value.parse::<u64>() {
                    self.config.wake.follow_up_seconds = v;
                }
            }
            "ui.earcons" => self.config.ui.earcons = value.parse().unwrap_or(true),
            "generation.temperature" => {
                if let Ok(v) = value.parse::<f32>() {
                    self.config.generation.temperature = v;
                }
            }
            "generation.max_tokens" => {
                if let Ok(v) = value.parse::<u32>() {
                    self.config.generation.max_tokens = v;
                }
            }
            "generation.model" => self.config.generation.model = value.to_string(),
            "memory.enable_long_term" => {
                self.config.memory.enable_long_term = value.parse().unwrap_or(true)
            }
            "desktop.backend" => self.config.desktop.backend = value.to_string(),
            "ui.orb" => self.config.ui.orb = value.parse().unwrap_or(true),
            _ => {}
        }

        match toml_value {
            Some(v) => {
                info!("setting {} = {}", key, value);
                if let Err(e) = crate::config::save_config_patch(key, v) {
                    tracing::warn!("could not persist {}: {}", key, e);
                }
            }
            None => tracing::warn!("ignoring unparsable setting {} = {:?}", key, value),
        }
    }

    /// Start the wake-word sidecar if the config enables one and it is
    /// provisioned. The blocking spawn runs on a worker thread — the
    /// result lands on `wake_spawn_rx` inside `run_forever` so python
    /// import time never stalls the audio loop. Failures degrade to
    /// plain VAD activation, never a crash.
    fn start_wake(&mut self) {
        if self.wake.is_some()
            || self.wake_spawning
            || crate::wake::is_disabled(&self.config.wake.model)
        {
            return;
        }
        if !crate::wake::sidecar_ready() {
            tracing::warn!("wake-word sidecar not provisioned; VAD activation applies");
            return;
        }
        let model = self.config.wake.model.clone();
        let threshold = self.config.wake.threshold;
        let vad_gate = self.config.wake.vad_gate;
        if let Some(tx) = &self.wake_spawn_tx {
            let tx = tx.clone();
            self.wake_spawning = true;
            info!("starting wake-word sidecar: {model}");
            std::thread::spawn(move || {
                let res = crate::wake::WakeWordEngine::spawn(&model, threshold, vad_gate);
                let _ = tx.send((model, res));
            });
        } else {
            // Outside the run loop (e.g. direct calls): spawn inline.
            match crate::wake::WakeWordEngine::spawn(&model, threshold, vad_gate) {
                Ok(engine) => {
                    info!("wake-word engine live: {model}");
                    self.wake = Some(engine);
                }
                Err(e) => {
                    tracing::warn!("wake-word engine failed to start ({e}); VAD activation applies")
                }
            }
        }
    }

    /// Apply a wake-word config change live: swap the sidecar for the
    /// new model/threshold, or trigger provisioning when it is absent.
    #[cfg(feature = "orb")]
    fn refresh_wake(&mut self) {
        self.wake = None;
        if crate::wake::is_disabled(&self.config.wake.model) {
            info!("wake-word detection disabled");
        } else if crate::wake::sidecar_ready() {
            self.start_wake();
        } else {
            self.queue_wake_provision();
        }
    }

    /// Provision the openWakeWord sidecar in the background, then ask
    /// the daemon loop to start the engine.
    #[cfg(feature = "orb")]
    fn queue_wake_provision(&self) {
        let Some(tx) = self.ui_cmd_tx.clone() else {
            return;
        };
        info!("provisioning openWakeWord sidecar for wake-word detection");
        tokio::spawn(async move {
            let res = tokio::task::spawn_blocking(crate::wake::ensure_wake).await;
            match res {
                Ok(Ok(())) => info!("wake-word provisioning complete"),
                Ok(Err(e)) => tracing::warn!("wake-word provisioning incomplete: {e}"),
                Err(e) => tracing::warn!("wake-word provisioning task failed: {e}"),
            }
            let _ = tx.send(crate::ui::UiCommand::ReloadWake);
        });
    }

    /// Rebuilds the speech synthesizer from the current config. Called
    /// after TTS provisioning completes so engine/voice/speed changes
    /// take effect without a restart.
    #[cfg(feature = "orb")]
    fn rebuild_tts(&mut self) {
        let tts_dir = crate::tts::catalog::tts_dir(&self.config.models.cache_dir);
        let model = tts_dir.join(format!("{}.onnx", self.config.tts.voice));
        let voices = tts_dir.join(format!("{}.onnx.json", self.config.tts.voice));
        self.tts = Arc::new(SpeechSynthesizer::with_paths(
            self.config.tts.engine.clone(),
            self.config.tts.voice.clone(),
            self.config.tts.speed,
            model,
            voices,
            self.config.audio.sample_rate,
        ));
        info!(
            "speech engine reloaded: {} / {}",
            self.config.tts.engine, self.config.tts.voice
        );
    }

    /// Spawns best-effort provisioning for the selected engine+voice
    /// ("auto-getter"), then asks the run loop to rebuild the
    /// synthesizer via `UiCommand::ReloadTts`.
    #[cfg(feature = "orb")]
    fn queue_tts_refresh(&self) {
        let Some(tx) = self.ui_cmd_tx.clone() else {
            return;
        };
        let engine = self.config.tts.engine.clone();
        let voice = self.config.tts.voice.clone();
        let tts_dir = crate::tts::catalog::tts_dir(&self.config.models.cache_dir);
        tokio::spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                crate::tts::catalog::ensure_selection(&engine, &voice, &tts_dir)
            })
            .await;
            match res {
                Ok(Ok(())) => info!("tts provisioning complete"),
                Ok(Err(e)) => tracing::warn!("tts provisioning incomplete: {e}"),
                Err(e) => tracing::warn!("tts provisioning task failed: {e}"),
            }
            let _ = tx.send(crate::ui::UiCommand::ReloadTts);
        });
    }

    pub async fn process_query(&mut self, query: &str) -> Result<String> {
        let _ = self
            .event_tx
            .send(PipelineEvent::TranscriptionCompleted(query.to_string()));

        match self.router.route(query).await {
            RoutingTier::System1FastPath {
                intent,
                parameter,
                confidence,
            } => {
                let _ = self.event_tx.send(PipelineEvent::FastPathRouted {
                    intent: intent.clone(),
                    parameter: parameter.clone(),
                    confidence,
                });

                // Assistant-control intents are handled by the pipeline
                // itself — they never reach the desktop executor.
                match intent.as_str() {
                    "stop_assistant" => {
                        self.interrupt_playback();
                        let _ = self.event_tx.send(PipelineEvent::Idle);
                        return Ok("Stopped.".to_string());
                    }
                    "repeat_response" => {
                        let reply = self
                            .last_response
                            .clone()
                            .unwrap_or_else(|| "I haven't said anything yet.".to_string());
                        self.speak_soft(&reply).await;
                        return Ok(reply);
                    }
                    _ => {}
                }

                let res = self.executor.execute(&intent, parameter.as_deref()).await?;

                let _ = self.event_tx.send(PipelineEvent::DesktopActionExecuted {
                    output: res.output_message.clone(),
                    latency_micros: res.execution_duration_micros,
                });

                self.speak_soft(&res.output_message).await;

                self.last_response = Some(res.output_message.clone());
                self.history.push("user", query);
                self.history.push("tycho", &res.output_message);
                self.persist_turn(query, &res.output_message).await;

                Ok(res.output_message)
            }
            RoutingTier::System2Deliberative { query } => {
                let _ = self.event_tx.send(PipelineEvent::DeliberationRouted {
                    query: query.clone(),
                });
                let desktop_ctx = self.executor.context_summary().await;
                let mut sys_prompt = self.persona.system_instruction(&desktop_ctx);

                // Relevance-ranked long-term memory: surface stored
                // turns related to this query that the bounded recent
                // window no longer covers.
                if self.config.memory.enable_long_term {
                    match self.memory.query_relevant(&query, 4).await {
                        Ok(mems) if !mems.is_empty() => {
                            sys_prompt.push_str("\nRelevant past exchanges:\n");
                            for m in mems {
                                sys_prompt.push_str(&format!("- {m}\n"));
                            }
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("memory recall failed: {}", e),
                    }
                }

                let prior: Vec<crate::generation::client::ChatMessage> = self
                    .history
                    .turns()
                    .map(|t| crate::generation::client::ChatMessage {
                        role: if t.speaker == "tycho" {
                            "assistant".to_string()
                        } else {
                            "user".to_string()
                        },
                        content: t.utterance.clone(),
                    })
                    .collect();

                // The persona budget caps the configured token limit —
                // Sentinel answers stay terse even with max_tokens high.
                let opts = crate::generation::client::GenerateOptions {
                    temperature: self.config.generation.temperature,
                    max_tokens: self
                        .persona
                        .max_tokens()
                        .min(self.config.generation.max_tokens),
                };

                // Streaming path: TTS starts on the first complete
                // sentence instead of waiting for the full response.
                let mut full = String::new();
                let mut pending = String::new();
                let mut sample_count = 0usize;
                if let Ok(mut rx) = self
                    .client
                    .generate_conversation_stream(&query, &sys_prompt, &prior, opts)
                    .await
                {
                    while let Some(piece) = rx.recv().await {
                        match piece {
                            Ok(delta) => {
                                full.push_str(&delta);
                                pending.push_str(&delta);
                                while let Some(s) = take_sentence(&mut pending) {
                                    sample_count += self.speak(&s).await?;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("generation stream failed: {e}");
                                break;
                            }
                        }
                    }
                }

                if full.is_empty() {
                    // Non-streaming fallback — endpoints that ignore
                    // `stream: true` or fail outright land here; the
                    // same chunking means a long answer still starts
                    // speaking at sentence one.
                    let gen = self
                        .client
                        .generate_conversation_opts(&query, &sys_prompt, &prior, Some(opts))
                        .await?;
                    full = gen.content;
                    pending = full.clone();
                    pending.push(' ');
                    while let Some(s) = take_sentence(&mut pending) {
                        sample_count += self.speak(&s).await?;
                    }
                }
                if !pending.trim().is_empty() {
                    sample_count += self.speak(pending.trim()).await?;
                }
                let full = full.trim().to_string();
                if full.is_empty() {
                    return Err(crate::error::Error::Inference(
                        "generation produced no text".to_string(),
                    ));
                }

                let _ = self.event_tx.send(PipelineEvent::GenerationCompleted {
                    response: full.clone(),
                });
                let _ = self
                    .event_tx
                    .send(PipelineEvent::SynthesisCompleted { sample_count });
                self.last_response = Some(full.clone());
                self.history.push("user", &query);
                self.history.push("tycho", &full);
                self.persist_turn(&query, &full).await;

                Ok(full)
            }
        }
    }

    /// Spoken output. On success `SpeakingFinished` is deferred to the
    /// run loop, which reports it when the output queue actually drains;
    /// on failure it is emitted here to close out the speaking state.
    /// Returns the samples enqueued (0 when interrupted mid-enqueue);
    /// synthesis and playback errors propagate so the deliberative path
    /// can fail the turn honestly.
    async fn speak(&mut self, text: &str) -> Result<usize> {
        let pcm = self.tts.synthesize(text).await?;
        let _ = self.event_tx.send(PipelineEvent::SpeakingStarted);
        self.speaking_reported = true;
        match self.playback_sink.play_chunk(&pcm).await {
            Ok(true) => Ok(pcm.len()),
            Ok(false) => Ok(0),
            Err(e) => {
                self.speaking_reported = false;
                let _ = self.event_tx.send(PipelineEvent::SpeakingFinished);
                Err(e)
            }
        }
    }

    /// Best-effort variant of `speak` for prompts and reprompts —
    /// failures are logged, never fatal.
    async fn speak_soft(&mut self, text: &str) {
        if let Err(e) = self.speak(text).await {
            tracing::warn!("spoken output failed: {}", e);
        }
    }

    /// Appends a completed exchange to the persistent memory store.
    /// Noise transcripts are never persisted — they are not real
    /// conversation turns. Persistence failures are logged rather than
    /// failing the query.
    async fn persist_turn(&self, query: &str, response: &str) {
        if !self.config.memory.enable_long_term || is_noise_transcript(query) {
            return;
        }
        for line in [format!("user: {}", query), format!("tycho: {}", response)] {
            if let Err(e) = self.memory.store(&line).await {
                tracing::warn!("memory store failed: {}", e);
            }
        }
    }
}

/// True when a transcript is recognizably not a real utterance:
/// whisper's noise hallucinations collapse into repeated-token loops
/// ("you you you you", "wind blowing wind blowing"), and lone fillers
/// carry no intent. These must never reach the router — an
/// uncalibrated classifier will happily map "wind blowing" onto
/// launch_browser (an actual observed incident).
fn is_noise_transcript(text: &str) -> bool {
    // Strip non-speech caption groups — "(wind blowing)", "[noise]",
    // "*music*" — before judging what remains.
    let mut cleaned = String::with_capacity(text.len());
    let mut depth = 0usize;
    let mut starred = false;
    for c in text.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            '*' => starred = !starred,
            _ if depth == 0 && !starred => cleaned.push(c),
            _ => {}
        }
    }
    let words: Vec<String> = cleaned
        .to_lowercase()
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return true;
    }
    const FILLER: &[&str] = &[
        "um", "uh", "hmm", "hm", "mm", "mhm", "ah", "oh", "eh", "uhh", "erm",
    ];
    if words.iter().all(|w| FILLER.contains(&w.as_str())) {
        return true;
    }
    // All-identical token loop.
    if words.len() > 1 && words.iter().all(|w| w == &words[0]) {
        return true;
    }
    // Repeating n-gram loop (n up to 3), allowing a partial tail:
    // "wind blowing wind blowing wind".
    for n in 2..=3usize {
        if words.len() < n * 3 {
            continue;
        }
        let unit = &words[..n];
        if words.chunks(n).all(|c| unit[..c.len()] == *c) {
            return true;
        }
    }
    false
}

/// Pops the longest leading complete sentence from `buf` — text up to
/// and including the first `.`/`!`/`?`/newline followed by whitespace
/// or end-of-buffer. Returns `None` while no terminator exists yet, so
/// streaming TTS never cuts mid-clause.
fn take_sentence(buf: &mut String) -> Option<String> {
    let end = buf.char_indices().find_map(|(i, c)| {
        if !matches!(c, '.' | '!' | '?' | '\n') {
            return None;
        }
        let after = &buf[i + c.len_utf8()..];
        after
            .chars()
            .next()
            .map(|n| n.is_whitespace())
            .unwrap_or(true)
            .then_some(i + c.len_utf8())
    })?;
    let sentence: String = buf.drain(..end).collect();
    let trimmed = sentence.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::{is_noise_transcript, take_sentence};

    #[test]
    fn noise_transcripts_rejected() {
        for t in [
            "(wind blowing)",
            "you you you you you",
            "wind blowing wind blowing wind blowing",
            "the the the the",
            "um uh hmm",
            "thank you thank you thank you",
            "   ",
            "...",
        ] {
            assert!(is_noise_transcript(t), "{t:?} should be noise");
        }
    }

    #[test]
    fn real_speech_passes() {
        for t in [
            "switch to workspace 2",
            "what is the weather like",
            "please open firefox",
            "no no, that's not right",
            "yes yes, go ahead and do it",
            "thank you",
        ] {
            assert!(!is_noise_transcript(t), "{t:?} is real speech");
        }
    }

    #[test]
    fn sentences_split_on_terminators() {
        let mut buf = "Hello there. How are".to_string();
        assert_eq!(take_sentence(&mut buf).as_deref(), Some("Hello there."));
        assert_eq!(buf, " How are");
        assert_eq!(take_sentence(&mut buf), None);

        let mut buf = "One! Two? Three".to_string();
        assert_eq!(take_sentence(&mut buf).as_deref(), Some("One!"));
        assert_eq!(take_sentence(&mut buf).as_deref(), Some("Two?"));

        // No terminator yet — nothing emitted.
        let mut buf = "still going".to_string();
        assert_eq!(take_sentence(&mut buf), None);
    }
}
