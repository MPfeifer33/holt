use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentAppearance {
    pub accent_color: Option<String>,
    pub glow_color: Option<String>,
    pub status_message: Option<String>,
    pub avatar: Option<String>,
    pub display_name: Option<String>,
    #[serde(default)]
    pub pinboard_notes: Vec<PinboardNote>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinboardNote {
    pub text: String,
    pub color: Option<String>,
}

impl AgentAppearance {
    /// Merge another appearance into this one. Only overwrites fields that are Some in `other`.
    pub fn merge(&mut self, other: &AgentAppearance) {
        if other.accent_color.is_some() {
            self.accent_color = other.accent_color.clone();
        }
        if other.glow_color.is_some() {
            self.glow_color = other.glow_color.clone();
        }
        if other.status_message.is_some() {
            self.status_message = other.status_message.clone();
        }
        if other.avatar.is_some() {
            self.avatar = other.avatar.clone();
        }
        if other.display_name.is_some() {
            self.display_name = other.display_name.clone();
        }
        if !other.pinboard_notes.is_empty() {
            self.pinboard_notes = other.pinboard_notes.clone();
        }
    }
}
