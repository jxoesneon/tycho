//! Short-term conversational context tracking.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationTurn {
    pub speaker: String,
    pub utterance: String,
}

#[derive(Debug, Clone)]
pub struct ConversationHistory {
    turns: VecDeque<ConversationTurn>,
    max_turns: usize,
}

impl ConversationHistory {
    pub fn new(max_turns: usize) -> Self {
        Self {
            turns: VecDeque::with_capacity(max_turns),
            max_turns,
        }
    }

    pub fn push(&mut self, speaker: &str, utterance: &str) {
        if self.turns.len() >= self.max_turns {
            self.turns.pop_front();
        }
        self.turns.push_back(ConversationTurn {
            speaker: speaker.to_string(),
            utterance: utterance.to_string(),
        });
    }

    /// The recorded turns, oldest first — used to thread real
    /// role-tagged messages into chat completion requests.
    pub fn turns(&self) -> impl Iterator<Item = &ConversationTurn> {
        self.turns.iter()
    }

    pub fn formatted_dialogue(&self) -> String {
        self.turns
            .iter()
            .map(|t| format!("{}: {}", t.speaker, t.utterance))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn len(&self) -> usize {
        self.turns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }
}
