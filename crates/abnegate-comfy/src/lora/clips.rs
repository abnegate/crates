use super::Attempt;
use super::Handoff;
use super::NO_CLIPS;
use super::Plan;
use super::Screening;
use super::TrainError;
use super::TrainImage;
use super::TrainOutcome;
use super::TrainRequest;
use super::blocking;
use super::ensure_child_directory;
use super::host_job;
use super::host_secrets;
use super::train_on_host;
use crate::config::Config;
use crate::dataset;
use crate::train::Phase;
use crate::train::Reporter;

/// Trains the video adapter `plan` settles for `request` on the host trainer.
///
/// Every clip among the request's files is cut into windows the size of one
/// generated clip, the windows go to the trainer in the job's
/// [`CLIPS_DIRECTORY`](crate::host::CLIPS_DIRECTORY), and the adapter it
/// trains is published where the plan says, like any weight the host trains.
/// Files that are not clips are left out.
pub(super) async fn train(
    config: &Config,
    mut request: TrainRequest,
    plan: Plan,
    progress: Reporter,
) -> Result<TrainOutcome, TrainError> {
    let trainer = config.host_trainer.as_ref().ok_or(TrainError::Disabled)?;
    let clips: Vec<TrainImage> = std::mem::take(&mut request.images)
        .into_iter()
        .filter(|image| crate::video::is_clip(&image.filename))
        .collect();
    let count = clips.len();
    let progress = progress.with_total(trainer.video_steps_for(count));
    progress.phase(Phase::Screening, "Staging clip windows", 0, count);
    let attempt = Attempt::create(&config.models_directory)?;
    let staged = ensure_child_directory(&attempt.root, Handoff::Clips.staged())?;
    let mut windows = Vec::new();
    for (index, mut clip) in clips.into_iter().enumerate() {
        let filename = std::mem::take(&mut clip.filename);
        let video = blocking(move || clip.take_encoded()).await??;
        let cut =
            crate::video::window::stage(config, &video, &filename, &staged, windows.len()).await?;
        windows.extend(cut);
        let done = index + 1;
        progress.phase(
            Phase::Screening,
            format!("Staged clip {done} of {count}"),
            done,
            count,
        );
    }
    if windows.is_empty() {
        return Err(TrainError::Invalid(NO_CLIPS));
    }
    let poses: Vec<String> = windows.iter().map(|window| window.pose.clone()).collect();
    let findings = dataset::inspect(&poses, windows.len());
    let progress = progress.with_total(trainer.video_steps_for(windows.len()));
    let job = host_job(
        &request,
        &plan.base,
        &plan.model,
        &plan.trigger,
        &plan.filename,
        &plan.destination,
        windows.len(),
    );
    let path = train_on_host(
        config,
        &attempt,
        Handoff::Clips,
        job,
        &host_secrets(
            &request.host_secrets,
            request.compute,
            request.compute_key.as_ref(),
        ),
        &plan.destination,
        &progress,
    )
    .await?;
    Ok(TrainOutcome {
        path,
        quality: None,
        dataset: findings,
        screening: Screening {
            kept: count,
            dropped: Vec::new(),
            attempted: Vec::new(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::super::FOREIGN_WEIGHT;
    use super::super::HOST_RENAMED;
    use super::super::LORA_ONLY;
    use super::super::METHOD_UNAVAILABLE;
    use super::super::MISSING_COMPUTE_KEY;
    use super::super::SUBJECT_UNAVAILABLE;
    use super::super::UNRUNNABLE_RULE;
    use super::super::VIDEO_HOST_ONLY;
    use super::super::plan;
    use super::super::train_with_pipeline;
    use super::super::trainable_bases;
    use super::super::validate_request;
    use super::super::weight_filename;
    use super::*;
    use crate::host;
    use crate::host::JobDirectory;
    use crate::inventory::WeightDocument;
    use crate::inventory::sidecar_path;
    use crate::lora::TrainMethod;
    use crate::lora::TrainPolicy;
    use crate::lora::TrainRule;
    use crate::lora::TrainSubject;
    use crate::recipe::RecipeCatalog;
    use crate::screening::Verdict;
    use crate::train::TrainProgress;
    use abnegate_secret::SecretValue;
    use std::fs;
    use std::path::Path;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::process::Command;
    use tokio::task::JoinHandle;

    const BASE: &str = "flux-schnell";
    const BASE_WEIGHT: &str = "checkpoints/flux1-schnell-fp8.safetensors";
    const VIDEO_MODEL: &str = "diffusion_models/wan2.2_ti2v_5B_fp16.safetensors";
    const VIDEO_FILES: &[&str] = &[
        VIDEO_MODEL,
        "text_encoders/umt5_xxl_fp8_e4m3fn_scaled.safetensors",
        "vae/wan2.2_vae.safetensors",
    ];
    const INSTALL_FIRST: &str = "install the files this base needs before training on it";

    fn video_rule() -> TrainRule {
        TrainRule::new(BASE, TrainMethod::Video, "wan-adapter", "loras")
            .with_subjects([TrainSubject::Person])
            .with_required(VIDEO_MODEL)
            .with_suffix("-wan")
    }

    fn pivotal_rule() -> TrainRule {
        TrainRule::new(BASE, TrainMethod::Pivotal, "flux-schnell-adapter", "loras")
            .with_subjects([TrainSubject::Person])
    }

    fn install(config: &Config, file: &str) {
        let path = config.models_directory.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"weights").unwrap();
    }

    /// Trains video adapters and pivotal LoRAs of a person on flux-schnell
    /// on the host trainer, with the base and the video model installed.
    fn hosted() -> (tempfile::TempDir, Config) {
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        fs::create_dir(&models).unwrap();
        let config = Config {
            models_directory: models,
            host_trainer: Some(
                host::Trainer::new()
                    .with_poll_interval(Duration::from_millis(5))
                    .with_pickup(Duration::from_secs(10)),
            ),
            train_policy: TrainPolicy::new()
                .with_rule(video_rule())
                .with_rule(pivotal_rule()),
            ..Default::default()
        };
        install(&config, BASE_WEIGHT);
        for file in VIDEO_FILES {
            install(&config, file);
        }
        (root, config)
    }

    fn clip(filename: &str, bytes: &[u8]) -> TrainImage {
        TrainImage::from_bytes(filename, "", bytes.to_vec())
    }

    fn video(name: &str, images: Vec<TrainImage>) -> TrainRequest {
        TrainRequest::new(name, BASE, images)
            .with_trigger("ohwx")
            .with_subject(TrainSubject::Person)
            .with_method(TrainMethod::Video)
    }

    fn walk(name: &str) -> TrainRequest {
        video(name, vec![clip("walk.mp4", b"clip")])
    }

    fn refused(config: &Config, request: &TrainRequest) -> &'static str {
        match validate_request(config, request) {
            Err(TrainError::Invalid(message)) => message,
            other => panic!("expected an invalid request, got {other:?}"),
        }
    }

    fn never_screened(_images: &[Vec<u8>], _side: u32) -> Verdict {
        panic!("a video adapter's clips are not screened as photos")
    }

    fn ffmpeg_installed() -> bool {
        ["ffmpeg", "ffprobe"].iter().all(|tool| {
            std::process::Command::new(tool)
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    }

    async fn synthesize(directory: &Path, seconds: &str) -> Vec<u8> {
        let path = directory.join("walk.mp4");
        let built = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=30",
                "-t",
                seconds,
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&path)
            .status()
            .await
            .unwrap();
        assert!(built.success(), "could not build the test clip");
        fs::read(path).unwrap()
    }

    /// What the host trainer found when it picked a video job up.
    struct PickedUp {
        job: host::Job,
        clips: Vec<String>,
        queued: Option<TrainProgress>,
        secret: Option<String>,
    }

    /// Plays the host trainer: waits for a job, records what it was handed,
    /// then writes `weight` as the job's file and marks it succeeded.
    fn host_trainer(config: &Config, weight: &'static [u8]) -> JoinHandle<PickedUp> {
        let config = config.clone();
        tokio::spawn(async move {
            let mut job = loop {
                if let Some(job) = host::current(&config) {
                    break job;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            let directory = JobDirectory::new(&config, job.id);
            let picked = job.clone();
            let queued = directory.read_progress();
            let mut clips: Vec<String> = fs::read_dir(directory.path().join(host::CLIPS_DIRECTORY))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            clips.sort();
            let secret = host::Compute::Runpod
                .key_file()
                .and_then(|file| fs::read_to_string(directory.path().join(file)).ok());
            job.status = host::Status::Running;
            directory.write_job(&job).unwrap();
            fs::write(
                directory.path().join(job.filename.as_deref().unwrap()),
                weight,
            )
            .unwrap();
            job.status = host::Status::Succeeded;
            directory.write_job(&job).unwrap();
            PickedUp {
                job: picked,
                clips,
                queued,
                secret,
            }
        })
    }

    #[test]
    fn a_video_adapter_takes_the_rules_suffix_so_it_never_replaces_the_still_weight() {
        assert_eq!(
            weight_filename("jerry", Some("-wan")).unwrap(),
            "jerry-wan.safetensors"
        );
        assert_eq!(
            weight_filename("jerry-wan", Some("-wan")).unwrap(),
            "jerry-wan.safetensors"
        );
        assert_eq!(
            weight_filename("jerry-wan.safetensors", Some("-wan")).unwrap(),
            "jerry-wan.safetensors"
        );
        assert_eq!(weight_filename("jerry", None).unwrap(), "jerry.safetensors");
        assert!(weight_filename("../jerry", Some("-wan")).is_err());

        let (_root, config) = hosted();
        assert_eq!(
            plan(&config, &walk("jerry")).unwrap().filename,
            "jerry-wan.safetensors"
        );
        install(&config, "loras/jerry-wan.safetensors");
        assert_eq!(
            refused(&config, &walk("jerry")),
            FOREIGN_WEIGHT,
            "the suffixed name is the one checked for a weight training did not publish"
        );
    }

    #[test]
    fn a_video_adapter_needs_the_host_trainer_the_video_model_and_a_clip() {
        let (_root, mut config) = hosted();
        validate_request(&config, &walk("jerry")).expect("a clip on the host trainer trains");

        assert_eq!(refused(&config, &video("jerry", Vec::new())), NO_CLIPS);
        assert_eq!(
            refused(&config, &video("jerry", vec![clip("portrait.png", b"png")])),
            NO_CLIPS
        );
        validate_request(
            &config,
            &video(
                "jerry",
                vec![clip("portrait.png", b"png"), clip("walk.MOV", b"clip")],
            ),
        )
        .expect("one clip among the files is enough");
        assert_eq!(
            refused(&config, &walk("jerry").with_subject(TrainSubject::Other)),
            SUBJECT_UNAVAILABLE
        );

        let commanded = Config {
            train_command: Some("true".into()),
            host_trainer: None,
            enabled: true,
            ..config.clone()
        };
        assert_eq!(refused(&commanded, &walk("jerry")), VIDEO_HOST_ONLY);

        let unruled = Config {
            train_policy: TrainPolicy::new().with_rule(pivotal_rule()),
            ..config.clone()
        };
        assert_eq!(refused(&unruled, &walk("jerry")), METHOD_UNAVAILABLE);

        config.train_policy = TrainPolicy::new().with_rule(
            TrainRule::new(BASE, TrainMethod::Video, "flux-schnell-adapter", "loras")
                .with_subjects([TrainSubject::Person]),
        );
        assert!(
            matches!(
                validate_request(&config, &walk("jerry")),
                Err(TrainError::Configuration(message)) if message == UNRUNNABLE_RULE
            ),
            "only a video adapter recipe runs a video adapter"
        );
        config.train_policy = TrainPolicy::new().with_rule(video_rule());
        fs::remove_file(config.models_directory.join(VIDEO_MODEL)).unwrap();
        assert_eq!(refused(&config, &walk("jerry")), INSTALL_FIRST);
    }

    #[test]
    fn a_pivotal_lora_trains_like_a_lora_only_where_a_trainer_takes_it() {
        let (_root, config) = hosted();
        let pivotal = |name: &str| {
            TrainRequest::new(name, BASE, vec![clip("portrait.png", b"png")])
                .with_trigger("ohwx")
                .with_subject(TrainSubject::Person)
                .with_method(TrainMethod::Pivotal)
        };
        let planned = plan(&config, &pivotal("jerry")).expect("the host trains a pivotal LoRA");
        assert_eq!(planned.filename, "jerry.safetensors");
        assert_eq!(planned.destination.directory, "loras");
        assert_eq!(planned.destination.recipe_id, "flux-schnell-adapter");

        let graph = Config {
            host_trainer: None,
            enabled: true,
            ..config.clone()
        };
        assert_eq!(refused(&graph, &pivotal("jerry")), LORA_ONLY);
        let commanded = Config {
            train_command: Some("true".into()),
            ..graph
        };
        validate_request(&commanded, &pivotal("jerry"))
            .expect("a training command trains a pivotal LoRA");

        let misplaced = Config {
            train_policy: TrainPolicy::new().with_rule(
                TrainRule::new(BASE, TrainMethod::Pivotal, "flux-schnell", "checkpoints")
                    .with_subjects([TrainSubject::Person]),
            ),
            ..config
        };
        assert!(matches!(
            validate_request(&misplaced, &pivotal("jerry")),
            Err(TrainError::Configuration(message)) if message == UNRUNNABLE_RULE
        ));
    }

    #[test]
    fn a_base_offers_video_and_pivotal_only_where_the_host_trainer_runs_them() {
        let (_root, config) = hosted();
        let offered = |config: &Config| {
            trainable_bases(config)
                .unwrap()
                .into_iter()
                .find(|base| base.id == BASE)
        };
        let base = offered(&config).expect("the host trains both rules");
        assert_eq!(base.subjects, [TrainSubject::Person]);
        assert_eq!(base.methods, [TrainMethod::Video, TrainMethod::Pivotal]);

        let commanded = Config {
            train_command: Some("true".into()),
            host_trainer: None,
            ..config.clone()
        };
        assert_eq!(
            offered(&commanded).unwrap().methods,
            [TrainMethod::Pivotal],
            "a training command never trains a video adapter"
        );

        fs::remove_file(config.models_directory.join(VIDEO_MODEL)).unwrap();
        assert_eq!(offered(&config).unwrap().methods, [TrainMethod::Pivotal]);
    }

    #[test]
    fn a_video_job_finished_while_nobody_waited_is_published_under_its_suffixed_name() {
        let (_root, config) = hosted();
        let job = |name: &str, filename: &str| {
            host::Job::new(name, "video", "flux1-schnell-fp8.safetensors")
                .with_subject("person")
                .with_trigger("ohwx")
                .with_base(BASE)
                .with_filename(filename)
        };
        let finished = |job: host::Job| {
            let mut job = job;
            job.status = host::Status::Succeeded;
            let directory = JobDirectory::new(&config, job.id);
            directory.write_job(&job).unwrap();
            fs::write(
                directory.path().join(job.filename.as_deref().unwrap()),
                b"adapter",
            )
            .unwrap();
        };
        finished(job("jerry", "jerry-wan.safetensors"));
        let mut renamed = job("yvonne", "yvonne.safetensors");
        renamed.started_at += chrono::Duration::seconds(1);
        finished(renamed);

        let recovered = host::recover(&config);

        let published = config.models_directory.join("loras/jerry-wan.safetensors");
        assert_eq!(recovered.len(), 2, "{recovered:?}");
        assert_eq!(recovered[0].as_ref().unwrap(), &published);
        assert!(
            matches!(&recovered[1], Err(TrainError::Invalid(message)) if *message == HOST_RENAMED),
            "{recovered:?}"
        );
        let document: WeightDocument =
            serde_json::from_slice(&fs::read(sidecar_path(&published, &config.contract)).unwrap())
                .unwrap();
        assert_eq!(document.sidecar.recipe_id, "wan-adapter");
        assert_eq!(
            document.sidecar.huggingface_base.as_deref(),
            Some("Comfy-Org/Wan_2.2_ComfyUI_Repackaged")
        );
    }

    #[tokio::test]
    async fn a_video_adapter_trains_on_clip_windows_on_the_host_and_binds_its_trigger() {
        if !ffmpeg_installed() {
            eprintln!("skipping: ffmpeg is not installed");
            return;
        }
        let (root, config) = hosted();
        let bytes = synthesize(root.path(), "6").await;
        let trainer = host_trainer(&config, b"adapter");
        let (reporter, recorded) = Reporter::recording();
        let request = video(
            "jerry",
            vec![
                clip("walk.mp4", &bytes),
                clip("portrait.png", b"not a clip"),
            ],
        )
        .with_host_field("queue", "video");

        let outcome = train_with_pipeline(
            &config,
            String::new(),
            SecretValue::new(""),
            request,
            never_screened,
            false,
            reporter,
        )
        .await
        .expect("the host trainer trains the video adapter");
        let picked = trainer.await.unwrap();

        let job = &picked.job;
        let windows = job.image_count;
        assert!((2..=3).contains(&windows), "{job:?}");
        assert_eq!(
            (
                job.method.as_str(),
                job.subject.as_str(),
                job.base.as_str(),
                job.trigger.as_str(),
                job.recipe_id.as_str(),
                job.huggingface_base.as_str(),
                job.architecture.as_str(),
            ),
            (
                "video",
                "person",
                BASE,
                "ohwx",
                "wan-adapter",
                "Comfy-Org/Wan_2.2_ComfyUI_Repackaged",
                "",
            )
        );
        assert_eq!(job.filename.as_deref(), Some("jerry-wan.safetensors"));
        assert_eq!(job.fields["queue"], "video");
        assert_eq!(job.compute, host::Compute::Local);
        assert!(job.secrets.is_empty(), "{:?}", job.secrets);
        assert!(picked.secret.is_none());
        let mut expected: Vec<String> = (0..windows)
            .flat_map(|index| [format!("{index:04}.json"), format!("{index:04}.mp4")])
            .collect();
        expected.sort();
        assert_eq!(
            picked.clips, expected,
            "every window and its description is in place before the job appears"
        );
        let trainer_settings = config.host_trainer.as_ref().unwrap();
        assert_eq!(
            picked.queued.map(|queued| queued.total),
            Some(trainer_settings.video_steps_for(windows)),
            "the queued job reports the video budget"
        );

        let published = config.models_directory.join("loras/jerry-wan.safetensors");
        assert_eq!(outcome.path, published);
        assert_eq!(fs::read(&published).unwrap(), b"adapter");
        assert!(outcome.quality.is_none());
        assert_eq!(outcome.screening.kept, 1, "one clip among the files");
        let document: WeightDocument =
            serde_json::from_slice(&fs::read(sidecar_path(&published, &config.contract)).unwrap())
                .unwrap();
        assert_eq!(document.sidecar.recipe_id, "wan-adapter");
        assert_eq!(document.sidecar.trigger.as_deref(), Some("ohwx"));
        let directory = JobDirectory::new(&config, job.id);
        assert!(
            !directory.path().join(host::CLIPS_DIRECTORY).exists(),
            "the windows outlived the published job"
        );
        assert_eq!(
            directory.read_job().unwrap().published.as_deref(),
            Some("loras/jerry-wan.safetensors")
        );
        let updates = recorded.lock().unwrap().clone();
        assert!(
            updates
                .iter()
                .any(|update| update.phase == Some(Phase::Screening)
                    && update.message.as_deref() == Some("Staged clip 1 of 1")),
            "{updates:?}"
        );

        let catalog = RecipeCatalog::load(config.workflow_path.as_deref()).unwrap();
        let bound = crate::inventory::video_identity_for_prompt(
            &config.models_directory,
            &catalog,
            &config.contract,
            "ohwx walking on the beach",
        )
        .expect("the trained trigger names the video adapter");
        assert_eq!(bound.lora.as_deref(), Some("jerry-wan.safetensors"));
    }

    #[tokio::test]
    async fn a_remote_video_adapter_queues_a_host_job_carrying_its_compute_and_key() {
        let (root, config) = hosted();
        let remote = |request: TrainRequest| request.with_compute(host::Compute::Runpod);
        assert_eq!(
            refused(&config, &remote(walk("jerry"))),
            MISSING_COMPUTE_KEY,
            "a remote video adapter is checked as a host job"
        );
        fs::remove_file(config.models_directory.join(VIDEO_MODEL)).unwrap();
        validate_request(
            &config,
            &remote(walk("jerry")).with_compute_key(SecretValue::new("rp-test-key")),
        )
        .expect("the pod pulls the video model itself");
        if !ffmpeg_installed() {
            eprintln!("skipping: ffmpeg is not installed");
            return;
        }
        let bytes = synthesize(root.path(), "6").await;
        let trainer = host_trainer(&config, b"adapter");
        let request = remote(video("jerry", vec![clip("walk.mp4", &bytes)]))
            .with_compute_key(SecretValue::new("rp-test-key"));

        let outcome = train_with_pipeline(
            &config,
            String::new(),
            SecretValue::new(""),
            request,
            never_screened,
            false,
            Reporter::default(),
        )
        .await
        .expect("the host trainer trains the remote video adapter");
        let picked = trainer.await.unwrap();

        let job = &picked.job;
        assert_eq!(job.method, "video");
        assert_eq!(job.compute, host::Compute::Runpod);
        assert_eq!(job.secrets, ["runpod.key"]);
        assert_eq!(picked.secret.as_deref(), Some("rp-test-key\n"));
        assert!(job.fields.is_empty(), "{:?}", job.fields);
        let published = config.models_directory.join("loras/jerry-wan.safetensors");
        assert_eq!(outcome.path, published);
        assert_eq!(fs::read(&published).unwrap(), b"adapter");
        let directory = JobDirectory::new(&config, job.id);
        assert!(
            !directory.path().join("runpod.key").exists(),
            "the key outlived the job"
        );
    }
}
