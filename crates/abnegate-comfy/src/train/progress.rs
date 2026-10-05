use crate::excerpt;
use crate::train::Phase;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use serde_json::Value;
use std::path::Component;
use std::path::Path;
use std::time::Duration;

const MAXIMUM_PERCENT: u8 = 100;
const KEPT_PREVIEWS: usize = 32;

/// How far a training run has got, as a trainer writes it to its progress
/// file and as [`lora::train_reporting`](crate::lora::train_reporting) sends it.
///
/// On the wire it is one JSON object with the keys `step`, `total`, `phase`,
/// `message`, `percent`, `loss`, `eta_seconds` and `previews`; every key but
/// `step` and `total` may be left out. Reading one is lenient, because a
/// trainer rewrites the file while it runs: a whole number may arrive as a
/// float, a blank string counts as absent, `percent` is capped at 100, a loss
/// that is not finite and an estimate of zero are dropped, a preview that
/// could leave the progress file's directory is dropped, and a `total` of
/// zero makes the whole update unreadable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Value")]
#[non_exhaustive]
pub struct TrainProgress {
    /// Training steps taken, never more than `total`.
    pub step: u32,
    /// Training steps the run will take, at least one.
    pub total: u32,
    /// The stage the run is in. A phase read from a trainer is cleaned and
    /// capped like [`message`](Self::message).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
    /// What the run is doing, in words a person reads: at most 512
    /// characters, with terminal control sequences and credentials removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// How far through `phase` the run is, from 0 to 100. Every phase runs
    /// from 0 to 100 on its own, so a trainer writes the share of its current
    /// stage that is done, not of the whole run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// The training loss at `step`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loss: Option<f32>,
    /// How long the trainer expects the run still to take, in whole seconds
    /// and never zero.
    #[serde(
        rename = "eta_seconds",
        serialize_with = "whole_seconds",
        skip_serializing_if = "Option::is_none"
    )]
    pub eta: Option<Duration>,
    /// The latest sample images the trainer rendered, oldest first and at
    /// most 32 of them, as paths relative to the directory its progress file
    /// sits in. Each is one or more plain path components: no root, no `.` or
    /// `..`, and no `\`, `:` or control character.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub previews: Vec<String>,
}

impl TrainProgress {
    /// `step` of `total` steps, with `step` held to `total`. A `total` of
    /// zero counts as one, so every progress built is one a reader accepts.
    pub fn new(step: u32, total: u32) -> Self {
        let total = total.max(1);
        Self {
            step: step.min(total),
            total,
            phase: None,
            message: None,
            percent: None,
            loss: None,
            eta: None,
            previews: Vec::new(),
        }
    }

    /// The same progress in `phase`.
    pub fn with_phase(mut self, phase: Phase) -> Self {
        self.phase = Some(phase);
        self
    }

    /// The same progress described by `message`, trimmed, cut to 512
    /// characters and stripped of terminal control sequences and credentials.
    /// A blank message is no message.
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = described(&message.into());
        self
    }

    /// The same progress `percent` of the way through its phase, capped at
    /// 100.
    pub fn with_percent(mut self, percent: u8) -> Self {
        self.percent = Some(percent.min(MAXIMUM_PERCENT));
        self
    }

    /// The same progress at `loss`. A loss that is not finite is ignored.
    pub fn with_loss(mut self, loss: f32) -> Self {
        if loss.is_finite() {
            self.loss = Some(loss);
        }
        self
    }

    /// The same progress expected to finish in `eta`, rounded up to whole
    /// seconds. An estimate of zero is ignored.
    pub fn with_eta(mut self, eta: Duration) -> Self {
        if !eta.is_zero() {
            self.eta = Some(Duration::from_secs(
                eta.as_secs()
                    .saturating_add(u64::from(eta.subsec_nanos() > 0)),
            ));
        }
        self
    }

    /// The same progress with `previews` rendered so far, keeping the latest
    /// 32 of those that stay inside the progress file's directory.
    pub fn with_previews(mut self, previews: Vec<String>) -> Self {
        self.previews = kept(previews.iter().map(String::as_str));
        self
    }

    /// Reads a progress file. `None` when it is not a JSON object with a
    /// whole `step` and a nonzero `total`, which is also what a file caught
    /// halfway through being written looks like.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        Self::try_from(serde_json::from_slice::<Value>(bytes).ok()?).ok()
    }
}

impl TryFrom<Value> for TrainProgress {
    type Error = &'static str;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let count = |key: &str| {
            whole(&value, key)
                .and_then(|count| u32::try_from(count).ok())
                .ok_or("progress needs a whole step and total")
        };
        let step = count("step")?;
        let total = count("total")?;
        if total == 0 {
            return Err("progress needs a nonzero total");
        }
        Ok(Self {
            step: step.min(total),
            total,
            phase: text(&value, "phase")
                .and_then(|phase| described(&phase))
                .map(Phase::from),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .and_then(described),
            percent: whole(&value, "percent")
                .map(|percent| u8::try_from(percent).unwrap_or(MAXIMUM_PERCENT))
                .map(|percent| percent.min(MAXIMUM_PERCENT)),
            loss: value
                .get("loss")
                .and_then(Value::as_f64)
                .map(|loss| loss as f32)
                .filter(|loss| loss.is_finite()),
            eta: whole(&value, "eta_seconds")
                .filter(|seconds| *seconds > 0)
                .map(Duration::from_secs),
            previews: kept(
                value
                    .get("previews")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            ),
        })
    }
}

fn whole(value: &Value, key: &str) -> Option<u64> {
    let item = value.get(key)?;
    item.as_u64().or_else(|| {
        item.as_f64()
            .filter(|number| number.is_finite() && *number >= 0.0)
            .map(|number| number.round() as u64)
    })
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn described(message: &str) -> Option<String> {
    Some(excerpt::head(message)).filter(|message| !message.is_empty())
}

fn kept<'a>(previews: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut kept = previews
        .map(str::trim)
        .filter(|preview| is_preview(preview))
        .map(str::to_string)
        .collect::<Vec<String>>();
    kept.drain(..kept.len().saturating_sub(KEPT_PREVIEWS));
    kept
}

fn is_preview(preview: &str) -> bool {
    !preview.is_empty()
        && !preview
            .chars()
            .any(|character| matches!(character, '\\' | ':') || character.is_control())
        && Path::new(preview)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn whole_seconds<S: Serializer>(eta: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error> {
    eta.map(|eta| eta.as_secs()).serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DETAILED: &[u8] = br#"{"step":0,"total":8000,"phase":"class_images","message":"Generating class image 160 of 2000","percent":8,"eta_seconds":14400,"loss":0.21}"#;

    #[test]
    fn a_progress_file_names_the_step() {
        let progress = TrainProgress::parse(br#"{"step":12,"total":400}"#).unwrap();
        assert_eq!(progress, TrainProgress::new(12, 400));
        assert!(TrainProgress::parse(br#"{"step":1,"total":0}"#).is_none());
        assert!(TrainProgress::parse(br#"{"total":400}"#).is_none());
        assert!(TrainProgress::parse(br#"{"step":-1,"total":400}"#).is_none());
        assert!(TrainProgress::parse(b"not json").is_none());
        assert!(TrainProgress::parse(br#"{"step":12,"tot"#).is_none());
        assert_eq!(
            TrainProgress::parse(br#"{"step":500,"total":400}"#)
                .unwrap()
                .step,
            400
        );
    }

    #[test]
    fn a_detailed_progress_file_carries_its_phase_message_and_estimate() {
        let progress = TrainProgress::parse(DETAILED).unwrap();
        assert_eq!(progress.step, 0);
        assert_eq!(progress.total, 8000);
        assert_eq!(progress.phase, Some(Phase::ClassImages));
        assert_eq!(
            progress.message.as_deref(),
            Some("Generating class image 160 of 2000")
        );
        assert_eq!(progress.percent, Some(8));
        assert_eq!(progress.eta, Some(Duration::from_secs(14400)));
        assert_eq!(progress.loss, Some(0.21));
        assert!(progress.previews.is_empty());
    }

    #[test]
    fn previews_are_read_in_order_and_blanks_are_dropped() {
        let progress = TrainProgress::parse(
            br#"{"step":250,"total":8000,"previews":["previews/step-250-0.png"," ",7,"previews/step-250-1.png"]}"#,
        )
        .unwrap();
        assert_eq!(
            progress.previews,
            vec![
                "previews/step-250-0.png".to_string(),
                "previews/step-250-1.png".to_string()
            ]
        );
    }

    #[test]
    fn a_trainer_that_writes_floats_blanks_and_overflows_is_still_read() {
        let progress = TrainProgress::parse(
            br#"{"step":12.0,"total":400.4,"phase":"  ","message":"","percent":250,"loss":null,"eta_seconds":0}"#,
        )
        .unwrap();
        assert_eq!(progress, TrainProgress::new(12, 400).with_percent(100));
    }

    #[test]
    fn the_wire_keys_are_pinned() {
        let progress = TrainProgress::new(250, 8000)
            .with_phase(Phase::Training)
            .with_message("Training step 250 of 8000")
            .with_percent(70)
            .with_loss(0.5)
            .with_eta(Duration::from_secs(3600))
            .with_previews(vec!["previews/step-250-0.png".into()]);
        assert_eq!(
            serde_json::to_value(&progress).unwrap(),
            json!({
                "step": 250,
                "total": 8000,
                "phase": "training",
                "message": "Training step 250 of 8000",
                "percent": 70,
                "loss": 0.5,
                "eta_seconds": 3600,
                "previews": ["previews/step-250-0.png"],
            })
        );
        assert_eq!(
            serde_json::to_value(TrainProgress::new(0, 400)).unwrap(),
            json!({"step": 0, "total": 400}),
            "what a trainer did not say is left out"
        );
        let written = serde_json::to_vec(&progress).unwrap();
        assert_eq!(TrainProgress::parse(&written), Some(progress.clone()));
        assert_eq!(
            serde_json::from_slice::<TrainProgress>(&written).unwrap(),
            progress
        );
    }

    #[test]
    fn deserializing_refuses_what_parsing_refuses() {
        assert!(serde_json::from_value::<TrainProgress>(json!({"step": 1, "total": 0})).is_err());
        assert!(serde_json::from_value::<TrainProgress>(json!("training")).is_err());
    }

    #[test]
    fn the_builders_hold_their_values_in_range() {
        let progress = TrainProgress::new(9, 4)
            .with_percent(101)
            .with_loss(f32::NAN);
        assert_eq!(progress.step, 4);
        assert_eq!(progress.percent, Some(100));
        assert_eq!(progress.loss, None);
        let unknown = TrainProgress::new(9, 0);
        assert_eq!((unknown.step, unknown.total), (1, 1));
        assert_eq!(
            TrainProgress::parse(&serde_json::to_vec(&unknown).unwrap()),
            Some(unknown),
            "a progress built with no total is still one a reader accepts"
        );
    }

    #[test]
    fn previews_that_could_leave_the_progress_directory_are_dropped() {
        let escaping = [
            "/etc/passwd",
            "../outside.png",
            "previews/../../outside.png",
            "./previews/step-1.png",
            "previews\\..\\outside.png",
            "C:/previews/step-1.png",
            "previews/step\u{0}.png",
            "previews/step\n-1.png",
        ];
        let written = json!({
            "step": 1,
            "total": 400,
            "previews": escaping
                .iter()
                .copied()
                .chain(["previews/step-1.png"])
                .collect::<Vec<&str>>(),
        });
        let read = TrainProgress::parse(&serde_json::to_vec(&written).unwrap()).unwrap();
        assert_eq!(read.previews, vec!["previews/step-1.png".to_string()]);
        let built = TrainProgress::new(1, 400).with_previews(
            escaping
                .iter()
                .map(|preview| preview.to_string())
                .chain(["step-2.png".to_string()])
                .collect(),
        );
        assert_eq!(built.previews, vec!["step-2.png".to_string()]);
    }

    #[test]
    fn only_the_latest_previews_are_kept() {
        let rendered = (0..KEPT_PREVIEWS + 8)
            .map(|index| format!("previews/step-{index}.png"))
            .collect::<Vec<String>>();
        let read = TrainProgress::parse(
            &serde_json::to_vec(&json!({"step": 1, "total": 400, "previews": rendered})).unwrap(),
        )
        .unwrap();
        assert_eq!(read.previews, rendered[8..]);
        assert_eq!(
            TrainProgress::new(1, 400)
                .with_previews(rendered.clone())
                .previews,
            rendered[8..]
        );
    }

    #[test]
    fn an_estimate_survives_the_wire_in_whole_seconds() {
        let progress = TrainProgress::new(1, 400).with_eta(Duration::from_millis(1500));
        assert_eq!(progress.eta, Some(Duration::from_secs(2)));
        let written = serde_json::to_vec(&progress).unwrap();
        assert_eq!(TrainProgress::parse(&written), Some(progress));
        assert_eq!(
            TrainProgress::new(1, 400)
                .with_eta(Duration::from_millis(1))
                .eta,
            Some(Duration::from_secs(1)),
            "an estimate under a second is not an estimate of zero"
        );
        assert_eq!(
            TrainProgress::new(1, 400).with_eta(Duration::ZERO).eta,
            None
        );
    }

    #[test]
    fn a_message_is_bounded_and_sanitized() {
        let noisy = format!(
            "\u{1b}[31m{}token ghp_{}",
            "step ".repeat(500),
            "0123456789abcdefghij0123456789abcdef"
        );
        for progress in [
            TrainProgress::parse(
                &serde_json::to_vec(&json!({"step": 1, "total": 400, "message": noisy})).unwrap(),
            )
            .unwrap(),
            TrainProgress::new(1, 400).with_message(noisy.clone()),
        ] {
            let message = progress.message.unwrap();
            assert!(!message.contains('\u{1b}'), "{message:?}");
            assert!(!message.contains("0123456789abcdefghij"), "{message:?}");
            assert!(
                message.chars().count() <= crate::excerpt::LIMIT + 1,
                "{message:?}"
            );
        }
        assert_eq!(TrainProgress::new(1, 400).with_message("  ").message, None);
    }

    #[test]
    fn the_longest_estimate_saturates_instead_of_overflowing() {
        let progress = TrainProgress::new(0, 1).with_eta(Duration::MAX);
        assert_eq!(progress.eta, Some(Duration::from_secs(u64::MAX)));
    }

    #[test]
    fn a_phase_is_cleaned_like_a_message() {
        let phase = format!("rendering\u{1b}[31m {}", "x".repeat(4000));
        let body = json!({"step": 1, "total": 2, "phase": phase}).to_string();
        let progress = TrainProgress::parse(body.as_bytes()).unwrap();
        let Some(Phase::Other(name)) = progress.phase else {
            panic!("an unknown phase is Other: {:?}", progress.phase);
        };
        assert!(!name.contains('\u{1b}'), "{name:?}");
        assert!(name.len() <= 1024, "{}", name.len());
    }
}
