use crate::config::ConfigError;
use crate::lora::TrainMethod;
use crate::lora::TrainRule;
use std::path::Component;
use std::path::Path;

/// Which subjects and methods each base trains, where each method's weight is
/// published, and what has to be installed first: a deployment's product
/// rules, kept out of the crate.
///
/// A base no rule names trains the crate's default: [`TrainMethod::Lora`] on
/// [`TrainSubject::Other`](crate::lora::TrainSubject::Other), published in
/// `loras` for the adapter recipe its catalog entry names. A base a rule names
/// trains only the methods its rules give.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TrainPolicy {
    /// The rules, at most one for each base and method.
    pub rules: Vec<TrainRule>,
}

impl TrainPolicy {
    /// A policy with no rules, under which every base trains the default.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `rule`, replacing the rule already given for its base and method.
    pub fn with_rule(mut self, rule: TrainRule) -> Self {
        self.rules
            .retain(|existing| existing.base != rule.base || existing.method != rule.method);
        self.rules.push(rule);
        self
    }

    /// The rules given for the base `base`. None means it trains the default.
    pub fn rules_for<'a>(&'a self, base: &'a str) -> impl Iterator<Item = &'a TrainRule> {
        self.rules.iter().filter(move |rule| rule.base == base)
    }

    /// The rule given for training the base `base` with `method`.
    pub fn rule(&self, base: &str, method: TrainMethod) -> Option<&TrainRule> {
        self.rules
            .iter()
            .find(|rule| rule.base == base && rule.method == method)
    }

    /// Refuses a rule with no subjects, a base or recipe that is blank, padded
    /// or holds a control character, a publication directory that is not one
    /// plain directory name, a suffix that is not a plain run of name
    /// characters, a required file that could leave the models root, and two
    /// rules for the same base and method.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.subjects.is_empty() || !is_name(&rule.base) || !is_name(&rule.recipe_id) {
                return Err(ConfigError::new(
                    "every training rule must name a base, a subject and a recipe",
                ));
            }
            if !is_directory_name(&rule.directory) {
                return Err(ConfigError::new(
                    "a training rule must publish into one plain directory name",
                ));
            }
            if rule
                .suffix
                .as_deref()
                .is_some_and(|suffix| !is_suffix(suffix))
            {
                return Err(ConfigError::new(
                    "a training rule's suffix must be letters, digits, dashes or underscores",
                ));
            }
            if !rule.required.iter().all(|file| is_beneath(file)) {
                return Err(ConfigError::new(
                    "a training rule's required files must be relative to the models directory",
                ));
            }
            if self.rules[index + 1..]
                .iter()
                .any(|other| other.base == rule.base && other.method == rule.method)
            {
                return Err(ConfigError::new(
                    "a base may have only one training rule for each method",
                ));
            }
        }
        Ok(())
    }
}

fn is_name(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && !name.chars().any(char::is_control)
}

fn is_suffix(suffix: &str) -> bool {
    !suffix.is_empty()
        && suffix
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn is_directory_name(name: &str) -> bool {
    is_name(name)
        && !name.contains(['/', '\\'])
        && is_beneath(Path::new(name))
        && Path::new(name).components().count() == 1
}

fn is_beneath(path: &Path) -> bool {
    path.components().next().is_some()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lora::TrainSubject;

    fn suffixed(suffix: &str) -> TrainPolicy {
        TrainPolicy::new().with_rule(
            TrainRule::new("sdxl", TrainMethod::Lora, "sdxl-adapter", "loras")
                .with_subjects([TrainSubject::Person])
                .with_suffix(suffix),
        )
    }

    #[test]
    fn a_suffix_is_a_plain_run_of_name_characters() {
        for accepted in ["-alt", "_v2", "sdxl2"] {
            assert_eq!(suffixed(accepted).validate(), Ok(()), "{accepted:?}");
        }
        for refused in ["", "-alt.safetensors", "/alt", "-a lt", "..", "-ält"] {
            assert!(
                suffixed(refused).validate().is_err(),
                "{refused:?} was accepted"
            );
        }
    }
}
