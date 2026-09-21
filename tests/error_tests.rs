//! Error type surface tests: Display, source, and From conversions.

use rust_voice_assistant::desktop::DesktopError;
use rust_voice_assistant::Error;
use std::io;
use std::path::PathBuf;

#[test]
fn test_error_display_variants() {
    let cases: Vec<(Error, &str)> = vec![
        (Error::Io(io::Error::other("disk")), "I/O error: disk"),
        (
            Error::Json(serde_json::from_str::<serde_json::Value>("{").unwrap_err()),
            "JSON serialization error:",
        ),
        (
            Error::Toml(toml::from_str::<toml::Value>("= =").unwrap_err()),
            "TOML configuration error:",
        ),
        (
            Error::Desktop(DesktopError::Internal("oops".into())),
            "Internal desktop error: oops",
        ),
        (Error::Audio("adev".into()), "audio error: adev"),
        (
            Error::ModelDownload {
                url: "u".into(),
                message: "m".into(),
            },
            "model download from u failed: m",
        ),
        (
            Error::ModelNotFound(PathBuf::from("/x/y.onnx")),
            "model file missing:",
        ),
        (
            Error::ModelCorrupted {
                path: PathBuf::from("/z"),
                expected_bytes: 10,
                actual_bytes: 3,
            },
            "corrupt model at",
        ),
        (Error::ModelMissing("gone".into()), "model missing: gone"),
        (Error::Routing("r".into()), "command routing error: r"),
        (Error::Transcription("t".into()), "transcription error: t"),
        (Error::Synthesis("s".into()), "speech synthesis error: s"),
        (Error::Inference("i".into()), "model inference error: i"),
        (Error::Config("c".into()), "configuration error: c"),
    ];
    for (err, prefix) in cases {
        assert!(
            err.to_string().starts_with(prefix),
            "unexpected display for {:?}",
            err
        );
    }
}

#[test]
fn test_error_source() {
    let has_source = |e: &Error| std::error::Error::source(e).is_some();
    assert!(has_source(&Error::Io(io::Error::other("x"))));
    assert!(has_source(&Error::Json(
        serde_json::from_str::<serde_json::Value>("{").unwrap_err()
    )));
    assert!(has_source(&Error::Toml(
        toml::from_str::<toml::Value>("= =").unwrap_err()
    )));
    assert!(has_source(&Error::Desktop(DesktopError::Internal(
        "i".into()
    ))));
    assert!(!has_source(&Error::Audio("a".into())));
}

#[test]
fn test_error_from_conversions() {
    let io_err: Error = io::Error::new(io::ErrorKind::NotFound, "nf").into();
    assert!(matches!(io_err, Error::Io(_)));

    let json_err: Error = serde_json::from_str::<serde_json::Value>("{")
        .unwrap_err()
        .into();
    assert!(matches!(json_err, Error::Json(_)));

    let toml_err: Error = toml::from_str::<toml::Value>("= =").unwrap_err().into();
    assert!(matches!(toml_err, Error::Toml(_)));

    let desktop_err: Error = DesktopError::WindowNotFound("w".into()).into();
    assert!(matches!(
        desktop_err,
        Error::Desktop(DesktopError::WindowNotFound(_))
    ));
}

#[test]
fn test_desktop_error_display_variants() {
    assert_eq!(
        DesktopError::ConnectionFailed("c".into()).to_string(),
        "Desktop connection failed: c"
    );
    assert_eq!(
        DesktopError::IpcError("i".into()).to_string(),
        "Desktop IPC error: i"
    );
    assert_eq!(
        DesktopError::UnsupportedOperation("u".into()).to_string(),
        "Unsupported desktop operation: u"
    );
    assert_eq!(
        DesktopError::WindowNotFound("w".into()).to_string(),
        "Window not found: w"
    );
    assert_eq!(
        DesktopError::WorkspaceNotFound(7).to_string(),
        "Workspace not found: 7"
    );
    assert_eq!(
        DesktopError::Internal("x".into()).to_string(),
        "Internal desktop error: x"
    );
}
