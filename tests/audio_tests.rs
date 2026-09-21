//! Audio pipeline tests: capture stream, playback sink, and VAD state machine.

use rust_voice_assistant::audio::{
    resample_linear, AudioCaptureStream, AudioPlaybackSink, VadState, VoiceActivityDetector,
};

#[tokio::test]
async fn test_scripted_capture_replay_and_early_exit() {
    // A completed script ends the stream on its own.
    let stream = AudioCaptureStream::new_scripted(vec![vec![0.5f32; 8]; 3]);
    let mut rx = stream.start().unwrap();
    for _ in 0..3 {
        assert_eq!(rx.recv().await.unwrap().len(), 8);
    }
    assert!(rx.recv().await.is_none());

    // Dropping the receiver mid-replay exits the producer on send error.
    let stream = AudioCaptureStream::new_scripted(vec![vec![0.5f32; 8]; 500]);
    let mut rx = stream.start().unwrap();
    let _ = rx.recv().await;
    drop(rx);
    stream.stop();
    tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;
}

#[tokio::test]
async fn test_capture_stream_send_failure_breaks() {
    let stream = AudioCaptureStream::new_scripted(vec![vec![0.5f32; 8]; 500]);
    let rx = stream.start().unwrap();
    drop(rx);
    tokio::time::sleep(tokio::time::Duration::from_millis(60)).await;
    stream.stop();
}

#[tokio::test]
async fn test_capture_unknown_device_errors() {
    // A device name that cannot exist fails honestly on every host.
    let stream = AudioCaptureStream::with_device(128, Some("definitely-no-such-input-xyz".into()));
    let err = stream.start().unwrap_err();
    assert!(err.to_string().contains("definitely-no-such-input-xyz"));
}

#[tokio::test]
async fn test_capture_default_device_probe() {
    // Real device capture where hardware exists; headless hosts report an
    // honest audio error instead of fabricating frames.
    let stream = AudioCaptureStream::new(128);
    match stream.start() {
        Ok(mut rx) => {
            // Frames arrive (or the channel closes cleanly); either way the
            // stream was live on a real device.
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv()).await;
            stream.stop();
        }
        Err(e) => {
            assert!(e.to_string().contains("audio") || e.to_string().contains("device"));
        }
    }
}

#[tokio::test]
async fn test_playback_sink_interrupt_cycle() {
    let sink = AudioPlaybackSink::new_null();
    assert!(!sink.is_interrupted());
    assert!(sink.play_chunk(&[0.1, 0.2]).await.unwrap());

    sink.interrupt();
    assert!(sink.is_interrupted());
    assert!(!sink.play_chunk(&[0.1]).await.unwrap());

    sink.reset();
    assert!(!sink.is_interrupted());
    assert!(sink.play_chunk(&[0.3]).await.unwrap());

    // Empty chunks are accepted without touching the device.
    assert!(sink.play_chunk(&[]).await.unwrap());

    let default_sink = AudioPlaybackSink::default();
    assert!(!default_sink.is_interrupted());
}

#[tokio::test]
async fn test_playback_unknown_device_errors() {
    let sink = AudioPlaybackSink::with_device(Some("definitely-no-such-output-xyz".into()), 16000);
    let err = sink.play_chunk(&[0.1, 0.2]).await.unwrap_err();
    assert!(err.to_string().contains("definitely-no-such-output-xyz"));
}

#[tokio::test]
async fn test_playback_default_device_probe() {
    // Real output where hardware exists; headless hosts get an honest error.
    let sink = AudioPlaybackSink::new();
    match sink.play_chunk(&[0.05; 1600]).await {
        Ok(v) => {
            assert!(v);
            // Live queue: interrupt discards buffered audio and reports
            // interruption; reset clears the flag for the next chunk.
            sink.interrupt();
            assert!(!sink.play_chunk(&[0.1]).await.unwrap());
            sink.reset();
            assert!(sink.play_chunk(&[0.1]).await.unwrap());
        }
        Err(e) => assert!(e.to_string().contains("audio") || e.to_string().contains("device")),
    }
}

#[test]
fn test_resample_linear() {
    // Identity passthrough.
    let src = vec![0.1f32, -0.2, 0.3];
    assert_eq!(resample_linear(&src, 16000, 16000), src);
    // Upsampling doubles length; endpoints are preserved.
    let up = resample_linear(&[0.0f32, 1.0], 1, 2);
    assert_eq!(up.len(), 4);
    assert!((up[0] - 0.0).abs() < 1e-6);
    assert!((up[3] - 1.0).abs() < 1e-6);
    // Downsampling halves length.
    let down = resample_linear(&[0.0f32; 100], 48000, 16000);
    assert_eq!(down.len(), 33);
    // Degenerate inputs stay safe.
    assert!(resample_linear(&[], 16000, 8000).is_empty());
    assert_eq!(resample_linear(&src, 0, 16000), src);
}

#[test]
fn test_vad_calculate_energy() {
    assert_eq!(VoiceActivityDetector::calculate_energy(&[]), 0.0);
    let energy = VoiceActivityDetector::calculate_energy(&[0.5, -0.5, 0.5, -0.5]);
    assert!((energy - 0.5).abs() < f32::EPSILON);
}

#[test]
fn test_vad_full_state_lifecycle() {
    let mut vad = VoiceActivityDetector::new(0.05, 2, 2);
    let silent = vec![0.001f32; 320];
    let loud = vec![0.15f32; 320];

    assert_eq!(vad.process_frame(&silent), VadState::Silence);
    assert_eq!(vad.process_frame(&loud), VadState::Silence);
    assert_eq!(vad.process_frame(&silent), VadState::Silence);
    assert_eq!(vad.process_frame(&loud), VadState::Silence);
    assert_eq!(vad.process_frame(&loud), VadState::SpeechStart);
    assert_eq!(vad.process_frame(&loud), VadState::InSpeech);
    assert_eq!(vad.process_frame(&silent), VadState::InSpeech);
    assert_eq!(vad.process_frame(&loud), VadState::InSpeech);
    assert_eq!(vad.process_frame(&silent), VadState::InSpeech);
    assert_eq!(vad.process_frame(&silent), VadState::SpeechEnd);
    assert_eq!(vad.process_frame(&silent), VadState::Silence);
    assert_eq!(vad.process_frame(&loud), VadState::Silence);
}
