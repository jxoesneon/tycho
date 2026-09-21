# Tycho: Voice Control Daemon for Linux Compositors

Tycho is a low-latency voice assistant daemon written in Rust, engineered for modern Linux Wayland compositors (Hyprland, KDE Plasma KWin) and standard Linux desktop sessions.

## Overview

- **Compositor Fast-Path**: Sub-millisecond execution for workspace navigation, window focus, layout manipulation, volume changes, and app launching via native IPC.
- **Local Audio Pipeline**: Continuous ring-buffered capture (PipeWire/ALSA), voice activity detection (VAD), and atomic playback sink with sub-millisecond barge-in interruption.
- **Model Cache Lifecycle**: Automatic first-run verification and retrieval of model weights directly from HuggingFace Hub — a whisper.cpp ggml model for speech-to-text and a Piper voice for text-to-speech.
- **Dialogue & Desktop Awareness**: Conversational engine incorporating active desktop context into prompt frames.
- **Status Orb**: A persistent floating indicator (wlr layer-shell overlay, auto-placed near a screen corner) that pulses with your voice while listening, spins while thinking, and glows while speaking — CPU-rendered, fully interactive (click to listen, double-click for chat, right-click menu, drag to reposition), no tray host required.
- **Zero-config backend**: On first run, if the default local endpoint has no API key, Tycho installs Ollama — `pacman` or the official install script interactively, otherwise a sudo-free tarball into `~/.local/share/tycho` — starts `ollama serve` detached, and pulls `llama3.2:3b`: install → launch → works. Set `TYCHO_NO_INSTALL=1` to disable automatic installs, or point `[generation] endpoint`/`api_key` (or `TYCHO_ENDPOINT`/`TYCHO_API_KEY`) at any OpenAI-compatible server to manage your own.
- **Packaging & Sandboxing**: Native distribution configs for systemd user units and sandboxed Flatpak installations.

## Quickstart

```bash
# Verify or pull local model weights
tycho pull-models

# Execute command query
tycho query "switch to workspace 3"

# Start the audio listener in the foreground
tycho run

# Or detach it as a background daemon (logs + pid file under ~/.local/state/tycho)
tycho daemon
```

## Configuration

Tycho reads its configuration from `~/.config/tycho/tycho.toml`:

```toml
[audio]
sample_rate = 16000
channels = 1
buffer_size = 512

[vad]
energy_threshold = 0.02
min_speech_duration_ms = 250
min_silence_duration_ms = 600

[models]
cache_dir = "~/.local/share/tycho/models"
auto_download_on_first_run = true
hf_endpoint = "https://huggingface.co"

[desktop]
backend = "auto"
auto_detect = true

[ui]
# Persistent floating status orb (wlr layer-shell overlay). Requires a
# Wayland session; silently disabled when headless or when built without
# the `orb` feature.
orb = true

[generation]
# Any OpenAI-compatible /v1/chat/completions endpoint. With the default
# local endpoint and no api_key, Tycho bootstraps Ollama automatically.
endpoint = "http://127.0.0.1:11434/v1/chat/completions"
model = "llama3.2:3b"
# api_key = "..."
# auto_setup = false   # never install/start/pull Ollama automatically
```

## Building

```bash
# Build with all compositor features
cargo build --release --features all-desktops

# Minimal headless build (no desktop backends, no orb overlay)
cargo build --release --no-default-features

# Run test suite
cargo test --all-features
```

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
