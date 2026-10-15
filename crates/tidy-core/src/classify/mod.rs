//! Classifier (SPEC §5.2). M1 has the rule engine only; the AI step and
//! the learner arrive in M4.

pub mod rules;

pub use rules::{Classification, FileFacts, Rule, RuleError, RuleSet};
