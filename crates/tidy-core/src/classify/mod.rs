//! Classifier (SPEC §5.2): rules → AI → ask the user, plus the learner
//! that turns repeated user decisions into rule suggestions.

pub mod ai;
pub mod learner;
pub mod rules;

pub use ai::{AiCall, AiClassifier, AiSuggestion};
pub use rules::{Classification, FileFacts, Rule, RuleError, RuleSet};
