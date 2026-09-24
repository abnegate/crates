use crate::lora::TrainError;
use crate::train::Contract;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Run {
    pub folder: String,
    pub artifact: String,
}

impl Run {
    /// A fresh pair of names under `contract`'s namespaces.
    pub fn new(contract: &Contract) -> Self {
        Self {
            folder: format!("{}{}", contract.folder_prefix, Uuid::new_v4()),
            artifact: format!("{}{}", contract.artifact_prefix, Uuid::new_v4()),
        }
    }

    /// Refuses names that are not a v4 UUID under `contract`'s namespaces,
    /// and any name at all under a contract that fails [`Contract::validate`].
    pub fn validate(&self, contract: &Contract) -> Result<(), TrainError> {
        contract.validate()?;
        validate_run_name(&self.folder, &contract.folder_prefix)?;
        validate_run_name(&self.artifact, &contract.artifact_prefix)
    }
}

impl Default for Run {
    fn default() -> Self {
        Self::new(&Contract::default())
    }
}

fn validate_run_name(name: &str, prefix: &str) -> Result<(), TrainError> {
    let Some(id) = name.strip_prefix(prefix) else {
        return Err(TrainError::Invalid("invalid training run namespace"));
    };
    let uuid = Uuid::parse_str(id).map_err(|_| TrainError::Invalid("invalid training run UUID"))?;
    if uuid.get_version_num() != 4 || uuid.to_string() != id {
        return Err(TrainError::Invalid("invalid training run UUID"));
    }
    Ok(())
}
