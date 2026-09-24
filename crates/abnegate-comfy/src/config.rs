//! Settings read from `COMFYUI_*` environment variables.

mod error;

pub use error::ConfigError;

use crate::train::Contract;
use abnegate_secret::SecretValue;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use std::env;
use std::path::PathBuf;
use std::time::Duration;

/// Default header that carries [`Config::api_token`], the one the proxy in
/// front of a token-protected ComfyUI checks.
pub const TOKEN_HEADER: &str = "X-Zone-ComfyUI-Token";

/// Variable [`Config::from_environment`] reads the U2-Net weights path from, the same
/// one `abnegate-vision` documents.
pub const VISION_MODEL_VARIABLE: &str = "ABNEGATE_VISION_MODEL";

/// Floor on every timeout, since a zero timeout fails a request before it is sent.
pub(crate) const MINIMUM_TIMEOUT: Duration = Duration::from_secs(1);
/// Floor on the poll interval, since a zero interval polls ComfyUI in a busy loop.
const MINIMUM_POLL_INTERVAL: Duration = Duration::from_millis(1);
const MAXIMUM_REQUEST_TIMEOUT_SECONDS: u64 = 600;
const DEFAULT_BASE_URL: &str = "http://comfyui:8188";
const DEFAULT_ARTIFACT_ROOT: &str = "/app/artifacts";
const DEFAULT_MODELS_DIRECTORY: &str = "/app/comfyui/models";
const DEFAULT_WORKFLOW_DIRECTORY: &str = "/app/comfyui/workflows";

/// Reading the value is the operating system's job; deciding what it means is
/// this crate's, so the two are separable and only one of them needs a process
/// to test.
fn truthy(value: Option<String>, default: bool) -> bool {
    match value {
        Some(value) => matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => default,
    }
}

/// A setting that is present but blank is not a setting.
fn text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn token(value: Option<String>) -> Option<SecretValue> {
    text(value).map(SecretValue::new)
}

fn bounded(value: Option<String>, default: u64, minimum: u64, maximum: u64) -> u64 {
    value
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
        .clamp(minimum, maximum)
}

fn seconds(value: Option<String>, default: Duration, minimum: u64, maximum: u64) -> Duration {
    Duration::from_secs(bounded(value, default.as_secs(), minimum, maximum))
}

fn milliseconds(value: Option<String>, default: Duration, minimum: u64, maximum: u64) -> Duration {
    let default = u64::try_from(default.as_millis()).unwrap_or(maximum);
    Duration::from_millis(bounded(value, default, minimum, maximum))
}

fn frames(value: Option<String>, default: u32, minimum: u32, maximum: u32) -> u32 {
    let frames = bounded(
        value,
        u64::from(default),
        u64::from(minimum),
        u64::from(maximum),
    );
    u32::try_from(frames).unwrap_or(maximum)
}

/// The graph at `value`, or the packaged graph `name` in the default workflow
/// directory.
fn workflow(value: Option<String>, name: &str) -> Option<PathBuf> {
    Some(value.map_or_else(
        || PathBuf::from(DEFAULT_WORKFLOW_DIRECTORY).join(name),
        PathBuf::from,
    ))
}

/// Direct image generation settings loaded from `COMFYUI_*` environment variables.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    /// Whether generation and graph training may reach ComfyUI at all.
    pub enabled: bool,
    /// ComfyUI server every request goes to, without a trailing slash.
    pub base_url: String,
    /// Token sent in [`Config::token_header`] with every request.
    pub api_token: Option<SecretValue>,
    /// Header [`Config::api_token`] is sent in.
    pub token_header: String,
    /// Image graph whose directory may overlay the packaged recipes. `None`
    /// uses the recipes and graphs packaged with the crate.
    pub workflow_path: Option<PathBuf>,
    /// Image checkpoint, or a LoRA whose sidecar names its base.
    pub checkpoint: String,
    /// `None` uses the packaged text-to-video graph.
    pub video_workflow_path: Option<PathBuf>,
    /// Diffusion model the video graphs load.
    pub video_unet: String,
    /// Text encoder the video graphs load.
    pub video_clip: String,
    /// VAE the video graphs load.
    pub video_vae: String,
    /// `None` uses the packaged text-to-audio graph.
    pub audio_workflow_path: Option<PathBuf>,
    /// Checkpoint the audio graph loads.
    pub audio_checkpoint: String,
    /// `None` uses the packaged upscale graphs.
    pub upscale_workflow_path: Option<PathBuf>,
    /// Model the upscale graphs load.
    pub upscale_model: String,
    /// Directory a host keeps generated media under.
    pub artifact_root: PathBuf,
    /// Model a host classifies prompts with; `auto` leaves the choice to it.
    pub classifier_model: String,
    /// Ceiling on one prompt classification.
    pub classifier_timeout: Duration,
    /// Vision model that captions LoRA training images. Empty disables captioning.
    pub caption_model: String,
    /// Ceiling on one caption request.
    pub caption_timeout: Duration,
    /// Ceiling on any one HTTP request. Generation and training are bounded by
    /// their own deadlines, not by this.
    pub request_timeout: Duration,
    /// Deadline for one image generation, from submission to the last byte.
    pub generation_timeout: Duration,
    /// Deadline for one video generation.
    pub video_generation_timeout: Duration,
    /// Deadline for one audio generation.
    pub audio_generation_timeout: Duration,
    /// Deadline for one image or video upscale.
    pub upscale_generation_timeout: Duration,
    /// Wait between two polls of a submitted graph's history.
    pub poll_interval: Duration,
    /// ComfyUI models root (`checkpoints/`, `loras/`, `diffusion_models/`, ...).
    pub models_directory: PathBuf,
    /// Optional command used to train a LoRA. Empty runs the packaged training
    /// graph on ComfyUI.
    pub train_command: Option<String>,
    /// Wall clock budget for a training run, graph or command.
    pub train_timeout: Duration,
    /// Decoder that turns a submitted clip into training frames.
    pub ffmpeg: String,
    /// Reads a clip's duration, so a long one lowers its sampling rate instead
    /// of being cut short.
    pub ffprobe: String,
    /// Frames kept per second of submitted video.
    pub frame_rate: u32,
    /// Frames one clip contributes to a training set.
    pub frame_limit: u32,
    /// U2-Net weights that locate the subject of a training image. `None`
    /// crops photos on their centre and video frames on whatever moved.
    pub vision_model: Option<PathBuf>,
    /// Node and namespace names the training graphs and training command are
    /// built with.
    pub contract: Contract,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: DEFAULT_BASE_URL.to_string(),
            api_token: None,
            token_header: TOKEN_HEADER.to_string(),
            workflow_path: None,
            checkpoint: "flux1-schnell-fp8.safetensors".to_string(),
            video_workflow_path: None,
            video_unet: "wan2.2_ti2v_5B_fp16.safetensors".to_string(),
            video_clip: "umt5_xxl_fp8_e4m3fn_scaled.safetensors".to_string(),
            video_vae: "wan2.2_vae.safetensors".to_string(),
            audio_workflow_path: None,
            audio_checkpoint: "ace_step_v1_3.5b.safetensors".to_string(),
            upscale_workflow_path: None,
            upscale_model: "RealESRGAN_x4plus.safetensors".to_string(),
            artifact_root: PathBuf::from(DEFAULT_ARTIFACT_ROOT),
            classifier_model: "auto".to_string(),
            classifier_timeout: Duration::from_secs(3),
            caption_model: String::new(),
            caption_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(120),
            generation_timeout: Duration::from_secs(300),
            video_generation_timeout: Duration::from_secs(600),
            audio_generation_timeout: Duration::from_secs(600),
            upscale_generation_timeout: Duration::from_secs(600),
            poll_interval: Duration::from_millis(500),
            models_directory: PathBuf::from(DEFAULT_MODELS_DIRECTORY),
            train_command: None,
            train_timeout: Duration::from_secs(3600),
            ffmpeg: "ffmpeg".to_string(),
            ffprobe: "ffprobe".to_string(),
            frame_rate: 4,
            frame_limit: 48,
            vision_model: None,
            contract: Contract::default(),
        }
    }
}

impl Config {
    /// Settings from `COMFYUI_*`, with the U2-Net weights taken from
    /// [`VISION_MODEL_VARIABLE`].
    pub fn from_environment() -> Self {
        Self::from_environment_with_vision_model(VISION_MODEL_VARIABLE)
    }

    /// [`Config::from_environment`], taking the U2-Net weights path from
    /// `variable` instead, for a deployment that already names it something
    /// else.
    pub fn from_environment_with_vision_model(variable: &str) -> Self {
        Self::from_variables(|name| env::var(name).ok(), variable)
    }

    /// Refuses settings that would fail every request, poll ComfyUI in a busy
    /// loop, or send a token no proxy could read. [`Client::new`](crate::Client::new),
    /// [`lora::train`](crate::lora::train) and [`train::run`](crate::train::run)
    /// call it first.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.poll_interval < MINIMUM_POLL_INTERVAL {
            return Err(ConfigError::new(
                "COMFYUI_POLL_INTERVAL_MILLISECONDS must be at least one millisecond",
            ));
        }
        for (timeout, message) in [
            (
                self.request_timeout,
                "COMFYUI_REQUEST_TIMEOUT_SECONDS must be at least one second",
            ),
            (
                self.generation_timeout,
                "COMFYUI_GENERATION_TIMEOUT_SECONDS must be at least one second",
            ),
            (
                self.video_generation_timeout,
                "COMFYUI_VIDEO_GENERATION_TIMEOUT_SECONDS must be at least one second",
            ),
            (
                self.audio_generation_timeout,
                "COMFYUI_AUDIO_GENERATION_TIMEOUT_SECONDS must be at least one second",
            ),
            (
                self.upscale_generation_timeout,
                "COMFYUI_UPSCALE_GENERATION_TIMEOUT_SECONDS must be at least one second",
            ),
            (
                self.caption_timeout,
                "COMFYUI_CAPTION_TIMEOUT_SECONDS must be at least one second",
            ),
            (
                self.classifier_timeout,
                "COMFYUI_CLASSIFIER_TIMEOUT_SECONDS must be at least one second",
            ),
            (
                self.train_timeout,
                "COMFYUI_TRAIN_TIMEOUT_SECONDS must be at least one second",
            ),
        ] {
            if timeout < MINIMUM_TIMEOUT {
                return Err(ConfigError::new(message));
            }
        }
        if HeaderName::from_bytes(self.token_header.as_bytes()).is_err() {
            return Err(ConfigError::new(
                "COMFYUI_TOKEN_HEADER is not a valid header name",
            ));
        }
        if let Some(token) = &self.api_token
            && HeaderValue::from_str(token.expose()).is_err()
        {
            return Err(ConfigError::new(
                "COMFYUI_API_TOKEN is not a valid header value",
            ));
        }
        self.contract.validate()
    }

    /// Settings from whatever `environment` answers for each variable, so the
    /// names are testable without touching the process environment.
    fn from_variables(environment: impl Fn(&str) -> Option<String>, vision_model: &str) -> Self {
        let defaults = Self::default();
        let models_directory = environment("COMFYUI_MODELS_DIRECTORY")
            .map_or(defaults.models_directory, PathBuf::from);
        Self {
            enabled: truthy(environment("COMFYUI_ENABLED"), defaults.enabled),
            base_url: environment("COMFYUI_BASE_URL")
                .unwrap_or(defaults.base_url)
                .trim_end_matches('/')
                .to_string(),
            api_token: token(environment("COMFYUI_API_TOKEN")),
            token_header: text(environment("COMFYUI_TOKEN_HEADER"))
                .unwrap_or(defaults.token_header),
            workflow_path: workflow(
                environment("COMFYUI_WORKFLOW_PATH"),
                "flux1-schnell-fp8-api.json",
            ),
            checkpoint: environment("COMFYUI_CHECKPOINT").unwrap_or(defaults.checkpoint),
            video_workflow_path: workflow(
                environment("COMFYUI_VIDEO_WORKFLOW_PATH"),
                "wan2.2-ti2v-5b-api.json",
            ),
            video_unet: environment("COMFYUI_VIDEO_UNET").unwrap_or(defaults.video_unet),
            video_clip: environment("COMFYUI_VIDEO_CLIP").unwrap_or(defaults.video_clip),
            video_vae: environment("COMFYUI_VIDEO_VAE").unwrap_or(defaults.video_vae),
            audio_workflow_path: workflow(
                environment("COMFYUI_AUDIO_WORKFLOW_PATH"),
                "ace-step-v1-3.5b-api.json",
            ),
            audio_checkpoint: environment("COMFYUI_AUDIO_CHECKPOINT")
                .unwrap_or(defaults.audio_checkpoint),
            upscale_workflow_path: workflow(
                environment("COMFYUI_UPSCALE_WORKFLOW_PATH"),
                "upscale-image-api.json",
            ),
            upscale_model: environment("COMFYUI_UPSCALE_MODEL").unwrap_or(defaults.upscale_model),
            artifact_root: environment("COMFYUI_ARTIFACT_ROOT")
                .map_or(defaults.artifact_root, PathBuf::from),
            classifier_model: text(environment("COMFYUI_CLASSIFIER_MODEL"))
                .unwrap_or(defaults.classifier_model),
            classifier_timeout: seconds(
                environment("COMFYUI_CLASSIFIER_TIMEOUT_SECONDS"),
                defaults.classifier_timeout,
                1,
                30,
            ),
            caption_model: text(environment("COMFYUI_CAPTION_MODEL")).unwrap_or_default(),
            caption_timeout: seconds(
                environment("COMFYUI_CAPTION_TIMEOUT_SECONDS"),
                defaults.caption_timeout,
                5,
                600,
            ),
            request_timeout: seconds(
                environment("COMFYUI_REQUEST_TIMEOUT_SECONDS"),
                defaults.request_timeout,
                MINIMUM_TIMEOUT.as_secs(),
                MAXIMUM_REQUEST_TIMEOUT_SECONDS,
            ),
            generation_timeout: seconds(
                environment("COMFYUI_GENERATION_TIMEOUT_SECONDS"),
                defaults.generation_timeout,
                10,
                3600,
            ),
            video_generation_timeout: seconds(
                environment("COMFYUI_VIDEO_GENERATION_TIMEOUT_SECONDS"),
                defaults.video_generation_timeout,
                10,
                3600,
            ),
            audio_generation_timeout: seconds(
                environment("COMFYUI_AUDIO_GENERATION_TIMEOUT_SECONDS"),
                defaults.audio_generation_timeout,
                10,
                3600,
            ),
            upscale_generation_timeout: seconds(
                environment("COMFYUI_UPSCALE_GENERATION_TIMEOUT_SECONDS"),
                defaults.upscale_generation_timeout,
                10,
                3600,
            ),
            poll_interval: milliseconds(
                environment("COMFYUI_POLL_INTERVAL_MILLISECONDS"),
                defaults.poll_interval,
                50,
                5000,
            ),
            vision_model: text(environment(vision_model))
                .map(PathBuf::from)
                .or_else(|| Some(models_directory.join("vision/u2net.onnx")))
                .filter(|path| path.is_file()),
            models_directory,
            train_command: text(environment("COMFYUI_TRAIN_COMMAND")),
            train_timeout: seconds(
                environment("COMFYUI_TRAIN_TIMEOUT_SECONDS"),
                defaults.train_timeout,
                60,
                14400,
            ),
            ffmpeg: text(environment("COMFYUI_FFMPEG")).unwrap_or(defaults.ffmpeg),
            ffprobe: text(environment("COMFYUI_FFPROBE")).unwrap_or(defaults.ffprobe),
            frame_rate: frames(
                environment("COMFYUI_TRAIN_FRAME_RATE"),
                defaults.frame_rate,
                1,
                30,
            ),
            frame_limit: frames(
                environment("COMFYUI_TRAIN_FRAME_LIMIT"),
                defaults.frame_limit,
                1,
                400,
            ),
            contract: defaults.contract,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::collections::HashMap;

    fn configured(variables: &[(&str, &str)]) -> Config {
        let variables: HashMap<&str, &str> = variables.iter().copied().collect();
        Config::from_variables(
            |name| variables.get(name).map(|value| (*value).to_string()),
            VISION_MODEL_VARIABLE,
        )
    }

    #[test]
    fn the_defaults_name_no_path_on_the_machine_that_built_the_crate() {
        let defaults = Config::default();
        for path in [
            &defaults.workflow_path,
            &defaults.video_workflow_path,
            &defaults.audio_workflow_path,
            &defaults.upscale_workflow_path,
        ] {
            assert_eq!(
                path, &None,
                "a default must fall back to the packaged graph"
            );
        }
        assert_eq!(defaults.audio_checkpoint, "ace_step_v1_3.5b.safetensors");
        assert_eq!(defaults.audio_generation_timeout, Duration::from_secs(600));
    }

    #[test]
    fn the_environment_defaults_to_the_container_graphs() {
        assert_eq!(
            configured(&[]).audio_workflow_path,
            Some(PathBuf::from(DEFAULT_WORKFLOW_DIRECTORY).join("ace-step-v1-3.5b-api.json"))
        );
    }

    #[test]
    fn an_empty_environment_reads_as_the_defaults() {
        let read = configured(&[]);
        let defaults = Config::default();
        assert_eq!(read.base_url, defaults.base_url);
        assert_eq!(read.models_directory, defaults.models_directory);
        assert_eq!(read.artifact_root, defaults.artifact_root);
        assert_eq!(read.request_timeout, defaults.request_timeout);
        assert_eq!(read.poll_interval, defaults.poll_interval);
        assert_eq!(read.train_timeout, defaults.train_timeout);
        assert_eq!(read.frame_rate, defaults.frame_rate);
    }

    #[test]
    fn every_renamed_variable_is_read_under_its_spelled_out_name() {
        let config = configured(&[
            ("COMFYUI_CLASSIFIER_TIMEOUT_SECONDS", "7"),
            ("COMFYUI_CAPTION_TIMEOUT_SECONDS", "70"),
            ("COMFYUI_REQUEST_TIMEOUT_SECONDS", "90"),
            ("COMFYUI_GENERATION_TIMEOUT_SECONDS", "400"),
            ("COMFYUI_VIDEO_GENERATION_TIMEOUT_SECONDS", "700"),
            ("COMFYUI_AUDIO_GENERATION_TIMEOUT_SECONDS", "800"),
            ("COMFYUI_UPSCALE_GENERATION_TIMEOUT_SECONDS", "900"),
            ("COMFYUI_TRAIN_TIMEOUT_SECONDS", "7200"),
            ("COMFYUI_POLL_INTERVAL_MILLISECONDS", "250"),
            ("COMFYUI_MODELS_DIRECTORY", "/srv/comfyui/models"),
            ("COMFYUI_TRAIN_FRAME_RATE", "6"),
            ("COMFYUI_ARTIFACT_ROOT", "/srv/artifacts"),
        ]);
        assert_eq!(config.classifier_timeout, Duration::from_secs(7));
        assert_eq!(config.caption_timeout, Duration::from_secs(70));
        assert_eq!(config.request_timeout, Duration::from_secs(90));
        assert_eq!(config.generation_timeout, Duration::from_secs(400));
        assert_eq!(config.video_generation_timeout, Duration::from_secs(700));
        assert_eq!(config.audio_generation_timeout, Duration::from_secs(800));
        assert_eq!(config.upscale_generation_timeout, Duration::from_secs(900));
        assert_eq!(config.train_timeout, Duration::from_secs(7200));
        assert_eq!(config.poll_interval, Duration::from_millis(250));
        assert_eq!(
            config.models_directory,
            PathBuf::from("/srv/comfyui/models")
        );
        assert_eq!(config.frame_rate, 6);
        assert_eq!(config.artifact_root, PathBuf::from("/srv/artifacts"));
    }

    #[test]
    fn the_environment_is_read_under_these_names_and_no_others() {
        let asked = RefCell::new(BTreeSet::new());
        Config::from_variables(
            |name| {
                asked.borrow_mut().insert(name.to_string());
                None
            },
            VISION_MODEL_VARIABLE,
        );
        let expected: BTreeSet<String> = [
            "ABNEGATE_VISION_MODEL",
            "COMFYUI_API_TOKEN",
            "COMFYUI_ARTIFACT_ROOT",
            "COMFYUI_AUDIO_CHECKPOINT",
            "COMFYUI_AUDIO_GENERATION_TIMEOUT_SECONDS",
            "COMFYUI_AUDIO_WORKFLOW_PATH",
            "COMFYUI_BASE_URL",
            "COMFYUI_CAPTION_MODEL",
            "COMFYUI_CAPTION_TIMEOUT_SECONDS",
            "COMFYUI_CHECKPOINT",
            "COMFYUI_CLASSIFIER_MODEL",
            "COMFYUI_CLASSIFIER_TIMEOUT_SECONDS",
            "COMFYUI_ENABLED",
            "COMFYUI_FFMPEG",
            "COMFYUI_FFPROBE",
            "COMFYUI_GENERATION_TIMEOUT_SECONDS",
            "COMFYUI_MODELS_DIRECTORY",
            "COMFYUI_POLL_INTERVAL_MILLISECONDS",
            "COMFYUI_REQUEST_TIMEOUT_SECONDS",
            "COMFYUI_TOKEN_HEADER",
            "COMFYUI_TRAIN_COMMAND",
            "COMFYUI_TRAIN_FRAME_LIMIT",
            "COMFYUI_TRAIN_FRAME_RATE",
            "COMFYUI_TRAIN_TIMEOUT_SECONDS",
            "COMFYUI_UPSCALE_GENERATION_TIMEOUT_SECONDS",
            "COMFYUI_UPSCALE_MODEL",
            "COMFYUI_UPSCALE_WORKFLOW_PATH",
            "COMFYUI_VIDEO_CLIP",
            "COMFYUI_VIDEO_GENERATION_TIMEOUT_SECONDS",
            "COMFYUI_VIDEO_UNET",
            "COMFYUI_VIDEO_VAE",
            "COMFYUI_VIDEO_WORKFLOW_PATH",
            "COMFYUI_WORKFLOW_PATH",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            asked.into_inner(),
            expected,
            "a renamed variable is read under its new name alone, with no fallback"
        );
    }

    #[test]
    fn a_setting_that_is_set_beats_its_default_and_a_missing_one_does_not() {
        for value in ["1", "true", "TRUE", "yes", "on"] {
            assert!(
                truthy(Some(value.into()), false),
                "{value} should read true"
            );
        }
        for value in ["0", "false", "no", "off", "", "maybe"] {
            assert!(
                !truthy(Some(value.into()), true),
                "{value} should read false"
            );
        }
        assert!(truthy(None, true), "an unset setting keeps its default");
        assert!(!truthy(None, false));
    }

    #[test]
    fn a_setting_that_is_blank_is_no_setting_at_all() {
        assert_eq!(text(Some("  ffmpeg  ".into())), Some("ffmpeg".into()));
        assert_eq!(text(Some("   ".into())), None, "whitespace is not a value");
        assert_eq!(text(Some(String::new())), None);
        assert_eq!(text(None), None);
    }

    #[test]
    fn a_setting_outside_its_range_is_clamped_rather_than_taken() {
        assert_eq!(bounded(Some("7".into()), 1, 0, 10), 7);
        assert_eq!(bounded(Some("99".into()), 1, 0, 10), 10, "over the ceiling");
        assert_eq!(bounded(Some("0".into()), 5, 2, 10), 2, "under the floor");
        assert_eq!(
            bounded(Some("not a number".into()), 5, 0, 10),
            5,
            "an unparseable setting is no setting at all"
        );
        assert_eq!(
            bounded(Some("-3".into()), 5, 0, 10),
            5,
            "so is a negative one"
        );
        assert_eq!(bounded(None, 5, 0, 10), 5);
    }

    #[test]
    fn frame_sampling_has_defaults_a_clip_can_be_trained_on() {
        let defaults = Config::default();
        assert_eq!(defaults.frame_rate, 4);
        assert_eq!(defaults.frame_limit, 48);
        assert_eq!(defaults.ffmpeg, "ffmpeg");
        assert_eq!(defaults.ffprobe, "ffprobe");
        assert_eq!(
            defaults.vision_model, None,
            "the weights are opt-in, so nothing is assumed present"
        );
    }

    #[test]
    fn subject_detection_stays_off_until_the_weights_are_actually_there() {
        let models = tempfile::tempdir().unwrap();
        let directory = models.path().display().to_string();
        assert_eq!(
            configured(&[("COMFYUI_MODELS_DIRECTORY", &directory)]).vision_model,
            None
        );
        let weights = models.path().join("vision/u2net.onnx");
        std::fs::create_dir_all(weights.parent().unwrap()).unwrap();
        std::fs::write(&weights, b"weights").unwrap();
        assert_eq!(
            configured(&[("COMFYUI_MODELS_DIRECTORY", &directory)]).vision_model,
            Some(weights)
        );
    }

    /// Set by Cargo, to its own binary, for every test process: a variable
    /// that names a file without the test having to mutate the environment.
    const SET_TO_A_FILE: &str = "CARGO";

    #[test]
    fn the_weights_path_is_read_from_the_variable_the_caller_names() {
        let Some(cargo) = env::var_os(SET_TO_A_FILE) else {
            return;
        };
        assert_eq!(
            Config::from_environment_with_vision_model(SET_TO_A_FILE).vision_model,
            Some(PathBuf::from(cargo))
        );
    }

    #[test]
    fn the_defaults_are_valid() {
        assert_eq!(Config::default().validate(), Ok(()));
    }

    #[test]
    fn a_zero_poll_interval_is_refused_rather_than_polled_in_a_busy_loop() {
        let config = Config {
            poll_interval: Duration::ZERO,
            ..Config::default()
        };
        assert!(config.validate().is_err());
        assert!(
            config
                .validate()
                .unwrap_err()
                .message()
                .contains("COMFYUI_POLL_INTERVAL_MILLISECONDS")
        );
    }

    #[test]
    fn a_zero_timeout_is_refused_rather_than_failing_every_request() {
        let zeroed: [fn(&mut Config); 8] = [
            |config| config.request_timeout = Duration::ZERO,
            |config| config.generation_timeout = Duration::ZERO,
            |config| config.video_generation_timeout = Duration::ZERO,
            |config| config.audio_generation_timeout = Duration::ZERO,
            |config| config.upscale_generation_timeout = Duration::ZERO,
            |config| config.caption_timeout = Duration::ZERO,
            |config| config.classifier_timeout = Duration::ZERO,
            |config| config.train_timeout = Duration::ZERO,
        ];
        for zero in zeroed {
            let mut config = Config::default();
            zero(&mut config);
            assert!(config.validate().is_err(), "{config:?} was accepted");
        }
    }

    #[test]
    fn a_contract_that_could_name_a_path_outside_comfyui_is_refused() {
        let mut config = Config::default();
        config.contract.folder_prefix = "../escape-".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn a_token_or_header_a_proxy_could_not_read_is_refused() {
        let header = Config {
            token_header: "Not A Header".into(),
            ..Config::default()
        };
        assert!(header.validate().is_err());
        let token = Config {
            api_token: Some(SecretValue::new("line\nbreak")),
            ..Config::default()
        };
        assert!(token.validate().is_err());
    }

    #[test]
    fn the_token_never_appears_in_debug_output() {
        let config = Config {
            api_token: Some(SecretValue::new("comfy-token-0123456789")),
            ..Config::default()
        };
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("comfy-token-0123456789"), "{rendered}");
    }

    #[test]
    fn the_token_is_stored_as_it_is_checked_without_surrounding_whitespace() {
        assert_eq!(
            token(Some("  comfy-token \n".into()))
                .as_ref()
                .map(SecretValue::expose),
            Some("comfy-token")
        );
        assert_eq!(token(Some(" \t ".into())), None);
        assert_eq!(token(None), None);
    }

    #[test]
    fn the_token_travels_in_the_header_the_proxy_checks_unless_told_otherwise() {
        assert_eq!(Config::default().token_header, TOKEN_HEADER);
        assert_eq!(configured(&[]).token_header, TOKEN_HEADER);
        assert_eq!(
            configured(&[("COMFYUI_TOKEN_HEADER", "X-Proxy-Token")]).token_header,
            "X-Proxy-Token"
        );
    }
}
