# Tycho Automated ONNX Model Pulling & HuggingFace First-Run Bootstrap

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
      [ Query HuggingFace Hub ]        [ Load Local ONNX Weights ]
                 │                                 │
     Stream ONNX Weights to *.tmp                  │
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
2. **Missing Weights Check**: If the required ASR/STT (`whisper-tiny.en.onnx`), TTS (`kokoro-v0_19.onnx`), or voice tokens (`voices.bin`) are absent:
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
| **STT (Whisper)** | `onnx-community/whisper-tiny.en` | `onnx/model.onnx` | `~/.local/share/tycho/models/stt/whisper-tiny.en.onnx` |
| **TTS (Kokoro)** | `onnx-community/Kokoro-82M-ONNX` | `kokoro-v0_19.onnx` | `~/.local/share/tycho/models/tts/kokoro-v0_19.onnx` |
| **TTS (Voices)** | `onnx-community/Kokoro-82M-ONNX` | `voices.bin` | `~/.local/share/tycho/models/tts/voices.bin` |
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
repo_id = "onnx-community/whisper-tiny.en"
revision = "main"
filename = "onnx/model.onnx"
target_filename = "whisper-tiny.en.onnx"
expected_min_bytes = 1048576

[models.tts_model]
repo_id = "onnx-community/Kokoro-82M-ONNX"
revision = "main"
filename = "kokoro-v0_19.onnx"
target_filename = "kokoro-v0_19.onnx"
expected_min_bytes = 1048576

[models.tts_voices]
repo_id = "onnx-community/Kokoro-82M-ONNX"
revision = "main"
filename = "voices.bin"
target_filename = "voices.bin"
expected_min_bytes = 10240
```

---

## 4. CLI Model Management

Users can pre-fetch, check, or force re-download models at any time using the Tycho CLI:

```bash
# Verify models and download if missing
tycho pull-models

# Force clean re-download of all latest ONNX assets from HuggingFace
tycho pull-models --force
```
