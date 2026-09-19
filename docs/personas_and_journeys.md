# Tycho Interaction Profiles & User Workflows

This document outlines the user interaction profiles, response verbosity policies, and verification workflows supported by Tycho.

## 1. Operator Profiles
1. **Systems Operator**: Brief, telemetry-focused responses.
2. **Window Manager / Tiling Workflow**: Pure action acknowledgments without conversational filler.
3. **Workspace Architect**: Detailed structural feedback.
4. **General Desktop Workflow**: Balanced interaction.

## 2. Response Policies
- **Sentinel**: Terse, direct status output (max 64 tokens).
- **Scholar**: Structured, analytical breakdown (max 1024 tokens).
- **Consigliere**: Balanced decision support (default, max 256 tokens).
- **Companion**: Conversational tone (max 384 tokens).

## 3. Core Workflows
- **Direct Compositor Dispatch**: Sub-millisecond workspace navigation and window focus via Hyprland IPC or KWin D-Bus.
- **Conversational Queries**: Dialogue generation informed by active window and workspace state.
- **Compositor Auto-Detection**: Dynamic session binding on start (`$HYPRLAND_INSTANCE_SIGNATURE` or `$KDE_FULL_SESSION`).
- **Barge-in Audio Interruption**: Instant cancellation of active TTS audio playback upon voice activity detection.
