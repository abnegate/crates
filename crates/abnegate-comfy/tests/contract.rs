use abnegate_comfy::Config;
use abnegate_comfy::inventory;
use abnegate_comfy::inventory::WeightSidecar;
use abnegate_comfy::recipe::RecipeCatalog;
use abnegate_comfy::train::Contract;
use std::fs;

/// A deployment's own node pack, set the only way a caller outside the crate
/// can: field by field on the default.
fn acme() -> Contract {
    let mut contract = Contract::default();
    contract.train_lora_node = "AcmeTrainLoRA".into();
    contract.cleanup_training_run_node = "AcmeCleanupTrainingRun".into();
    contract.load_train_dataset_node = "AcmeLoadTrainDataset".into();
    contract.probe_loss_node = "AcmeProbeLoss".into();
    contract.stage_training_artifact_node = "AcmeStageTrainingArtifact".into();
    contract.folder_prefix = "acme-train-".into();
    contract.artifact_prefix = "acme-lora-".into();
    contract.probe_prefix = "acme-probe-".into();
    contract.environment_prefix = "ACME_TRAIN".into();
    contract.input_environment_prefix = "ACME_COMFY".into();
    contract.sidecar_suffix = ".acme.json".into();
    contract.publication_directory = ".acme-publish".into();
    contract
}

#[test]
fn a_contract_overridden_outside_the_crate_is_a_valid_configuration() {
    let mut config = Config::default();
    config.contract = acme();
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn the_inventory_binds_and_reads_weights_under_the_contract_it_is_given() {
    let models = tempfile::tempdir().unwrap();
    let loras = models.path().join("loras");
    fs::create_dir_all(&loras).unwrap();
    let weight = loras.join("style.safetensors");
    fs::write(&weight, b"lora").unwrap();

    inventory::write_sidecar(
        &weight,
        &WeightSidecar::new("flux-schnell-adapter")
            .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
        &acme(),
    )
    .unwrap();

    assert!(loras.join("style.safetensors.acme.json").is_file());
    let catalog = RecipeCatalog::packaged().unwrap();
    let listed = inventory::scan(models.path(), &catalog, &acme());
    assert_eq!(
        inventory::find(&listed, "style.safetensors").map(|item| item.recipe_id.as_str()),
        Some("flux-schnell-adapter")
    );
    assert!(
        inventory::scan(models.path(), &catalog, &Contract::default()).is_empty(),
        "the default contract reads only its own sidecars"
    );
}
