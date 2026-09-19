# Tycho: Voice Control Daemon for Linux Compositors

Tycho is a low-latency voice assistant daemon written in Rust, engineered for modern Linux Wayland compositors (Hyprland, KDE Plasma KWin) and standard Linux desktop sessions.

## Overview

- **Compositor Fast-Path**: Sub-millisecond execution for workspace navigation, window focus, layout manipulation, volume changes, and app launching via native IPC.
- **Local Audio Pipeline**: Continuous ring-buffered capture (PipeWire/ALSA), voice activity detection (VAD), and atomic playback sink with sub-millisecond barge-in interruption.
- **Model Cache Lifecycle**: Automatic first-run verification and retrieval of ONNX weights directly from HuggingFace Hub for speech-to-text (Whisper) and text-to-speech (Kokoro).
- **Dialogue & Desktop Awareness**: Conversational engine incorporating active desktop context into prompt frames.
- **Packaging & Sandboxing**: Native distribution configs for systemd user units and sandboxed Flatpak installations.

## Quickstart

```bash
# Verify or pull local ONNX models
tycho pull-models

# Execute command query
tycho query "switch to workspace 3"

# Start the audio listener daemon
tycho run
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
```

## Building

```bash
# Build with all compositor features
cargo build --release --features all-desktops

# Run test suite
cargo test --all-features
```

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
