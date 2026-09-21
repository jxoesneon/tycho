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
    "volume_set",
    "mute_audio",
    "unmute_audio",
    "launch_terminal",
    "launch_browser",
    "launch_app",
    "stop_assistant",
    "repeat_response",
    "general_query",
];

/// Maps spelled-out numbers (as STT emits them, including common
/// homophones) to digits.
fn number_word(word: &str) -> Option<u32> {
    Some(match word {
        "zero" | "oh" => 0,
        "one" | "won" => 1,
        "two" | "to" | "too" => 2,
        "three" => 3,
        "four" | "for" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" | "ate" => 8,
        "nine" => 9,
        "ten" => 10,
        "fifteen" => 15,
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "fifty" => 50,
        "sixty" => 60,
        "seventy" => 70,
        "eighty" => 80,
        "ninety" => 90,
        "hundred" => 100,
        _ => return None,
    })
}

/// Finds a numeric token (digit or spelled-out) in `tokens`. The last
/// match wins: homophone prepositions ("to"=2, "for"=4) appear early
/// in a sentence, while the real operand is almost always trailing
/// ("volume up to thirty").
fn find_number(tokens: &[&str]) -> Option<u32> {
    tokens.iter().rev().find_map(|t| {
        let w = t.trim_matches(|c: char| !c.is_alphanumeric());
        w.parse::<u32>().ok().or_else(|| number_word(w))
    })
}

/// Strips articles/fillers from a spoken object phrase so "close the
/// browser" resolves the target "browser".
fn clean_target(tokens: &[&str]) -> Option<String> {
    let words: Vec<&str> = tokens
        .iter()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| {
            !w.is_empty()
                && !matches!(
                    *w,
                    "the" | "a" | "an" | "my" | "this" | "that" | "it" | "please"
                )
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Like `clean_target` but also drops generic object nouns — for
/// "close firefox window" the real target is "firefox".
fn named_target(tokens: &[&str]) -> Option<String> {
    let words: Vec<&str> = tokens
        .iter()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| {
            !w.is_empty()
                && !matches!(
                    *w,
                    "the"
                        | "a"
                        | "an"
                        | "my"
                        | "this"
                        | "that"
                        | "it"
                        | "please"
                        | "window"
                        | "app"
                        | "application"
                        | "tab"
                        | "program"
                )
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Best-effort parameter extraction for a neural-router pick the
/// strict grammar did not parse — drops command verbs, articles, and
/// domain nouns, returning what remains ("close the big firefox
/// window" → "big firefox").
fn loose_parameter(normalized_text: &str) -> Option<String> {
    let text = normalized_text.to_lowercase();
    let words: Vec<&str> = text
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| {
            !w.is_empty()
                && !matches!(
                    *w,
                    "the"
                        | "a"
                        | "an"
                        | "my"
                        | "this"
                        | "that"
                        | "it"
                        | "please"
                        | "can"
                        | "could"
                        | "you"
                        | "would"
                        | "to"
                        | "for"
                        | "me"
                        | "of"
                        | "off"
                        | "on"
                        | "up"
                        | "down"
                        | "get"
                        | "rid"
                        | "switch"
                        | "swap"
                        | "go"
                        | "change"
                        | "jump"
                        | "move"
                        | "close"
                        | "kill"
                        | "quit"
                        | "dismiss"
                        | "shut"
                        | "focus"
                        | "bring"
                        | "show"
                        | "open"
                        | "launch"
                        | "run"
                        | "start"
                        | "stop"
                        | "cancel"
                        | "repeat"
                        | "mute"
                        | "unmute"
                        | "set"
                        | "turn"
                        | "make"
                        | "window"
                        | "app"
                        | "application"
                        | "tab"
                        | "volume"
                        | "sound"
                        | "audio"
                        | "workspace"
                        | "work"
                        | "space"
                        | "fullscreen"
                        | "full"
                        | "screen"
                )
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Parameter for an intent chosen by a neural router (Laya/Jev) when
/// the strict grammar failed: numbers for volume/workspace intents,
/// object phrases for window/app intents.
pub fn neural_parameter(intent: &str, normalized_text: &str) -> Option<String> {
    match intent {
        i if i.starts_with("switch_workspace_") => i
            .trim_start_matches("switch_workspace_")
            .parse::<u32>()
            .ok()
            .map(|n| n.to_string()),
        "volume_up" | "volume_down" | "volume_set" => {
            let text = normalized_text.to_lowercase();
            let tokens: Vec<&str> = text.split_whitespace().collect();
            find_number(&tokens).map(|n| n.to_string())
        }
        "focus_window" | "close_window" | "launch_app" => loose_parameter(normalized_text),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDesktopCommand {
    pub intent: String,
    pub parameter: Option<String>,
}

impl ParsedDesktopCommand {
    pub fn parse(normalized_text: &str) -> Option<Self> {
        // STT output splits "workspace" and spells out numbers.
        let text = normalized_text
            .to_lowercase()
            .replace("work space", "workspace")
            .replace("work-space", "workspace");
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let trimmed = text.trim().trim_end_matches(['.', '!', '?']);

        // Assistant control — short phrases, checked before desktop verbs.
        if matches!(
            trimmed,
            "stop" | "cancel" | "never mind" | "nevermind" | "shut up" | "be quiet" | "quiet"
        ) || trimmed.contains("stop talking")
            || trimmed.contains("stop speaking")
        {
            return Some(Self {
                intent: "stop_assistant".to_string(),
                parameter: None,
            });
        }
        if matches!(trimmed, "repeat" | "repeat that" | "say again" | "what")
            || trimmed.contains("say that again")
            || trimmed.contains("what did you say")
            || trimmed.contains("repeat what you said")
        {
            return Some(Self {
                intent: "repeat_response".to_string(),
                parameter: None,
            });
        }

        if let Some(pos) = tokens.iter().position(|t| t.contains("workspace")) {
            let inline: String = tokens[pos].chars().filter(|c| c.is_ascii_digit()).collect();
            let n = if inline.is_empty() {
                tokens.get(pos + 1).and_then(|t| {
                    let w = t.trim_matches(|c: char| !c.is_alphanumeric());
                    w.parse::<u32>().ok().or_else(|| number_word(w))
                })
            } else {
                inline.parse::<u32>().ok()
            };
            if let Some(n) = n {
                return Some(Self {
                    intent: format!("switch_workspace_{}", n),
                    parameter: Some(n.to_string()),
                });
            }
        }

        // "switch to firefox" focuses a window — workspace already handled.
        if let Some(rest) = trimmed
            .strip_prefix("switch to ")
            .or_else(|| trimmed.strip_prefix("switch over to "))
            .or_else(|| trimmed.strip_prefix("go to "))
        {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if let Some(target) = clean_target(&toks) {
                if !target.contains("workspace") {
                    return Some(Self {
                        intent: "focus_window".to_string(),
                        parameter: Some(target),
                    });
                }
            }
        }

        if let Some(pos) = tokens.iter().position(|&w| w == "focus") {
            if pos + 1 < tokens.len() {
                if let Some(target) = clean_target(&tokens[pos + 1..]) {
                    return Some(Self {
                        intent: "focus_window".to_string(),
                        parameter: Some(target),
                    });
                }
            }
        }

        if let Some(pos) = tokens.iter().position(|&w| w == "close" || w == "kill") {
            let target = named_target(&tokens[pos + 1..]);
            let has_target_word = tokens.iter().any(|t| {
                matches!(
                    t.trim_matches(|c: char| !c.is_alphanumeric()),
                    "window" | "app" | "this" | "it" | "tab"
                )
            });
            if target.is_some() || has_target_word || tokens.len() <= pos + 1 {
                return Some(Self {
                    intent: "close_window".to_string(),
                    parameter: target,
                });
            }
        }

        if text.contains("fullscreen") || text.contains("full screen") {
            return Some(Self {
                intent: "toggle_fullscreen".to_string(),
                parameter: None,
            });
        }

        // Volume: "unmute" is a distinct state-aware intent, not a toggle.
        if tokens.contains(&"unmute") {
            return Some(Self {
                intent: "unmute_audio".to_string(),
                parameter: None,
            });
        }
        if tokens.contains(&"mute") {
            return Some(Self {
                intent: "mute_audio".to_string(),
                parameter: None,
            });
        }

        let mentions_volume =
            text.contains("volume") || text.contains("louder") || text.contains("quieter");
        if mentions_volume {
            let n = find_number(&tokens);
            if text.contains("louder")
                || text.contains("volume up")
                || text.contains("turn it up")
                || text.contains("turn up")
            {
                return Some(Self {
                    intent: "volume_up".to_string(),
                    parameter: n.map(|v| v.to_string()),
                });
            }
            if text.contains("quieter")
                || text.contains("volume down")
                || text.contains("turn it down")
                || text.contains("turn down")
            {
                return Some(Self {
                    intent: "volume_down".to_string(),
                    parameter: n.map(|v| v.to_string()),
                });
            }
            if let Some(v) = n {
                return Some(Self {
                    intent: "volume_set".to_string(),
                    parameter: Some(v.to_string()),
                });
            }
        }

        // Generic "open X" / "launch X" — the parameter is the program.
        if let Some(pos) = tokens.iter().position(|&w| w == "launch" || w == "open") {
            if let Some(target) = clean_target(&tokens[pos + 1..]) {
                // Bare compositor nouns are not launchable apps —
                // "open workspace"/"open window" are malformed desktop
                // commands, not requests to start a program.
                if matches!(
                    target.as_str(),
                    "workspace" | "window" | "desktop" | "screen" | "monitor" | "fullscreen"
                ) {
                    return None;
                }
                let intent = match target.as_str() {
                    "terminal" | "terminal please" => "launch_terminal",
                    "browser" | "web browser" => "launch_browser",
                    _ => "launch_app",
                };
                return Some(Self {
                    intent: intent.to_string(),
                    parameter: Some(match intent {
                        "launch_terminal" => "alacritty".to_string(),
                        "launch_browser" => "firefox".to_string(),
                        _ => target,
                    }),
                });
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::ParsedDesktopCommand;

    fn parse(q: &str) -> Option<(String, Option<String>)> {
        ParsedDesktopCommand::parse(q).map(|c| (c.intent, c.parameter))
    }

    #[test]
    fn workspace_phrasings() {
        for (q, n) in [
            ("switch to workspace 2", 2),
            ("go to workspace two", 2),
            ("workspace 4", 4),
            ("move to workspace three", 3),
        ] {
            let (i, p) = parse(q).unwrap_or_else(|| panic!("no parse: {q}"));
            assert_eq!(i, format!("switch_workspace_{n}"), "{q}");
            assert_eq!(p.as_deref(), Some(n.to_string().as_str()), "{q}");
        }
    }

    #[test]
    fn assistant_control() {
        for q in ["stop", "cancel", "stop talking", "shut up", "never mind"] {
            assert_eq!(parse(q).unwrap().0, "stop_assistant", "{q}");
        }
        for q in ["repeat", "say that again", "what did you say"] {
            assert_eq!(parse(q).unwrap().0, "repeat_response", "{q}");
        }
    }

    #[test]
    fn unmute_is_state_aware() {
        assert_eq!(parse("unmute").unwrap().0, "unmute_audio");
        assert_eq!(parse("mute").unwrap().0, "mute_audio");
    }

    #[test]
    fn volume_magnitudes() {
        assert_eq!(
            parse("set volume to forty"),
            Some(("volume_set".into(), Some("40".into())))
        );
        assert_eq!(
            parse("volume up by ten"),
            Some(("volume_up".into(), Some("10".into())))
        );
        assert_eq!(parse("volume down"), Some(("volume_down".into(), None)));
        // Homophone guard: "to" must not win over the real operand.
        assert_eq!(
            parse("turn the volume up to thirty"),
            Some(("volume_up".into(), Some("30".into())))
        );
    }

    #[test]
    fn close_and_focus_targets() {
        assert_eq!(
            parse("close firefox"),
            Some(("close_window".into(), Some("firefox".into())))
        );
        assert_eq!(
            parse("close the browser window"),
            Some(("close_window".into(), Some("browser".into())))
        );
        assert_eq!(
            parse("close this window"),
            Some(("close_window".into(), None))
        );
        assert_eq!(
            parse("focus terminal"),
            Some(("focus_window".into(), Some("terminal".into())))
        );
        assert_eq!(
            parse("switch to firefox"),
            Some(("focus_window".into(), Some("firefox".into())))
        );
    }

    #[test]
    fn generic_launch() {
        assert_eq!(
            parse("open spotify"),
            Some(("launch_app".into(), Some("spotify".into())))
        );
        assert_eq!(
            parse("open the browser"),
            Some(("launch_browser".into(), Some("firefox".into())))
        );
        assert_eq!(
            parse("launch terminal"),
            Some(("launch_terminal".into(), Some("alacritty".into())))
        );
    }

    #[test]
    fn non_commands_do_not_parse() {
        for q in [
            "what is the weather",
            "tell me about windows",
            "how are you today",
        ] {
            assert!(parse(q).is_none(), "{q}");
        }
    }

    #[test]
    fn neural_parameter_extraction() {
        assert_eq!(
            super::neural_parameter("close_window", "please close the firefox window"),
            Some("firefox".into())
        );
        assert_eq!(
            super::neural_parameter("volume_set", "set the volume to sixty"),
            Some("60".into())
        );
        assert_eq!(
            super::neural_parameter("switch_workspace_3", "anything"),
            Some("3".into())
        );
        assert_eq!(super::neural_parameter("mute_audio", "mute"), None);
    }
}
