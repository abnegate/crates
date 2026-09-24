use crate::config::ConfigError;
use crate::inventory::WEIGHT_EXTENSION;
use crate::train::ARTIFACT_PREFIX;
use crate::train::CLEANUP_TRAINING_RUN_NODE;
use crate::train::ENVIRONMENT_PREFIX;
use crate::train::FOLDER_PREFIX;
use crate::train::INPUT_ENVIRONMENT_PREFIX;
use crate::train::LOAD_TRAIN_DATASET_NODE;
use crate::train::PROBE_LOSS_NODE;
use crate::train::PROBE_PREFIX;
use crate::train::PUBLICATION_DIRECTORY;
use crate::train::SIDECAR_SUFFIX;
use crate::train::STAGE_TRAINING_ARTIFACT_NODE;
use crate::train::TRAIN_LORA_NODE;

/// The names a training run shares with the ComfyUI node pack that executes it
/// and with an external training command. The nodes refuse a run namespace
/// they do not recognise, so these have to match the deployment.
///
/// The prefixes, the sidecar suffix and the publication directory become
/// directory and file names under ComfyUI's directories, so
/// [`Contract::validate`] holds each to one plain path component. The
/// defaults name everything under a neutral `Abnegate` namespace; a deployment
/// whose node pack and training script register other names sets them here.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Contract {
    /// `class_type` of the node that trains the adapter.
    pub train_lora_node: String,
    /// `class_type` of the node that deletes a run's dataset and weights.
    pub cleanup_training_run_node: String,
    /// `class_type` of the node that loads a staged dataset and its manifest.
    pub load_train_dataset_node: String,
    /// `class_type` of the node that measures a model's loss on a dataset.
    pub probe_loss_node: String,
    /// `class_type` of the node that moves a trained checkpoint to where a
    /// LoRA loader finds it.
    pub stage_training_artifact_node: String,
    /// Namespace of the input folder a run stages its dataset in.
    pub folder_prefix: String,
    /// Namespace of the weights a run writes.
    pub artifact_prefix: String,
    /// Namespace of the input folder a quality probe stages its sample in.
    pub probe_prefix: String,
    /// Prefix of the variables handed to
    /// [`Config::train_command`](crate::Config::train_command): the command
    /// reads the dataset from `<prefix>_DIRECTORY` and writes the adapter to
    /// `<prefix>_OUTPUT`. Beside them it gets `COMFYUI_BASE_URL`, the server
    /// in [`Config::base_url`](crate::Config::base_url), and, when
    /// [`Config::api_token`](crate::Config::api_token) is set,
    /// `COMFYUI_API_TOKEN` with `COMFYUI_TOKEN_HEADER`, the token and the
    /// header in [`Config::token_header`](crate::Config::token_header) it
    /// travels in. Any variable this process has under the prefix reaches the
    /// command too, so a deployment keeps its trainer's own settings there.
    pub environment_prefix: String,
    /// Prefix of `<prefix>_INPUT`, the ComfyUI input directory handed to
    /// [`Config::train_command`](crate::Config::train_command).
    pub input_environment_prefix: String,
    /// Suffix of the recipe binding written beside every weight. Model volumes
    /// keep bindings under the name they were written with, so changing it
    /// orphans every one already there.
    pub sidecar_suffix: String,
    /// Directory under `loras/` whose markers hide a weight while it is
    /// replaced. Every process sharing a models directory has to agree on it.
    pub publication_directory: String,
}

impl Default for Contract {
    fn default() -> Self {
        Self {
            train_lora_node: TRAIN_LORA_NODE.to_string(),
            cleanup_training_run_node: CLEANUP_TRAINING_RUN_NODE.to_string(),
            load_train_dataset_node: LOAD_TRAIN_DATASET_NODE.to_string(),
            probe_loss_node: PROBE_LOSS_NODE.to_string(),
            stage_training_artifact_node: STAGE_TRAINING_ARTIFACT_NODE.to_string(),
            folder_prefix: FOLDER_PREFIX.to_string(),
            artifact_prefix: ARTIFACT_PREFIX.to_string(),
            probe_prefix: PROBE_PREFIX.to_string(),
            environment_prefix: ENVIRONMENT_PREFIX.to_string(),
            input_environment_prefix: INPUT_ENVIRONMENT_PREFIX.to_string(),
            sidecar_suffix: SIDECAR_SUFFIX.to_string(),
            publication_directory: PUBLICATION_DIRECTORY.to_string(),
        }
    }
}

impl Contract {
    /// Refuses names that could leave the directory they are joined onto,
    /// namespaces that overlap so one run could match another's files, and
    /// node or variable names ComfyUI or a shell could not carry.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let namespaces = [
            self.folder_prefix.as_str(),
            self.artifact_prefix.as_str(),
            self.probe_prefix.as_str(),
        ];
        if !namespaces
            .iter()
            .all(|prefix| is_name(prefix, is_namespace_character))
        {
            return Err(ConfigError::new(
                "contract prefixes must be ASCII letters, digits, '-' or '_'",
            ));
        }
        for (index, prefix) in namespaces.iter().enumerate() {
            if namespaces[index + 1..]
                .iter()
                .any(|other| prefix.starts_with(other) || other.starts_with(prefix))
            {
                return Err(ConfigError::new(
                    "no contract prefix may begin with another",
                ));
            }
        }
        if ![
            &self.train_lora_node,
            &self.cleanup_training_run_node,
            &self.load_train_dataset_node,
            &self.probe_loss_node,
            &self.stage_training_artifact_node,
        ]
        .iter()
        .all(|node| is_name(node, is_node_character))
        {
            return Err(ConfigError::new(
                "contract node names must be ASCII letters, digits or '_'",
            ));
        }
        if ![&self.environment_prefix, &self.input_environment_prefix]
            .iter()
            .all(|prefix| is_name(prefix, is_variable_character))
        {
            return Err(ConfigError::new(
                "contract environment prefixes must be uppercase ASCII letters, digits or '_'",
            ));
        }
        if !is_name(&self.sidecar_suffix, is_file_character)
            || self.sidecar_suffix.ends_with(WEIGHT_EXTENSION)
        {
            return Err(ConfigError::new(
                "the contract sidecar suffix must be one plain name that no weight ends in",
            ));
        }
        if !is_name(&self.publication_directory, is_file_character)
            || matches!(self.publication_directory.as_str(), "." | "..")
        {
            return Err(ConfigError::new(
                "the contract publication directory must be one plain directory name",
            ));
        }
        Ok(())
    }

    pub(crate) fn variable(&self, name: &str) -> String {
        format!("{}_{name}", self.environment_prefix)
    }

    pub(crate) fn input_variable(&self) -> String {
        format!("{}_INPUT", self.input_environment_prefix)
    }
}

fn is_name(value: &str, allowed: fn(char) -> bool) -> bool {
    !value.is_empty() && value.chars().all(allowed)
}

fn is_namespace_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
}

fn is_node_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn is_variable_character(character: char) -> bool {
    character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
}

fn is_file_character(character: char) -> bool {
    is_namespace_character(character) || character == '.'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_contract_is_valid() {
        assert_eq!(Contract::default().validate(), Ok(()));
    }

    #[test]
    fn a_prefix_that_is_not_one_plain_name_is_refused() {
        for prefix in [
            "",
            "../",
            "../escape-",
            "nested/prefix-",
            "back\\slash-",
            ".",
            "space d-",
        ] {
            for field in [
                |contract: &mut Contract, value: &str| contract.folder_prefix = value.into(),
                |contract: &mut Contract, value: &str| contract.artifact_prefix = value.into(),
                |contract: &mut Contract, value: &str| contract.probe_prefix = value.into(),
            ] {
                let mut contract = Contract::default();
                field(&mut contract, prefix);
                assert!(contract.validate().is_err(), "{prefix:?} was accepted");
            }
        }
    }

    #[test]
    fn a_prefix_that_begins_another_is_refused() {
        let nested = Contract {
            folder_prefix: "run-".into(),
            artifact_prefix: "run-weights-".into(),
            ..Contract::default()
        };
        assert!(nested.validate().is_err());
        let shared = Contract {
            probe_prefix: "same-".into(),
            folder_prefix: "same-".into(),
            ..Contract::default()
        };
        assert!(shared.validate().is_err());
    }

    #[test]
    fn a_node_name_comfyui_could_not_register_is_refused() {
        for node in ["", "Train LoRA", "Train-LoRA", "Train/LoRA"] {
            let contract = Contract {
                probe_loss_node: node.into(),
                ..Contract::default()
            };
            assert!(contract.validate().is_err(), "{node:?} was accepted");
        }
    }

    #[test]
    fn an_environment_prefix_a_shell_could_not_carry_is_refused() {
        for prefix in ["", "lower", "WITH-DASH", "WITH SPACE"] {
            let contract = Contract {
                environment_prefix: prefix.into(),
                ..Contract::default()
            };
            assert!(contract.validate().is_err(), "{prefix:?} was accepted");
            let contract = Contract {
                input_environment_prefix: prefix.into(),
                ..Contract::default()
            };
            assert!(contract.validate().is_err(), "{prefix:?} was accepted");
        }
    }

    #[test]
    fn a_sidecar_suffix_or_publication_directory_that_is_not_one_plain_name_is_refused() {
        for name in [
            "",
            "/",
            "../escape",
            "nested/name",
            "back\\slash",
            "space d",
        ] {
            let suffix = Contract {
                sidecar_suffix: name.into(),
                ..Contract::default()
            };
            assert!(suffix.validate().is_err(), "suffix {name:?} was accepted");
            let directory = Contract {
                publication_directory: name.into(),
                ..Contract::default()
            };
            assert!(
                directory.validate().is_err(),
                "directory {name:?} was accepted"
            );
        }
        for name in [".", ".."] {
            let directory = Contract {
                publication_directory: name.into(),
                ..Contract::default()
            };
            assert!(
                directory.validate().is_err(),
                "directory {name:?} was accepted"
            );
        }
    }

    #[test]
    fn a_sidecar_suffix_a_weight_could_end_in_is_refused() {
        let contract = Contract {
            sidecar_suffix: ".binding.safetensors".into(),
            ..Contract::default()
        };
        assert!(contract.validate().is_err());
    }
}
