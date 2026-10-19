//! Classifier (SPEC §5.2): rules → AI → ask the user, plus the learner
//! that turns repeated user decisions into rule suggestions.

pub mod ai;
pub mod ai_rules;
pub mod learner;
pub mod preview;
pub mod rules;
pub mod rules_doc;
pub mod starter;

pub use ai::{AiCall, AiClassifier, AiSuggestion};
pub use rules::{Anchors, Classification, FileFacts, Rule, RuleError, RuleSet};
pub use rules_doc::{append_cards, apply_cards, parse_cards, RuleCard};
