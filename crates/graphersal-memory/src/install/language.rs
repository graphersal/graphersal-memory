//! The memory language: the rule that says in which language the agent stores and searches
//! memories. It is chosen at install (`config.json` `memory.language`) and goes into the `use`
//! server's MCP instructions (`crate::runtime::mcp`) and into the managed rule block
//! (`crate::install`). English is the default: one language makes the search find a memory
//! whatever language the conversation was in (the embedding models match words of one language
//! best); `any` keeps the user's own words, and the multilingual model is recommended with it.

/// The memory language rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Language {
    /// Store and search in English (the default).
    #[default]
    English,
    /// Store in the language the user uses; search in it and in English.
    Any,
}

impl Language {
    /// Parses the settings value (`english`, `any`).
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "english" => Some(Self::English),
            "any" => Some(Self::Any),
            _ => None,
        }
    }

    /// The settings value.
    pub fn name(self) -> &'static str {
        match self {
            Self::English => "english",
            Self::Any => "any",
        }
    }

    /// The rule as the agent reads it (one sentence or two).
    pub fn rule(self) -> &'static str {
        match self {
            Self::English => {
                "Store and search memories in English. Translate a user's words into English \
                 before saving them or searching; quote exact wording only when the exact words \
                 matter."
            }
            Self::Any => {
                "Store memories in the language the user uses; search in that language and in \
                 English."
            }
        }
    }

    /// What the choice means, for `status` and the wizard.
    pub fn describe(self) -> &'static str {
        match self {
            Self::English => "English only (stored and searched in English)",
            Self::Any => "any language (stored in the language of the conversation)",
        }
    }
}

/// The placeholder line of `mcp/use.md` the rule replaces.
pub const PLACEHOLDER: &str = "{language_rule}";

/// The `use` instructions with the language rule filled in.
pub fn instructions(template: &str, language: Language) -> String {
    template.replace(PLACEHOLDER, language.rule())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_rules_differ() {
        for language in [Language::English, Language::Any] {
            assert_eq!(Language::parse(language.name()), Some(language));
        }
        assert_eq!(Language::parse("German"), None);
        assert_eq!(Language::default(), Language::English);
        assert!(Language::English.rule().contains("in English"));
        assert!(Language::Any.rule().contains("language the user uses"));
        let text = instructions("a\n- {language_rule}\nb", Language::Any);
        assert!(text.contains("- Store memories in the language the user uses"));
        assert!(!text.contains(PLACEHOLDER));
    }
}
