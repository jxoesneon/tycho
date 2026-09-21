# Tycho Automated Model Pulling & HuggingFace First-Run Bootstrap

Tycho is designed to be fully zero-configuration on first launch. Rather than requiring users to manually hunt down, convert, or place neural network weights across disparate system directories, Tycho incorporates an automated **First-Run HuggingFace Model Puller**.

---

## 1. Architectural Workflow

```
                        [ First Run Detected ]
                                  │
                  Is local model cache populated?
                                  │
                 ┌────────────────┴────────────────┐
                 │ NO                              │ YES
                 ▼                                 ▼
      [ Query HuggingFace Hub ]        [ Load Local Model Weights ]
                 │                                 │
       Stream Weights to *.tmp                     │
                 │                                 │
      Verify File Integrity (Bytes)                │
                 │                                 │
       Atomic Rename to Target                     │
                 │                                 │
                 └────────────────┬────────────────┘
                                  │
                                  ▼
                   [ Initialize STT & TTS Engines ]
```

1. **Detection**: Upon starting the coordinator (`TychoPipelineCoordinator::init`) or launching via CLI, Tycho inspects the resolved model cache directory (`~/.local/share/tycho/models`).
2. **Missing Weights Check**: If the required STT (`ggml-tiny.en.bin`), TTS voice (`en_US-amy-medium.onnx`), voice config (`en_US-amy-medium.onnx.json`), or router (`laya_intent_classifier.onnx`) files are absent:
   - Tycho emits informative logging.
   - It queries the configured HuggingFace endpoint (`https://huggingface.co`).
3. **Atomic Download**:
   - Streams weights directly from the official HuggingFace repository (`/resolve/main/<filename>`).
   - Writes to a temporary download file (`<filename>.tmp_download`).
   - Validates the minimum byte threshold to safeguard against truncated or corrupted files.
   - Performs an atomic swap (`std::fs::rename`) into the cache hierarchy.
4. **Offline Resilience**:
   - Once downloaded, Tycho runs 100% locally and completely offline.
   - For air-gapped workstations, users can pre-populate `~/.local/share/tycho/models` or disable `auto_download_on_first_run`.

---

## 2. Default HuggingFace Repositories

| Subsystem | HuggingFace Repository | Repository Path | Default Local Target |
| :--- | :--- | :--- | :--- |
| **STT (Whisper, ggml)** | `ggerganov/whisper.cpp` | `ggml-tiny.en.bin` | `~/.local/share/tycho/models/stt/ggml-tiny.en.bin` |
| **TTS (Piper voice)** | `rhasspy/piper-voices` | `en/en_US/amy/medium/en_US-amy-medium.onnx` | `~/.local/share/tycho/models/tts/en_US-amy-medium.onnx` |
| **TTS (Voice config)** | `rhasspy/piper-voices` | `en/en_US/amy/medium/en_US-amy-medium.onnx.json` | `~/.local/share/tycho/models/tts/en_US-amy-medium.onnx.json` |
| **Router (Laya)** | `convaiinnovations/laya` | `model.onnx` | `~/.local/share/tycho/models/router/laya_intent_classifier.onnx` |

---

## 3. Configuration & Custom Repositories

In `tycho.toml`:

```toml
[models]
cache_dir = "~/.local/share/tycho/models"
auto_download_on_first_run = true
hf_endpoint = "https://huggingface.co"
# hf_token = "hf_..." # (Or set HF_TOKEN environment variable)

[models.stt_model]
repo_id = "ggerganov/whisper.cpp"
revision = "main"
filename = "ggml-tiny.en.bin"
target_filename = "ggml-tiny.en.bin"
expected_min_bytes = 33554432

[models.tts_model]
repo_id = "rhasspy/piper-voices"
revision = "main"
filename = "en/en_US/amy/medium/en_US-amy-medium.onnx"
target_filename = "en_US-amy-medium.onnx"
expected_min_bytes = 1048576

[models.tts_voices]
repo_id = "rhasspy/piper-voices"
revision = "main"
filename = "en/en_US/amy/medium/en_US-amy-medium.onnx.json"
target_filename = "en_US-amy-medium.onnx.json"
expected_min_bytes = 512
```

---

## 4. CLI Model Management

Users can pre-fetch, check, or force re-download models at any time using the Tycho CLI:

```bash
# Verify models and download if missing
tycho pull-models

# Force clean re-download of all model assets from HuggingFace
tycho pull-models --force
```

---

## 5. TTS Engines & Voices — Catalog and Auto-Getters

The settings window lists **every supported engine** (`auto`, `piper`,
`kokoro`, `vibevoice`, `espeak-ng`, `espeak`, `flite`) and **every
managed voice**, installed or not. Selecting an option that is absent
triggers its automatic getter, then the synthesizer is rebuilt live:

| Selection | Getter |
|---|---|
| piper voice (`en_US-*` / `en_GB-*`) | `<voice>.onnx` + `.onnx.json` from `rhasspy/piper-voices` (`v1.0.0`) into `<cache>/tts/` |
| kokoro engine / `af_*`-style voice | python venv at `~/.local/share/tycho/kokoro` (`kokoro-onnx` + `soundfile` + WAV-stdout wrapper) and `kokoro-v1.0.onnx` / `voices-v1.0.bin` from the `thewh1teagle/kokoro-onnx` `model-files-v1.0` release |
| vibevoice engine | clone `microsoft/VibeVoice` + venv at `~/.local/share/tycho/vibevoice` (`pip install -e`, multi-GB; CPU is slow — GPU recommended) |
| `espeak-ng` / `espeak` / `flite` | system packages — no getter; an actionable error is reported |
| `auto` | best available at synth time: piper → kokoro → espeak-ng → espeak → flite |

Notes:

- `TYCHO_NO_INSTALL` disables every getter; failures are logged, never
  fatal, and the pipeline keeps speaking through the espeak fallback
  chain.
- Files under a size floor (pre-allocated `ONNX_WEIGHT_CONTAINER_V1`
  stubs) do not count as installed and are re-fetched.
- `tts.engine`, `tts.voice`, and `tts.speed` apply live after
  provisioning completes — no restart needed.

---

## 6. Wake-Word Detection — Optional openWakeWord Sidecar

`[wake]` in `tycho.toml` is **off by default** — plain VAD activation.
Setting `wake.model` to a bundled openWakeWord name (`hey_jarvis`,
`alexa`, `hey_mycroft`, `timer`, `weather`) or a path to a custom
`.onnx` model gates the mic behind wake-word detection:

| Piece | Detail |
|---|---|
| sidecar | python venv at `~/.local/share/tycho/wake` (`openwakeword` + `onnxruntime`) + `oww_daemon.py` streaming wrapper |
| protocol | s16le 16 kHz mono on stdin → `READY <model>` handshake → `<model> <score>` lines on stdout; stderr appends to `~/.local/state/tycho/wake.log` |
| gating | detections above `wake.threshold` (default 0.5) open an utterance exactly like an orb click; ambient VAD never fires while the engine is live; a 2 s cooldown plus a post-utterance drain prevents stale-score retriggers |
| provisioning | automatic getter on first use or settings selection — honors `TYCHO_NO_INSTALL`; spawn runs on a worker thread so python import time never stalls the audio loop; failures degrade to VAD, never a crash |
| live config | `wake.model` / `wake.threshold` persist and hot-swap the sidecar without restart; `off` kills the engine and restores VAD |
