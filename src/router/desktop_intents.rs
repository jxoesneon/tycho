//! Desktop intent taxonomy and canonical dispatch options.

pub const CANONICAL_DESKTOP_OPTIONS: &[&str] = &[
    "switch_workspace_1",
    "switch_workspace_2",
    "switch_workspace_3",
    "switch_workspace_4",
    "switch_workspace_5",
    "focus_window",
    "close_window",
    "toggle_fullscreen",
    "volume_up",
    "volume_down",
    "mute_audio",
    "launch_terminal",
    "launch_browser",
    "general_query",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDesktopCommand {
    pub intent: String,
    pub parameter: Option<String>,
}

impl ParsedDesktopCommand {
    pub fn parse(normalized_text: &str) -> Option<Self> {
        let text = normalized_text.to_lowercase();
        if text.contains("switch to workspace") || text.contains("workspace") {
            for i in 1..=9 {
                if text.contains(&format!("workspace {}", i)) || text.ends_with(&i.to_string()) {
                    return Some(Self {
                        intent: format!("switch_workspace_{}", i),
                        parameter: Some(i.to_string()),
                    });
                }
            }
        }

        if text.contains("focus") {
            let parts: Vec<&str> = text.split_whitespace().collect();
            if let Some(pos) = parts.iter().position(|&w| w == "focus") {
                if pos + 1 < parts.len() {
                    return Some(Self {
                        intent: "focus_window".to_string(),
                        parameter: Some(parts[pos + 1..].join(" ")),
                    });
                }
            }
        }

        if text.contains("close window") || text.contains("close this") {
            return Some(Self {
                intent: "close_window".to_string(),
                parameter: None,
            });
        }

        if text.contains("fullscreen") {
            return Some(Self {
                intent: "toggle_fullscreen".to_string(),
                parameter: None,
            });
        }

        if text.contains("volume up") {
            return Some(Self {
                intent: "volume_up".to_string(),
                parameter: None,
            });
        }

        if text.contains("volume down") {
            return Some(Self {
                intent: "volume_down".to_string(),
                parameter: None,
            });
        }

        if text.contains("launch terminal") || text.contains("open terminal") {
            return Some(Self {
                intent: "launch_terminal".to_string(),
                parameter: Some("alacritty".to_string()),
            });
        }

        None
    }
}
