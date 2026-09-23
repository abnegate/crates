//! Settings read from `COMFYUI_*` environment variables.

mod error;

pub use error::ConfigError;

use crate::train::Contract;
use abnegate_secret::SecretValue;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use std::env;
use std::path::PathBuf;

/// Default header that carries [`Config::api_token`], the one the proxy in
/// front of a token-protected ComfyUI checks.
pub const TOKEN_HEADER: &str = "X-Zone-ComfyUI-Token";

/// Variable [`Config::from_env`] reads the U2-Net weights path from, the same
/// one `abnegate-vision` documents.
pub const VISION_MODEL_VARIABLE: &str = "ABNEGATE_VISION_MODEL";

/// Floor on every timeout, since a zero timeout fails a request before it is sent.
pub(crate) const MINIMUM_TIMEOUT_SECONDS: u64 = 1;
/// Floor on the poll interval, since a zero interval polls ComfyUI in a busy loop.
const MINIMUM_POLL_INTERVAL_MILLISECONDS: u64 = 1;
const DEFAULT_REQUEST_TIMEOUT_SECONDS: u64 = 120;
const MAXIMUM_REQUEST_TIMEOUT_SECONDS: u64 = 600;

fn env_truthy(name: &str, default: bool) -> bool {
    truthy(env::var(name).ok(), default)
}

fn env_u64(name: &str, default: u64, min: u64, max: u64) -> u64 {
    bounded(env::var(name).ok(), default, min, max)
}

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
fn env_text(name: &str) -> Option<String> {
    text(env::var(name).ok())
}

fn text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn token(value: Option<String>) -> Option<SecretValue> {
    text(value).map(SecretValue::new)
}

fn bounded(value: Option<String>, default: u64, min: u64, max: u64) -> u64 {
    value
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

/// Direct image generation settings loaded from `COMFYUI_*` environment variables.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    pub enabled: bool,
    pub base_url: String,
    pub api_token: Option<SecretValue>,
    /// Header [`Config::api_token`] is sent in.
    pub token_header: String,
    /// Image graph whose directory may overlay the packaged recipes. `None`
    /// uses the recipes and graphs packaged with the crate.
    pub workflow_path: Option<PathBuf>,
    pub checkpoint: String,
    /// `None` uses the packaged text-to-video graph.
    pub video_workflow_path: Option<PathBuf>,
    pub video_unet: String,
    pub video_clip: String,
    pub video_vae: String,
    /// `None` uses the packaged text-to-audio graph.
    pub audio_workflow_path: Option<PathBuf>,
    pub audio_checkpoint: String,
    /// `None` uses the packaged upscale graphs.
    pub upscale_workflow_path: Option<PathBuf>,
    pub upscale_model: String,
    pub artifact_root: PathBuf,
    pub classifier_model: String,
    pub classifier_timeout_seconds: u64,
    /// Vision model that captions LoRA training images. Empty disables captioning.
    pub caption_model: String,
    pub caption_timeout_seconds: u64,
    /// Ceiling on any one HTTP request. Generation and training are bounded by
    /// their own deadlines, not by this.
    pub request_timeout_seconds: u64,

    pub generation_timeout_seconds: u64,
    pub video_generation_timeout_seconds: u64,
    pub audio_generation_timeout_seconds: u64,
    pub upscale_generation_timeout_seconds: u64,
    pub poll_interval_milliseconds: u64,
    /// ComfyUI models root (`checkpoints/`, `loras/`, `diffusion_models/`, ...).
    pub models_directory: PathBuf,
    /// Optional command used to train a LoRA. Empty runs the packaged training
    /// graph on ComfyUI.
    pub train_command: Option<String>,
    /// Wall clock budget for a ComfyUI train job.
    pub train_timeout_seconds: u64,
    /// Decoder that turns a submitted clip into training frames.
    pub ffmpeg: String,
    /// Reads a clip's duration, so a long one lowers its sampling rate instead
    /// of being cut short.
    pub ffprobe: String,
    /// Frames kept per second of submitted video.
    pub frame_fps: u32,
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
            base_url: "http://comfyui:8188".to_string(),
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
            artifact_root: "/app/artifacts".into(),
            classifier_model: "auto".to_string(),
            classifier_timeout_seconds: 3,
            caption_model: String::new(),
            caption_timeout_seconds: 60,
            request_timeout_seconds: DEFAULT_REQUEST_TIMEOUT_SECONDS,
            generation_timeout_seconds: 300,
            video_generation_timeout_seconds: 600,
            audio_generation_timeout_seconds: 600,
            upscale_generation_timeout_seconds: 600,
            poll_interval_milliseconds: 500,
            models_directory: PathBuf::from("/app/comfyui/models"),
            train_command: None,
            train_timeout_seconds: 3600,
            ffmpeg: "ffmpeg".to_string(),
            ffprobe: "ffprobe".to_string(),
            frame_fps: 4,
            frame_limit: 48,
            vision_model: None,
            contract: Contract::default(),
        }
    }
}

impl Config {
    /// Settings from `COMFYUI_*`, with the U2-Net weights taken from
    /// [`VISION_MODEL_VARIABLE`].
    pub fn from_env() -> Self {
        Self::from_env_with_vision_model(VISION_MODEL_VARIABLE)
    }

    /// Refuses settings that would fail every request, poll ComfyUI in a busy
    /// loop, or send a token no proxy could read. [`Client::new`](crate::Client::new),
    /// [`lora::train`](crate::lora::train) and [`train::run`](crate::train::run)
    /// call it first.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.poll_interval_milliseconds < MINIMUM_POLL_INTERVAL_MILLISECONDS {
            return Err(ConfigError::new(
                "COMFYUI_POLL_INTERVAL_MS must be at least one millisecond",
            ));
        }
        for (seconds, message) in [
            (
                self.request_timeout_seconds,
                "COMFYUI_REQUEST_TIMEOUT_SECS must be at least one second",
            ),
            (
                self.generation_timeout_seconds,
                "COMFYUI_GENERATION_TIMEOUT_SECS must be at least one second",
            ),
            (
                self.video_generation_timeout_seconds,
                "COMFYUI_VIDEO_GENERATION_TIMEOUT_SECS must be at least one second",
            ),
            (
                self.audio_generation_timeout_seconds,
                "COMFYUI_AUDIO_GENERATION_TIMEOUT_SECS must be at least one second",
            ),
            (
                self.upscale_generation_timeout_seconds,
                "COMFYUI_UPSCALE_GENERATION_TIMEOUT_SECS must be at least one second",
            ),
            (
                self.caption_timeout_seconds,
                "COMFYUI_CAPTION_TIMEOUT_SECS must be at least one second",
            ),
            (
                self.classifier_timeout_seconds,
                "COMFYUI_CLASSIFIER_TIMEOUT_SECS must be at least one second",
            ),
            (
                self.train_timeout_seconds,
                "COMFYUI_TRAIN_TIMEOUT_SECS must be at least one second",
            ),
        ] {
            if seconds < MINIMUM_TIMEOUT_SECONDS {
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

    /// [`Config::from_env`], taking the U2-Net weights path from `variable`
    /// instead, for a deployment that already names it something else.
    pub fn from_env_with_vision_model(variable: &str) -> Self {
        let models_directory: PathBuf = env::var("COMFYUI_MODELS_DIR")
            .unwrap_or_else(|_| "/app/comfyui/models".to_string())
            .into();
        Self {
            enabled: env_truthy("COMFYUI_ENABLED", false),
            base_url: env::var("COMFYUI_BASE_URL")
                .unwrap_or_else(|_| "http://comfyui:8188".to_string())
                .trim_end_matches('/')
                .to_string(),
            api_token: token(env::var("COMFYUI_API_TOKEN").ok()),
            token_header: env_text("COMFYUI_TOKEN_HEADER")
                .unwrap_or_else(|| TOKEN_HEADER.to_string()),
            workflow_path: Some(
                env::var("COMFYUI_WORKFLOW_PATH")
                    .unwrap_or_else(|_| {
                        "/app/comfyui/workflows/flux1-schnell-fp8-api.json".to_string()
                    })
                    .into(),
            ),
            checkpoint: env::var("COMFYUI_CHECKPOINT")
                .unwrap_or_else(|_| "flux1-schnell-fp8.safetensors".to_string()),
            video_workflow_path: Some(
                env::var("COMFYUI_VIDEO_WORKFLOW_PATH")
                    .unwrap_or_else(|_| {
                        "/app/comfyui/workflows/wan2.2-ti2v-5b-api.json".to_string()
                    })
                    .into(),
            ),
            video_unet: env::var("COMFYUI_VIDEO_UNET")
                .unwrap_or_else(|_| "wan2.2_ti2v_5B_fp16.safetensors".to_string()),
            video_clip: env::var("COMFYUI_VIDEO_CLIP")
                .unwrap_or_else(|_| "umt5_xxl_fp8_e4m3fn_scaled.safetensors".to_string()),
            video_vae: env::var("COMFYUI_VIDEO_VAE")
                .unwrap_or_else(|_| "wan2.2_vae.safetensors".to_string()),
            audio_workflow_path: Some(
                env::var("COMFYUI_AUDIO_WORKFLOW_PATH")
                    .unwrap_or_else(|_| {
                        "/app/comfyui/workflows/ace-step-v1-3.5b-api.json".to_string()
                    })
                    .into(),
            ),
            audio_checkpoint: env::var("COMFYUI_AUDIO_CHECKPOINT")
                .unwrap_or_else(|_| "ace_step_v1_3.5b.safetensors".to_string()),
            upscale_workflow_path: Some(
                env::var("COMFYUI_UPSCALE_WORKFLOW_PATH")
                    .unwrap_or_else(|_| "/app/comfyui/workflows/upscale-image-api.json".to_string())
                    .into(),
            ),
            upscale_model: env::var("COMFYUI_UPSCALE_MODEL")
                .unwrap_or_else(|_| "RealESRGAN_x4plus.safetensors".to_string()),
            artifact_root: env::var("ARTIFACT_ROOT")
                .unwrap_or_else(|_| "/app/artifacts".to_string())
                .into(),
            classifier_model: env_text("COMFYUI_CLASSIFIER_MODEL")
                .unwrap_or_else(|| "auto".to_string()),
            classifier_timeout_seconds: env_u64("COMFYUI_CLASSIFIER_TIMEOUT_SECS", 3, 1, 30),
            caption_model: env_text("COMFYUI_CAPTION_MODEL").unwrap_or_default(),
            caption_timeout_seconds: env_u64("COMFYUI_CAPTION_TIMEOUT_SECS", 60, 5, 600),
            request_timeout_seconds: env_u64(
                "COMFYUI_REQUEST_TIMEOUT_SECS",
                DEFAULT_REQUEST_TIMEOUT_SECONDS,
                MINIMUM_TIMEOUT_SECONDS,
                MAXIMUM_REQUEST_TIMEOUT_SECONDS,
            ),

            generation_timeout_seconds: env_u64("COMFYUI_GENERATION_TIMEOUT_SECS", 300, 10, 3600),
            video_generation_timeout_seconds: env_u64(
                "COMFYUI_VIDEO_GENERATION_TIMEOUT_SECS",
                600,
                10,
                3600,
            ),
            audio_generation_timeout_seconds: env_u64(
                "COMFYUI_AUDIO_GENERATION_TIMEOUT_SECS",
                600,
                10,
                3600,
            ),
            upscale_generation_timeout_seconds: env_u64(
                "COMFYUI_UPSCALE_GENERATION_TIMEOUT_SECS",
                600,
                10,
                3600,
            ),
            poll_interval_milliseconds: env_u64("COMFYUI_POLL_INTERVAL_MS", 500, 50, 5000),
            vision_model: env_text(variable)
                .map(PathBuf::from)
                .or_else(|| Some(models_directory.join("vision/u2net.onnx")))
                .filter(|path| path.is_file()),
            models_directory,
            train_command: env_text("COMFYUI_TRAIN_COMMAND"),
            train_timeout_seconds: env_u64("COMFYUI_TRAIN_TIMEOUT_SECS", 3600, 60, 14400),
            ffmpeg: env_text("COMFYUI_FFMPEG").unwrap_or_else(|| "ffmpeg".to_string()),
            ffprobe: env_text("COMFYUI_FFPROBE").unwrap_or_else(|| "ffprobe".to_string()),
            frame_fps: env_u64("COMFYUI_TRAIN_FRAME_FPS", 4, 1, 30) as u32,
            frame_limit: env_u64("COMFYUI_TRAIN_FRAME_LIMIT", 48, 1, 400) as u32,
            contract: Contract::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(defaults.audio_generation_timeout_seconds, 600);
    }

    #[test]
    fn the_environment_defaults_to_the_container_graphs() {
        if env::var_os("COMFYUI_AUDIO_WORKFLOW_PATH").is_none() {
            assert_eq!(
                Config::from_env().audio_workflow_path,
                Some(PathBuf::from(
                    "/app/comfyui/workflows/ace-step-v1-3.5b-api.json"
                ))
            );
        }
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
        assert_eq!(defaults.frame_fps, 4);
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
        if env::var_os(VISION_MODEL_VARIABLE).is_some()
            || env::var_os("COMFYUI_MODELS_DIR").is_some()
        {
            return;
        }
        assert_eq!(Config::from_env().vision_model, None);
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
            Config::from_env_with_vision_model(SET_TO_A_FILE).vision_model,
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
            poll_interval_milliseconds: 0,
            ..Config::default()
        };
        assert!(config.validate().is_err());
        assert!(
            config
                .validate()
                .unwrap_err()
                .message()
                .contains("COMFYUI_POLL_INTERVAL_MS")
        );
    }

    #[test]
    fn a_zero_timeout_is_refused_rather_than_failing_every_request() {
        let zeroed: [fn(&mut Config); 8] = [
            |config| config.request_timeout_seconds = 0,
            |config| config.generation_timeout_seconds = 0,
            |config| config.video_generation_timeout_seconds = 0,
            |config| config.audio_generation_timeout_seconds = 0,
            |config| config.upscale_generation_timeout_seconds = 0,
            |config| config.caption_timeout_seconds = 0,
            |config| config.classifier_timeout_seconds = 0,
            |config| config.train_timeout_seconds = 0,
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
        if env::var_os("COMFYUI_TOKEN_HEADER").is_none() {
            assert_eq!(Config::from_env().token_header, TOKEN_HEADER);
        }
    }
}
