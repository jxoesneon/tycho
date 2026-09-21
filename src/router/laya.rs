//! Lightweight deterministic intent classifier — the middle tier
//! between the exact grammar and the neural router. Each intent is a
//! set of concept groups (synonym sets); a candidate scores when every
//! required group has at least one hit. This catches the near-miss
//! phrasings the strict grammar rejects ("turn the volume up",
//! "get rid of the browser") without an LLM round-trip.

#[derive(Debug, Clone, PartialEq)]
pub struct LayaDecision {
    pub best_option: String,
    pub confidence: f32,
}

pub struct LayaDecisionModel {
    pub hf_repo: String,
}

/// True when any `words` entry matches — phrases match by substring,
/// single words match whole tokens (so "unmute" does not hit "mute").
fn hits(text: &str, tokens: &[&str], words: &[&str]) -> bool {
    words.iter().any(|w| {
        if w.contains(' ') {
            text.contains(w)
        } else {
            tokens
                .iter()
                .any(|t| t.trim_matches(|c: char| !c.is_alphanumeric()) == *w)
        }
    })
}

/// The last numeric token wins — homophones like "to"/"for" appear
/// early as prepositions ("move me TO desk three"), so a trailing
/// number is far more likely to be the operand.
fn find_number(tokens: &[&str]) -> Option<u32> {
    tokens.iter().rev().find_map(|t| {
        let w = t.trim_matches(|c: char| !c.is_alphanumeric());
        w.parse::<u32>().ok().or_else(|| number_word(w))
    })
}

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
        "eleven" => 11,
        "twelve" => 12,
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

/// Object noun left after dropping the verb and articles — used as the
/// intent parameter when the strict grammar did not fire.
fn object_phrase(tokens: &[&str], verbs: &[&str]) -> Option<String> {
    let words: Vec<&str> = tokens
        .iter()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| {
            !w.is_empty()
                && !verbs.contains(w)
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
                        | "get"
                        | "rid"
                        | "window"
                        | "app"
                        | "application"
                        | "tab"
                        | "up"
                        | "down"
                )
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Scores the query against every intent's concept groups. Returns the
/// winning `(intent, confidence)` plus the extracted object phrase, or
/// `None` when nothing clears the coverage bar.
fn classify(text: &str, tokens: &[&str]) -> Option<(String, f32, Option<String>)> {
    const ACT: &[&str] = &[
        "switch", "swap", "go", "change", "jump", "move", "close", "kill", "quit", "dismiss",
        "shut", "focus", "bring", "show", "open", "launch", "run", "start", "stop", "cancel",
        "repeat", "mute", "unmute", "set", "turn", "make", "raise", "lower", "increase",
        "decrease", "reduce", "boost",
    ];
    let mut out: Vec<(String, f32, Option<String>)> = Vec::new();
    let num = find_number(tokens);

    // Workspace: "workspace three", "move me to desk 2", "jump to space four".
    let num_bounded = num.filter(|n| (1..=10).contains(n));
    if hits(text, tokens, &["workspace", "work space"])
        || (num_bounded.is_some()
            && hits(text, tokens, &["desk", "space", "desktop"])
            && hits(
                text,
                tokens,
                &["switch", "swap", "go", "change", "jump", "move"],
            ))
    {
        if let Some(n) = num_bounded {
            let verb = hits(
                text,
                tokens,
                &["switch", "swap", "go", "change", "jump", "move"],
            );
            out.push((
                format!("switch_workspace_{n}"),
                if verb { 0.9 } else { 0.82 },
                Some(n.to_string()),
            ));
        }
    }

    // Assistant control.
    if hits(text, tokens, &["unmute"]) {
        out.push(("unmute_audio".into(), 0.95, None));
    } else if hits(text, tokens, &["mute"]) {
        out.push(("mute_audio".into(), 0.9, None));
    }
    if hits(text, tokens, &["stop", "cancel", "shut"])
        && hits(
            text,
            tokens,
            &[
                "talking",
                "speaking",
                "listening",
                "it",
                "that",
                "now",
                "up",
            ],
        )
    {
        out.push(("stop_assistant".into(), 0.88, None));
    }
    if hits(
        text,
        tokens,
        &["repeat", "again", "say that again", "what did you say"],
    ) {
        out.push(("repeat_response".into(), 0.85, None));
    }

    // Fullscreen.
    if hits(
        text,
        tokens,
        &["fullscreen", "full screen", "maximize", "maximise"],
    ) {
        out.push(("toggle_fullscreen".into(), 0.9, None));
    }

    // Volume — direction words beat the generic set.
    let volumeish = hits(text, tokens, &["volume", "sound", "audio", "speaker"]);
    let louder = hits(
        text,
        tokens,
        &["louder", "raise", "increase", "boost", "higher", "up"],
    );
    let quieter = hits(
        text,
        tokens,
        &["quieter", "lower", "decrease", "reduce", "softer", "down"],
    );
    if volumeish || louder || quieter {
        if louder && !quieter {
            let conf = if volumeish { 0.9 } else { 0.85 };
            out.push(("volume_up".into(), conf, num.map(|n| n.to_string())));
        } else if quieter && !louder {
            let conf = if volumeish { 0.9 } else { 0.85 };
            out.push(("volume_down".into(), conf, num.map(|n| n.to_string())));
        } else if volumeish {
            if let Some(v) = num {
                out.push(("volume_set".into(), 0.88, Some(v.to_string())));
            }
        }
    }

    // Window management.
    if hits(
        text,
        tokens,
        &["close", "kill", "quit", "dismiss", "shut", "get rid"],
    ) {
        let target = object_phrase(tokens, ACT);
        let has_obj =
            target.is_some() || hits(text, tokens, &["window", "app", "this", "it", "tab"]);
        if has_obj {
            out.push(("close_window".into(), 0.85, target));
        }
    }
    if hits(
        text,
        tokens,
        &["focus", "foreground", "bring up", "switch to", "go to"],
    ) {
        if let Some(target) = object_phrase(tokens, ACT) {
            if !target.contains("workspace") && num_bounded.is_none() {
                out.push(("focus_window".into(), 0.85, Some(target)));
            }
        }
    }
    if hits(
        text,
        tokens,
        &["open", "launch", "run", "fire up", "bring up"],
    ) {
        if let Some(target) = object_phrase(tokens, ACT) {
            let intent = match target.as_str() {
                "terminal" | "console" | "shell" => "launch_terminal",
                "browser" | "web browser" => "launch_browser",
                _ => "launch_app",
            };
            out.push((intent.into(), 0.85, Some(target)));
        }
    }

    out.into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
}

impl LayaDecisionModel {
    pub fn new(hf_repo: impl Into<String>) -> Self {
        Self {
            hf_repo: hf_repo.into(),
        }
    }

    /// Keyword-coverage classification of `query` restricted to
    /// `options`. Returns `None` below the coverage bar so the caller
    /// can fall through to the neural router.
    pub async fn select_option(&self, query: &str, options: &[&str]) -> Option<LayaDecision> {
        let text = query.to_lowercase();
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let (intent, confidence, _) = classify(&text, &tokens)?;
        if !options.contains(&intent.as_str()) {
            return None;
        }
        Some(LayaDecision {
            best_option: intent,
            confidence,
        })
    }

    /// The object phrase `classify` extracted for the winning intent —
    /// lets the router pass a parameter even when the strict grammar
    /// did not parse the phrasing.
    pub async fn extract_parameter(&self, query: &str) -> Option<String> {
        let text = query.to_lowercase();
        let tokens: Vec<&str> = text.split_whitespace().collect();
        classify(&text, &tokens).and_then(|(_, _, p)| p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify_str(q: &str) -> Option<(String, f32, Option<String>)> {
        let text = q.to_lowercase();
        let tokens: Vec<&str> = text.split_whitespace().collect();
        classify(&text, &tokens)
    }

    #[test]
    fn near_miss_phrasings_route() {
        for (q, intent) in [
            ("can you move me to desk three", "switch_workspace_3"),
            ("turn the volume up", "volume_up"),
            ("make it louder please", "volume_up"),
            ("get rid of the browser window", "close_window"),
            ("shut the terminal", "close_window"),
            ("could you open spotify", "launch_app"),
            ("bring up the browser", "launch_browser"),
            ("please unmute the sound", "unmute_audio"),
            ("maximize this", "toggle_fullscreen"),
            ("set the volume to forty", "volume_set"),
            ("stop talking now", "stop_assistant"),
            ("say that again", "repeat_response"),
        ] {
            let (i, c, _) = classify_str(q).unwrap_or_else(|| panic!("no match: {q}"));
            assert_eq!(i, intent, "{q}");
            assert!(c >= 0.8, "{q} -> {c}");
        }
    }

    #[test]
    fn parameters_extract() {
        assert_eq!(
            classify_str("could you open spotify please").unwrap().2,
            Some("spotify".into())
        );
        assert_eq!(
            classify_str("shut the firefox window").unwrap().2,
            Some("firefox".into())
        );
        assert_eq!(
            classify_str("set the volume to forty").unwrap().2,
            Some("40".into())
        );
    }

    #[test]
    fn conversational_queries_stay_unrouted() {
        for q in [
            "what is the capital of france",
            "tell me a joke",
            "how do i bake bread",
            "why is the sky blue",
            "what time is it in tokyo",
        ] {
            assert!(classify_str(q).is_none(), "{q} should not route");
        }
    }

    #[test]
    fn unmute_is_not_mute() {
        let (i, _, _) = classify_str("unmute the speakers").unwrap();
        assert_eq!(i, "unmute_audio");
    }
}
