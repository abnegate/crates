//! The models each agent is offered with, and the reasoning efforts each
//! one takes.

use abnegate_llm::ReasoningEffort;

/// The word a host uses for letting the model be chosen. An agent would send
/// it to its API as a model name, and the turn would fail there.
const AUTO: &str = "auto";

/// Separates a local model's tag, as in `gpt-oss:20b`. No agent model has one.
const TAG_SEPARATOR: char = ':';

pub(super) const CLAUDE_MODELS: &[&str] = &["sonnet", "opus", "haiku", "fable"];

/// Claude's offered models that run on usage credits beyond the plan's
/// allowance, so run one only when a person names it.
pub(super) const CLAUDE_NAMED_ONLY: &[&str] = &["fable"];

/// Claude's names for its latest models, in any case. `default` is left out:
/// it means what passing no `--model` means.
const CLAUDE_ALIASES: &[&str] = &[
    "sonnet",
    "opus",
    "haiku",
    "fable",
    "best",
    "sonnet[1m]",
    "opus[1m]",
    "fable[1m]",
    "opusplan",
];
const CLAUDE_FAMILY: &str = "claude-";
const LONG_CONTEXT: &str = "[1m]";

pub(super) const CODEX_MODELS: &[&str] = &[
    "gpt-6-astra",
    "gpt-6-sol",
    "gpt-6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
];
const CODEX_FAMILY: &str = "gpt-";

const UP_TO_XHIGH: &[ReasoningEffort] = &[
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::XHigh,
];
const UP_TO_MAX: &[ReasoningEffort] = &[
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::XHigh,
    ReasoningEffort::Max,
];
const UP_TO_ULTRA: &[ReasoningEffort] = &[
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::XHigh,
    ReasoningEffort::Max,
    ReasoningEffort::Ultra,
];

/// What Claude's `--effort` takes on every model its aliases name but haiku,
/// and so on the default model a run with none runs. It warns about any
/// other level and runs at its default.
pub(super) const CLAUDE_EFFORTS: &[ReasoningEffort] = UP_TO_MAX;

/// Claude drops `--effort` for its haiku models without a word, so none is
/// offered for them.
const CLAUDE_EFFORTLESS_FAMILY: &str = "haiku";

/// What each offered Codex model takes. Codex checks none of them itself and
/// sends whatever it is given to its API.
pub(super) const CODEX_EFFORTS: &[(&str, &[ReasoningEffort])] = &[
    ("gpt-6-astra", UP_TO_ULTRA),
    ("gpt-6-sol", UP_TO_ULTRA),
    ("gpt-6-luna", UP_TO_MAX),
    ("gpt-5.6-sol", UP_TO_ULTRA),
    ("gpt-5.6-terra", UP_TO_ULTRA),
    ("gpt-5.6-luna", UP_TO_MAX),
    ("gpt-5.5", UP_TO_XHIGH),
];

/// What every offered Codex model takes, for a run whose model Codex picks.
pub(super) const CODEX_SHARED_EFFORTS: &[ReasoningEffort] = UP_TO_XHIGH;

/// Whether `model`, trimmed, could name a model at all: not empty, not the
/// host's own [`AUTO`], not a local model's tagged name, and without spaces.
pub(super) fn plausible(model: &str) -> bool {
    !model.is_empty()
        && !model.eq_ignore_ascii_case(AUTO)
        && !model.contains(TAG_SEPARATOR)
        && !model.contains(char::is_whitespace)
}

/// One of Claude's aliases, in any case.
pub(super) fn claude_alias(model: &str) -> bool {
    CLAUDE_ALIASES.contains(&model.to_ascii_lowercase().as_str())
}

/// One of Claude's aliases, all of whose models take [`CLAUDE_EFFORTS`] but
/// haiku's.
pub(super) fn claude_effortful(model: &str) -> bool {
    claude_alias(model)
        && !model
            .to_ascii_lowercase()
            .contains(CLAUDE_EFFORTLESS_FAMILY)
}

/// A full Claude model name: `^claude(-[a-z0-9]+)+(\[1m\])?$`.
pub(super) fn claude_model(model: &str) -> bool {
    model
        .strip_suffix(LONG_CONTEXT)
        .unwrap_or(model)
        .strip_prefix(CLAUDE_FAMILY)
        .is_some_and(|rest| {
            rest.split('-')
                .all(|word| !word.is_empty() && word.bytes().all(lowercase_or_digit))
        })
}

/// A model of Codex's family: `^gpt-[a-z0-9][a-z0-9.-]*$`.
pub(super) fn codex_model(model: &str) -> bool {
    let Some(rest) = model.strip_prefix(CODEX_FAMILY) else {
        return false;
    };
    let mut bytes = rest.bytes();

    bytes.next().is_some_and(lowercase_or_digit)
        && bytes.all(|byte| lowercase_or_digit(byte) || byte == b'.' || byte == b'-')
}

/// What `model` takes on Codex, or nothing for a model not offered.
pub(super) fn codex_efforts(model: &str) -> &'static [ReasoningEffort] {
    CODEX_EFFORTS
        .iter()
        .find(|(name, _)| *name == model)
        .map_or(&[], |(_, efforts)| efforts)
}

/// `word` with its first character upper case.
pub(super) fn capitalised(word: &str) -> String {
    let mut characters = word.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

fn lowercase_or_digit(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use abnegate_llm::ReasoningEffort;
    use serde_json::Value;

    use super::CLAUDE_MODELS;
    use super::CODEX_EFFORTS;
    use super::CODEX_MODELS;
    use crate::kind::AgentKind;

    #[test]
    fn no_agent_knows_a_local_model_the_hosts_own_auto_or_a_name_with_spaces() {
        for agent in AgentKind::ALL {
            for model in [
                "gpt-oss:20b",
                "llama3.2:3b",
                "qwen2.5:7b-instruct",
                "",
                "   ",
                "auto",
                "AUTO",
                "Auto",
                "gpt-6 sol",
                "claude-opus 5",
                "son net",
                "opus\tplan",
            ] {
                assert!(!agent.knows(model), "{agent} claims to know {model:?}");
            }
        }
    }

    #[test]
    fn every_model_an_agent_offers_is_one_it_knows() {
        for agent in AgentKind::ALL {
            assert!(!agent.models().is_empty(), "{agent} offers no models");
            for model in agent.models() {
                assert!(
                    agent.knows(model),
                    "{agent} offers {model} without knowing it"
                );
            }
        }
    }

    #[test]
    fn each_agent_offers_its_models_in_the_order_it_ranks_them() {
        assert_eq!(
            AgentKind::Claude.models(),
            ["sonnet", "opus", "haiku", "fable"]
        );
        assert_eq!(
            AgentKind::Codex.models(),
            [
                "gpt-6-astra",
                "gpt-6-sol",
                "gpt-6-luna",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
            ]
        );
    }

    #[test]
    fn only_claudes_fable_runs_only_when_named_and_is_still_offered() {
        assert_eq!(AgentKind::Claude.named_only(), ["fable"]);
        assert!(AgentKind::Codex.named_only().is_empty());

        for agent in AgentKind::ALL {
            for model in agent.named_only() {
                assert!(
                    agent.models().contains(model),
                    "{agent} runs {model} only when named without offering it"
                );
            }
        }
    }

    #[test]
    fn claude_knows_its_aliases_in_any_case_and_its_full_model_names() {
        for model in [
            "sonnet",
            "opus",
            "haiku",
            "fable",
            "best",
            "sonnet[1m]",
            "opus[1m]",
            "fable[1m]",
            "opusplan",
            "Opus",
            "SONNET",
            "Opus[1M]",
            " haiku ",
            "claude-opus-4-8",
            "claude-sonnet-4-6[1m]",
            "claude-haiku-4-5-20251001",
            "claude-opus-6",
        ] {
            assert!(AgentKind::Claude.knows(model), "{model:?}");
        }

        for model in [
            "default",
            "Default",
            "Claude-Opus-5",
            "claude-",
            "claude--",
            "claude-opus-",
            "claude--opus",
            "claude-opus--5",
            "claude-[1m]",
            "claude-opus-5[2m]",
            "claude_opus",
            "us.anthropic.claude-opus-5",
            "claude-3-5-haiku-20241022-v1:0",
            "opus[1m][1m]",
            "gpt-6-sol",
            "llama3.2",
        ] {
            assert!(!AgentKind::Claude.knows(model), "{model:?}");
        }
    }

    #[test]
    fn codex_knows_its_presets_and_the_gpt_family() {
        for model in [
            "gpt-6-astra",
            "gpt-6-sol",
            "gpt-5.6-terra",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-7",
            " gpt-6-sol ",
        ] {
            assert!(AgentKind::Codex.knows(model), "{model:?}");
        }

        for model in [
            "o3",
            "o4-mini",
            "openai/gpt-oss-20b",
            "codex-auto-review",
            "GPT-6-Sol",
            "gpt-",
            "gpt-.5",
            "opus",
            "claude-opus-4-8",
        ] {
            assert!(!AgentKind::Codex.knows(model), "{model:?}");
        }
    }

    #[test]
    fn each_agent_offers_the_efforts_its_models_take() {
        use ReasoningEffort::High;
        use ReasoningEffort::Low;
        use ReasoningEffort::Max;
        use ReasoningEffort::Medium;
        use ReasoningEffort::Ultra;
        use ReasoningEffort::XHigh;

        for model in [
            None,
            Some(""),
            Some("  "),
            Some("sonnet"),
            Some("opus"),
            Some("fable"),
            Some("Opus"),
            Some("sonnet[1m]"),
            Some("opus[1m]"),
            Some("fable[1m]"),
            Some("best"),
            Some("opusplan"),
        ] {
            assert_eq!(
                AgentKind::Claude.efforts(model),
                [Low, Medium, High, XHigh, Max],
                "{model:?}"
            );
        }
        for model in [
            "haiku",
            "Haiku",
            "claude-haiku-4-5",
            "claude-opus-4-8",
            "claude-sonnet-5",
            "claude-opus-4-8[1m]",
            "gpt-7",
        ] {
            assert!(AgentKind::Claude.efforts(Some(model)).is_empty(), "{model}");
        }
        for model in ["gpt-6-astra", "gpt-6-sol", "gpt-5.6-sol", "gpt-5.6-terra"] {
            assert_eq!(
                AgentKind::Codex.efforts(Some(model)),
                [Low, Medium, High, XHigh, Max, Ultra],
                "{model}"
            );
        }
        for model in ["gpt-6-luna", "gpt-5.6-luna", " gpt-6-luna "] {
            assert_eq!(
                AgentKind::Codex.efforts(Some(model)),
                [Low, Medium, High, XHigh, Max],
                "{model:?}"
            );
        }
        for model in [Some("gpt-5.5"), None, Some(" ")] {
            assert_eq!(
                AgentKind::Codex.efforts(model),
                [Low, Medium, High, XHigh],
                "{model:?}"
            );
        }
        for model in ["gpt-7", "gpt-4o", "opus", "auto", "qwen2.5:7b-instruct"] {
            assert!(AgentKind::Codex.efforts(Some(model)).is_empty(), "{model}");
        }
    }

    #[test]
    fn claudes_aliases_are_labelled_capitalised_and_every_other_name_as_spelled() {
        for (agent, model, label) in [
            (AgentKind::Claude, "sonnet", "Sonnet"),
            (AgentKind::Claude, "opus", "Opus"),
            (AgentKind::Claude, "haiku", "Haiku"),
            (AgentKind::Claude, "fable", "Fable"),
            (AgentKind::Claude, "OPUS", "Opus"),
            (AgentKind::Claude, " opus ", "Opus"),
            (AgentKind::Claude, "opusplan", "Opusplan"),
            (AgentKind::Claude, "sonnet[1m]", "Sonnet[1m]"),
            (AgentKind::Claude, "claude-opus-4-8", "claude-opus-4-8"),
            (AgentKind::Codex, "gpt-6-sol", "gpt-6-sol"),
            (AgentKind::Codex, "gpt-5.5", "gpt-5.5"),
            (AgentKind::Codex, "opus", "opus"),
        ] {
            assert_eq!(agent.label(model), label, "{agent}: {model}");
        }
    }

    #[test]
    fn every_offered_codex_model_has_its_efforts_listed() {
        let listed: Vec<&str> = CODEX_EFFORTS.iter().map(|(model, _)| *model).collect();
        assert_eq!(listed, CODEX_MODELS);
    }

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn levels(value: &Value) -> Vec<ReasoningEffort> {
        value
            .as_array()
            .expect("a list of efforts")
            .iter()
            .map(|level| {
                let level = level.as_str().expect("an effort's name");
                level
                    .parse::<ReasoningEffort>()
                    .unwrap_or_else(|error| panic!("an effort this crate does not know: {error}"))
            })
            .collect()
    }

    fn listed<'fixture>(
        fixture: &'fixture Value,
        name: &str,
    ) -> Vec<(&'fixture str, Vec<ReasoningEffort>)> {
        fixture["models"]
            .as_array()
            .expect("the agent's models")
            .iter()
            .map(|model| {
                (
                    model[name].as_str().expect("a model's name"),
                    levels(&model["efforts"]),
                )
            })
            .collect()
    }

    #[test]
    fn claudes_efforts_are_the_ones_its_cli_lists() {
        let fixture = fixture("claude/efforts.json");
        let listed = listed(&fixture, "alias");

        let aliases: Vec<&str> = listed.iter().map(|(alias, _)| *alias).collect();
        assert_eq!(aliases, CLAUDE_MODELS);

        for (alias, efforts) in &listed {
            assert_eq!(
                AgentKind::Claude.efforts(Some(alias)),
                efforts.as_slice(),
                "{alias}"
            );
        }

        for default in ["sonnet", "opus"] {
            let (_, efforts) = listed
                .iter()
                .find(|(alias, _)| *alias == default)
                .expect("a default model");
            assert_eq!(
                AgentKind::Claude.efforts(None),
                efforts.as_slice(),
                "{default}"
            );
        }
    }

    #[test]
    fn codex_offers_the_models_and_efforts_its_cli_lists() {
        let fixture = fixture("codex/models.json");
        let listed = listed(&fixture, "slug");

        let offered: BTreeSet<&str> = CODEX_MODELS.iter().copied().collect();
        let slugs: BTreeSet<&str> = listed.iter().map(|(slug, _)| *slug).collect();
        assert_eq!(offered, slugs);

        for (slug, efforts) in &listed {
            assert_eq!(
                AgentKind::Codex.efforts(Some(slug)),
                efforts.as_slice(),
                "{slug}"
            );
        }

        let shared: Vec<ReasoningEffort> = ReasoningEffort::ALL
            .iter()
            .copied()
            .filter(|effort| listed.iter().all(|(_, efforts)| efforts.contains(effort)))
            .collect();
        assert_eq!(AgentKind::Codex.efforts(None), shared);
    }
}
