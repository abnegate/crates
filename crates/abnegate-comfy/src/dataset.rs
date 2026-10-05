//! Advisory checks on a training set, run before a job starts.
//!
//! A LoRA learns one subject and a run costs the better part of an hour, so a
//! set that cannot teach one is worth naming up front. The captioner already
//! describes every image and counts how often each word recurs across those
//! descriptions, and that count is a coherence measure: descriptions with
//! nothing in common are probably not one subject, and descriptions that repeat
//! each other are probably one shot taken eight times.
//!
//! The same descriptions also say what the subject is doing. A set that moves
//! the subject to a new room in every shot and never moves the subject itself
//! reads as varied by the count above, and still teaches the pose as part of the
//! identity, so the words for pose and viewpoint are read separately.
//!
//! Findings are reported, never enforced. The evidence is a small vision
//! model's vocabulary, so it will sometimes be wrong, and a wrong warning a user
//! can overrule costs far less than a refusal they cannot.
//!
//! A person's set also carries companions beside each `NNNN.png` for the
//! trainer to read, which [`lora::train`](crate::lora::train) writes and the
//! [`host`](crate::host) job directory hands on: which part of the person the
//! crop frames ([`kind_path`], a [`FrameKind`]), the pose its caption names
//! ([`pose_path`], a [`pose_cluster`]), the subject mask ([`mask_path`]), and
//! one [`Rebalance`] for the whole set ([`rebalance_path`]). A video window is
//! described by a [`ClipWindow`].

mod clip_window;
mod concern;
mod finding;
mod frame_kind;
mod rebalance;

pub use clip_window::ClipWindow;
pub use concern::Concern;
pub use finding::Finding;
pub use frame_kind::FrameKind;
pub use rebalance::Rebalance;

use crate::caption::content_words;
use crate::host::REBALANCE_FILE;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

/// A measured eight-image run improved the subject by 34.72%, so the floor sits
/// below it: under five, the set is also too small for the checks below to say
/// anything about it.
pub(crate) const MINIMUM: usize = 5;
/// Word overlap needs enough descriptions for a majority to exist.
const DESCRIBED: usize = 3;
/// Share of description pairs that must share a word for one subject to be
/// plausible. Sets of one subject measured 0.75 and up, albums of unrelated
/// subjects 0.32 and down.
const SHARED: f32 = 0.5;
/// Mean pairwise word similarity above which the images are the same shot.
/// Near-identical frames measured 0.76 and up, and the tightest set that still
/// varied its camera angle measured 0.52.
const REPEATED: f32 = 0.65;
/// Mean pairwise similarity of pose and viewpoint words above which every image
/// holds the same pose. Sets that varied the pose, the camera or both measured
/// 0.04 and down; the tightest legitimate set, one subject shot from eight
/// angles, measured 0.33; sets that changed only the background measured 0.83
/// and up.
const UNIFORM: f32 = 0.6;
/// The share of a person's frames one pose may hold before [`rebalance`]
/// draws it less often, so a few common setups cannot eat the step budget.
pub const REBALANCE_SHARE: f32 = 0.40;
/// The companion naming which part of the person a crop frames.
pub const KIND_SUFFIX: &str = ".kind";
/// The companion naming the pose a crop's caption names.
pub const POSE_SUFFIX: &str = ".pose";
/// The companion holding a crop's 8-bit subject mask, as PNG.
pub const MASK_SUFFIX: &str = ".mask.png";
/// The pose of a caption that names none of [`pose_cluster`]'s poses.
pub const UNKNOWN_POSE: &str = "unknown";
/// The extension of the OpenPose image [`pose_filename`] names.
const POSE_IMAGE_EXTENSION: &str = ".png";

/// What the subject is doing and where the camera is, grouped by the pose each
/// term names so that paraphrase does not read as variety: a set that says
/// "standing" in half its descriptions and "stands" in the rest is one pose.
/// The first term of each group names its cluster.
pub(crate) const POSES: &[&[&str]] = &[
    &["standing", "stands", "stood", "upright"],
    &["sitting", "sits", "seated", "sat", "perched"],
    &["lying", "lies", "laying", "reclining", "sprawled", "curled"],
    &["on back", "supine"],
    &["on side"],
    &["crouching", "crouched", "squatting", "hunched"],
    &["kneeling", "knelt", "on knees"],
    &["all fours", "on all fours", "hands and knees"],
    &["leaning", "leans", "propped"],
    &["walking", "walks", "striding", "strolling"],
    &["running", "runs", "sprinting", "bounding"],
    &[
        "jumping", "jumps", "leaping", "leaps", "airborne", "mid air",
    ],
    &["climbing", "climbs", "clambering"],
    &["dancing", "dances", "twirling", "spinning"],
    &["flying", "flies", "soaring", "hovering", "floating"],
    &["riding", "rides", "cycling"],
    &[
        "reaching",
        "reaches",
        "stretching",
        "pointing",
        "waving",
        "raising",
    ],
    &["holding", "holds", "carrying", "grasping", "clutching"],
    &[
        "handstand",
        "headstand",
        "cartwheel",
        "somersault",
        "upside down",
        "tumbling",
    ],
    &["front", "frontal", "facing", "head on"],
    &["back view", "rear", "from behind", "backward"],
    &["side", "profile", "sideways"],
    &["three quarter", "quarter"],
    &[
        "above",
        "overhead",
        "top down",
        "aerial",
        "birds eye",
        "high angle",
        "looking down",
    ],
    &[
        "below",
        "beneath",
        "underneath",
        "worms eye",
        "low angle",
        "looking up",
    ],
    &["close up", "closeup", "close crop", "macro"],
    &["wide", "distant", "long shot"],
    &["full body", "full length", "head to toe"],
    &["medium shot", "medium close", "waist up"],
];

/// Read the vision model's descriptions for signs the set cannot train well.
///
/// `count` is every image in the set; `descriptions` are only those the model
/// described, which is fewer when captions were written by hand or captioning
/// was unavailable.
pub fn inspect(descriptions: &[String], count: usize) -> Vec<Finding> {
    let mut findings = Vec::new();
    if count < MINIMUM {
        let noun = if count == 1 { "image" } else { "images" };
        findings.push(Finding {
            concern: Concern::TooFew,
            detail: format!(
                "Only {count} {noun} here, which is usually too few — add more until there are \
                 at least {MINIMUM}, so the LoRA learns the subject rather than these exact shots."
            ),
        });
    }
    let described: Vec<HashSet<String>> = descriptions
        .iter()
        .map(|description| content_words(description).collect::<HashSet<String>>())
        .filter(|words| !words.is_empty())
        .collect();
    if described.len() < DESCRIBED {
        return findings;
    }
    let mut pairs = 0_usize;
    let mut sharing = 0_usize;
    let mut similarity = 0.0_f32;
    for (index, left) in described.iter().enumerate() {
        for right in &described[index + 1..] {
            let common = left.intersection(right).count();
            pairs += 1;
            sharing += usize::from(common > 0);
            similarity += common as f32 / (left.len() + right.len() - common) as f32;
        }
    }
    let shared = sharing as f32 / pairs as f32;
    let repetition = similarity / pairs as f32;
    if repetition > REPEATED {
        findings.push(Finding {
            concern: Concern::LowVariety,
            detail: "These images look almost identical — add shots from other angles, distances \
                     and settings, or the LoRA will only be able to reproduce this one pose."
                .to_string(),
        });
    } else if shared < SHARED {
        findings.push(Finding {
            concern: Concern::MixedSubjects,
            detail: "These images look like different subjects and a LoRA can only learn one — \
                     keep the shots of a single subject and remove the rest."
                .to_string(),
        });
    } else if one_pose(descriptions, described.len()) {
        findings.push(Finding {
            concern: Concern::LowPoseVariety,
            detail: "These images all show the subject in the same pose — add shots of it \
                     sitting, moving or seen from another angle, otherwise it will hold this \
                     pose however you prompt it."
                .to_string(),
        });
    }
    findings
}

/// Whether the descriptions that name a pose all name the same one.
///
/// A description that names no pose is no evidence either way, so a set the
/// model described only by its surroundings is left unjudged rather than
/// assumed uniform.
fn one_pose(descriptions: &[String], described: usize) -> bool {
    let posed: Vec<HashSet<usize>> = descriptions
        .iter()
        .map(|description| poses(description))
        .filter(|poses| !poses.is_empty())
        .collect();
    if posed.len() < DESCRIBED || posed.len() * 2 <= described {
        return false;
    }
    let mut pairs = 0_usize;
    let mut similarity = 0.0_f32;
    for (index, left) in posed.iter().enumerate() {
        for right in &posed[index + 1..] {
            let common = left.intersection(right).count();
            pairs += 1;
            similarity += common as f32 / (left.len() + right.len() - common) as f32;
        }
    }
    similarity / pairs as f32 > UNIFORM
}

fn poses(description: &str) -> HashSet<usize> {
    let words = tokens(description);
    POSES
        .iter()
        .enumerate()
        .filter(|(_, terms)| terms.iter().any(|term| mentions(&words, term)))
        .map(|(index, _)| index)
        .collect()
}

/// Every pose `caption` names, as the cluster each belongs to, in the order
/// [`pose_cluster`] ranks them.
pub fn pose_clusters(caption: &str) -> Vec<&'static str> {
    let words = tokens(caption);
    POSES
        .iter()
        .filter(|terms| terms.iter().any(|term| mentions(&words, term)))
        .map(|terms| terms[0])
        .collect()
}

/// The pose a person crop's caption names, which its [`POSE_SUFFIX`]
/// companion holds and [`rebalance`] counts: the cluster of the first pose
/// it names, standing before sitting before lying and so on, or
/// [`UNKNOWN_POSE`].
pub fn pose_cluster(caption: &str) -> &'static str {
    pose_clusters(caption)
        .first()
        .copied()
        .unwrap_or(UNKNOWN_POSE)
}

/// The OpenPose image for `cluster` inside
/// [`Config::poses_directory`](crate::Config::poses_directory): its words
/// joined by underscores, `on back` as `on_back.png`. `None` for anything
/// [`pose_clusters`] does not return.
pub fn pose_filename(cluster: &str) -> Option<String> {
    POSES
        .iter()
        .map(|terms| terms[0])
        .find(|name| *name == cluster)
        .map(|name| format!("{}{POSE_IMAGE_EXTENSION}", name.replace(' ', "_")))
}

/// Whether `word` belongs to a pose, so the captioner keeps it even when
/// most of a set's captions share it.
pub(crate) fn pose_word(word: &str) -> bool {
    POSES
        .iter()
        .flat_map(|terms| terms.iter())
        .any(|term| term.split(' ').any(|part| part.eq_ignore_ascii_case(word)))
}

/// The draw weight of every frame of a person's set, keyed by its stem, from
/// the [`pose_cluster`] of each. A set whose largest pose holds at most
/// [`REBALANCE_SHARE`] of the frames draws every frame once; otherwise each
/// frame is drawn in inverse proportion to its pose's size, so the rarest
/// pose keeps weight 1. A stem listed twice counts once, with its last
/// pose.
pub fn rebalance<S: AsRef<str>, C: AsRef<str>>(frames: &[(S, C)]) -> Rebalance {
    let by_stem: BTreeMap<&str, &str> = frames
        .iter()
        .map(|(stem, cluster)| (stem.as_ref(), cluster.as_ref()))
        .collect();
    let mut clusters: BTreeMap<String, usize> = BTreeMap::new();
    for cluster in by_stem.values() {
        *clusters.entry((*cluster).to_string()).or_default() += 1;
    }
    let largest = clusters.values().copied().max().unwrap_or(0);
    let rarest = clusters.values().copied().min().unwrap_or(1);
    let skewed = !by_stem.is_empty() && largest as f32 / by_stem.len() as f32 > REBALANCE_SHARE;
    let weights = by_stem
        .iter()
        .map(|(stem, cluster)| {
            let weight = if skewed {
                rarest as f32 / clusters[*cluster] as f32
            } else {
                1.0
            };
            ((*stem).to_string(), weight)
        })
        .collect();
    Rebalance { clusters, weights }
}

/// The [`KIND_SUFFIX`] companion of the frame `stem` in `directory`.
pub fn kind_path(directory: &Path, stem: &str) -> PathBuf {
    companion(directory, stem, KIND_SUFFIX)
}

/// The [`POSE_SUFFIX`] companion of the frame `stem` in `directory`.
pub fn pose_path(directory: &Path, stem: &str) -> PathBuf {
    companion(directory, stem, POSE_SUFFIX)
}

/// The [`MASK_SUFFIX`] companion of the frame `stem` in `directory`.
pub fn mask_path(directory: &Path, stem: &str) -> PathBuf {
    companion(directory, stem, MASK_SUFFIX)
}

/// Where a person set in `directory` keeps its [`Rebalance`].
pub fn rebalance_path(directory: &Path) -> PathBuf {
    directory.join(REBALANCE_FILE)
}

fn companion(directory: &Path, stem: &str, suffix: &str) -> PathBuf {
    directory.join(format!("{stem}{suffix}"))
}

fn tokens(description: &str) -> Vec<String> {
    description
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn mentions(words: &[String], term: &str) -> bool {
    let phrase: Vec<&str> = term.split(' ').collect();
    words
        .windows(phrase.len())
        .any(|window| window.iter().zip(&phrase).all(|(word, part)| word == part))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptions(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn concerns(findings: &[Finding]) -> Vec<Concern> {
        findings.iter().map(|finding| finding.concern).collect()
    }

    fn dog() -> Vec<String> {
        descriptions(&[
            "golden retriever sitting on a wooden porch, three-quarter view, warm afternoon light",
            "golden retriever running across a grassy field, wide shot, bright overcast daylight",
            "close-up of a golden retriever on a couch, indoor lamp light, shallow depth of field",
            "a golden retriever standing in shallow water at a lake, side view, low evening sun",
            "the dog lying on a tiled kitchen floor, high angle, cool window light",
            "golden retriever on a snowy path, full body framing, flat winter light",
            "retriever with head tilted in front of a bookshelf, medium close-up, soft indoor light",
            "golden retriever in the back seat of a car, framed through the window, dim light",
        ])
    }

    /// Sixteen drawings of one character, every one of them upright and facing
    /// the viewer, each in a different place. This is the set that trains a
    /// LoRA which will not do a handstand.
    fn upright() -> Vec<String> {
        descriptions(&[
            "a cartoon fox wearing a red scarf, standing in a tiled kitchen, facing the camera, \
             warm morning light",
            "cartoon fox with a red scarf stands on a cobbled street, facing the camera, soft \
             overcast daylight",
            "a cartoon fox in a red scarf standing beside a tall bookshelf, facing the camera, \
             dim lamp light",
            "the cartoon fox stands in a grassy park, red scarf around its neck, facing the \
             camera, bright afternoon sun",
            "a cartoon fox wearing a red scarf standing on a sandy beach, facing the camera, \
             hazy golden light",
            "cartoon fox in a red scarf standing in a snowy forest clearing, facing the camera, \
             flat winter light",
            "a cartoon fox with a red scarf standing inside a train carriage, facing the camera, \
             cool fluorescent light",
            "the fox stands at a busy market stall wearing a red scarf, facing the camera, \
             dappled midday light",
            "a cartoon fox in a red scarf standing on a wooden jetty, facing the camera, pale \
             blue morning light",
            "cartoon fox wearing a red scarf, standing in a library aisle, facing the camera, \
             warm tungsten light",
            "a cartoon fox in a red scarf standing outside a bakery, facing the camera, bright \
             even daylight",
            "the cartoon fox stands in a rainy alley, red scarf soaked, facing the camera, dim \
             reflected light",
            "a cartoon fox with a red scarf standing in a flower meadow, facing the camera, soft \
             diffused sun",
            "cartoon fox in a red scarf standing on a subway platform, facing the camera, harsh \
             overhead light",
            "a cartoon fox wearing a red scarf, standing in a wheat field, facing the camera, \
             golden evening light",
            "the cartoon fox stands beside a campfire in a red scarf, facing the camera, \
             flickering orange light",
        ])
    }

    #[test]
    fn a_varied_set_of_one_subject_is_left_alone() {
        assert_eq!(inspect(&dog(), 8), Vec::new());
    }

    #[test]
    fn an_album_of_unrelated_photos_is_flagged_as_mixed() {
        let album = descriptions(&[
            "a tabby cat curled on a radiator, close-up, warm indoor light",
            "a red sports car on a mountain road, side view, golden hour",
            "a glass office tower seen from below, wide angle, flat overcast sky",
            "a plate of pasta on a marble counter, overhead shot, soft window light",
            "snow covered peaks across a valley, panoramic framing, cold morning haze",
            "a woman laughing at a dinner party, medium shot, candlelight",
            "a bicycle leaning against a fence, three-quarter view, late evening sun",
            "a bunch of sunflowers in a vase, close-up, bright daylight",
        ]);
        assert_eq!(concerns(&inspect(&album, 8)), vec![Concern::MixedSubjects]);
    }

    #[test]
    fn a_burst_of_near_identical_frames_is_flagged_as_low_variety() {
        let burst = descriptions(&[
            "sitting on a wooden stool, three-quarter view, warm indoor light",
            "sitting on a wooden stool, three-quarter view, warm indoor light",
            "sitting on a wooden stool, three-quarter view, warm indoor lighting",
            "seated on a wooden stool, three-quarter view, warm indoor light",
            "sitting on a wooden stool, three-quarter framing, warm indoor light",
            "sitting on the wooden stool, three-quarter view, warm indoor light",
            "sitting on a wooden stool, three-quarter view, warm indoor light",
            "sitting on a wooden stool, three-quarter view, warm interior light",
        ]);
        assert_eq!(concerns(&inspect(&burst, 8)), vec![Concern::LowVariety]);
    }

    /// One object on one backdrop shot from every side is a legitimate product
    /// set: the camera moves even though the words barely do.
    #[test]
    fn a_studio_set_that_only_moves_the_camera_is_left_alone() {
        let studio = descriptions(&[
            "toy robot on a white seamless backdrop, front view, even studio light",
            "toy robot on a white seamless backdrop, side view, even studio light",
            "toy robot on a white backdrop, three-quarter view, bright studio lighting",
            "toy robot on a white seamless backdrop, rear view, even studio light",
            "toy robot on a white backdrop, low angle, soft studio lighting",
            "toy robot on a white seamless backdrop, high angle, even studio light",
            "close-up of a toy robot on a white backdrop, even studio lighting",
            "toy robot on a white seamless backdrop, top down view, bright studio light",
        ]);
        assert_eq!(inspect(&studio, 8), Vec::new());
    }

    #[test]
    fn a_handful_of_images_is_flagged_as_too_few() {
        let few = dog()[..3].to_vec();
        let findings = inspect(&few, 3);
        assert_eq!(concerns(&findings), vec![Concern::TooFew]);
        assert!(
            findings[0].detail.contains("3 images"),
            "the user should be told the size of their own set, got {}",
            findings[0].detail
        );
    }

    #[test]
    fn a_single_image_reads_as_singular() {
        let findings = inspect(&[], 1);
        assert!(
            findings[0].detail.contains("1 image here"),
            "got {}",
            findings[0].detail
        );
    }

    #[test]
    fn nothing_to_read_is_not_a_finding() {
        assert_eq!(concerns(&inspect(&[], 0)), vec![Concern::TooFew]);
        assert_eq!(
            inspect(&descriptions(&["", "   ", "a, the, is"]), 12),
            Vec::new(),
            "a set nobody described says nothing about its subjects"
        );
    }

    /// Most captions were written by hand, so the model only described two
    /// images. Two descriptions are one pair, and one pair is not a set.
    #[test]
    fn a_set_the_model_barely_described_is_not_judged() {
        let sparse = descriptions(&[
            "a tabby cat curled on a radiator, close-up, warm indoor light",
            "a red sports car on a mountain road, side view, golden hour",
        ]);
        assert_eq!(inspect(&sparse, 8), Vec::new());
    }

    #[test]
    fn an_uncaptioned_set_is_still_sized() {
        assert_eq!(concerns(&inspect(&[], 4)), vec![Concern::TooFew]);
    }

    #[test]
    fn concerns_serialize_as_snake_case() {
        let findings = vec![
            Finding {
                concern: Concern::TooFew,
                detail: "too few".into(),
            },
            Finding {
                concern: Concern::LowVariety,
                detail: "low variety".into(),
            },
            Finding {
                concern: Concern::MixedSubjects,
                detail: "mixed subjects".into(),
            },
        ];
        assert_eq!(
            serde_json::to_value(&findings).unwrap(),
            serde_json::json!([
                {"concern": "too_few", "detail": "too few"},
                {"concern": "low_variety", "detail": "low variety"},
                {"concern": "mixed_subjects", "detail": "mixed subjects"},
            ])
        );
    }

    #[test]
    fn a_low_pose_variety_concern_serializes_as_snake_case() {
        let finding = Finding {
            concern: Concern::LowPoseVariety,
            detail: "low pose variety".into(),
        };
        assert_eq!(
            serde_json::to_value(&finding).unwrap(),
            serde_json::json!({"concern": "low_pose_variety", "detail": "low pose variety"})
        );
    }

    #[test]
    fn a_set_that_only_changes_the_background_is_flagged() {
        assert_eq!(
            concerns(&inspect(&upright(), 16)),
            vec![Concern::LowPoseVariety]
        );
    }

    #[test]
    fn the_pose_finding_reads_as_advice_about_poses() {
        let findings = inspect(&upright(), 16);
        assert!(
            findings[0].detail.contains("same pose"),
            "the user should be told what is wrong with their set, got {}",
            findings[0].detail
        );
    }

    /// Eight shots in one kitchen. Nothing moves but the subject, which is the
    /// one thing that has to.
    #[test]
    fn a_set_that_changes_only_the_pose_is_left_alone() {
        let kitchen = descriptions(&[
            "a cartoon fox in a red scarf standing in a tiled kitchen, facing the camera, warm \
             morning light",
            "the cartoon fox sitting on the kitchen floor, red scarf loose, side view, warm \
             morning light",
            "a cartoon fox lying stretched out across the kitchen tiles, seen from above, warm \
             morning light",
            "cartoon fox in a red scarf crouching behind a kitchen chair, low angle, warm morning \
             light",
            "the fox jumping to reach a shelf in the kitchen, red scarf trailing, side view, warm \
             morning light",
            "a cartoon fox leaning against the kitchen counter, three-quarter view, warm morning \
             light",
            "cartoon fox running across the kitchen with a red scarf, motion blurred, warm \
             morning light",
            "the cartoon fox kneeling to open a cupboard door, rear view, warm morning light",
        ]);
        assert_eq!(inspect(&kitchen, 8), Vec::new());
    }

    #[test]
    fn a_set_that_changes_both_the_pose_and_the_setting_is_left_alone() {
        let varied = descriptions(&[
            "a cartoon fox in a red scarf sitting on a park bench, three-quarter view, warm \
             afternoon light",
            "the cartoon fox running along a beach, red scarf streaming behind, wide shot, bright \
             daylight",
            "close-up of a cartoon fox lying on a rug indoors, soft lamp light",
            "a cartoon fox standing on a snowy ridge, side view, flat winter light",
            "cartoon fox climbing a wooden fence in a red scarf, low angle, overcast daylight",
            "the cartoon fox crouching under a market stall, high angle, dappled midday light",
            "a cartoon fox leaping between rooftops, red scarf trailing, seen from below, dusk \
             light",
            "cartoon fox seated at a kitchen table with a red scarf, front view, cool window light",
        ]);
        assert_eq!(inspect(&varied, 8), Vec::new());
    }

    /// Six frontal headshots of one character. A different set from the one
    /// above, failing the same way.
    #[test]
    fn a_run_of_headshots_is_flagged() {
        let portraits = descriptions(&[
            "a cartoon fox in a red scarf, head and shoulders, facing the camera, soft studio \
             light",
            "portrait of a cartoon fox wearing a red scarf, facing the camera, warm key light",
            "a cartoon fox in a red scarf, tight headshot, facing the camera, cool rim light",
            "head and shoulders of a cartoon fox with a red scarf, facing the camera, flat \
             daylight",
            "a cartoon fox in a red scarf, close crop of the face, facing the camera, dim lamp \
             light",
            "portrait of the cartoon fox in a red scarf, facing the camera, bright even light",
        ]);
        assert_eq!(
            concerns(&inspect(&portraits, 6)),
            vec![Concern::LowPoseVariety]
        );
    }

    #[test]
    fn a_small_set_of_one_pose_is_told_both_things() {
        let few = upright()[..4].to_vec();
        assert_eq!(
            concerns(&inspect(&few, 4)),
            vec![Concern::TooFew, Concern::LowPoseVariety],
            "a set can be both too small and stuck in one pose"
        );
    }

    /// The model named the room in every image and never said what the subject
    /// was doing, so the set says nothing about its poses either way.
    #[test]
    fn a_set_described_without_any_pose_is_not_judged() {
        let settings = descriptions(&[
            "a cartoon fox with a red scarf",
            "a cartoon fox in a bright kitchen",
            "cartoon fox outdoors near some trees",
            "the cartoon fox beside a wooden door",
            "a cartoon fox on a city street",
            "cartoon fox in a park with green grass",
            "a cartoon fox next to a window",
            "the cartoon fox by a stone wall",
        ]);
        assert_eq!(inspect(&settings, 8), Vec::new());
    }

    /// Two of eight descriptions mention a pose and both say the same one. Two
    /// is not enough of the set to accuse the other six.
    #[test]
    fn a_set_with_too_few_poses_named_is_not_judged() {
        let scarce = descriptions(&[
            "a cartoon fox with a red scarf",
            "a cartoon fox standing in a bright kitchen",
            "cartoon fox outdoors near some trees",
            "the cartoon fox beside a wooden door",
            "a cartoon fox standing on a city street",
            "cartoon fox in a park with green grass",
            "a cartoon fox next to a window",
            "the cartoon fox by a stone wall",
        ]);
        assert_eq!(inspect(&scarce, 8), Vec::new());
    }

    #[test]
    fn paraphrase_of_one_pose_is_still_one_pose() {
        let paraphrased = descriptions(&[
            "a cartoon fox in a red scarf stands in a tiled kitchen, front view, warm light",
            "the cartoon fox standing on a cobbled street, seen head on, overcast daylight",
            "cartoon fox stood beside a bookshelf in a red scarf, frontal view, dim lamp light",
            "a cartoon fox upright in a grassy park, facing the viewer, bright afternoon sun",
            "the cartoon fox stands on a sandy beach, red scarf loose, head on, hazy light",
        ]);
        assert_eq!(
            concerns(&inspect(&paraphrased, 5)),
            vec![Concern::LowPoseVariety],
            "the same pose in different words is still the same pose"
        );
    }

    #[test]
    fn companion_files_use_the_frame_stem() {
        let directory = Path::new("/job/dataset");
        assert_eq!(
            mask_path(directory, "0000"),
            Path::new("/job/dataset/0000.mask.png")
        );
        assert_eq!(
            kind_path(directory, "0000"),
            Path::new("/job/dataset/0000.kind")
        );
        assert_eq!(
            pose_path(directory, "0000"),
            Path::new("/job/dataset/0000.pose")
        );
        assert_eq!(
            rebalance_path(directory),
            Path::new("/job/dataset/rebalance.json")
        );
        assert_eq!(REBALANCE_SHARE, 0.40);
    }

    #[test]
    fn pose_cluster_uses_the_first_matching_canonical_term() {
        assert_eq!(
            pose_cluster("standing in a kitchen, warm light"),
            "standing"
        );
        assert_eq!(
            pose_cluster("from behind, tiled bathroom, cool window light"),
            "back view"
        );
        assert_eq!(pose_cluster("lying on back on a sofa"), "lying");
        assert_eq!(pose_cluster("on all fours on a rug"), "all fours");
        assert_eq!(pose_cluster("kneeling on a mat"), "kneeling");
        assert_eq!(pose_cluster("a kitchen with warm light"), UNKNOWN_POSE);
    }

    #[test]
    fn the_last_pose_a_prompt_names_picks_its_openpose_image() {
        for (prompt, image) in [
            ("ohwx sitting in a kitchen", "sitting.png"),
            ("standing close up", "close_up.png"),
            ("lying on a sofa, full body", "full_body.png"),
            ("lying on back on a sofa", "on_back.png"),
            ("on all fours on a rug", "all_fours.png"),
        ] {
            let cluster = pose_clusters(prompt).last().copied().unwrap();
            assert_eq!(pose_filename(cluster).as_deref(), Some(image), "{prompt}");
        }
        assert!(pose_clusters("a kitchen with warm light").is_empty());
    }

    #[test]
    fn every_pose_has_its_own_openpose_image_and_nothing_else_does() {
        let images: Vec<String> = POSES
            .iter()
            .map(|terms| pose_filename(terms[0]).unwrap())
            .collect();
        assert_eq!(
            images,
            [
                "standing.png",
                "sitting.png",
                "lying.png",
                "on_back.png",
                "on_side.png",
                "crouching.png",
                "kneeling.png",
                "all_fours.png",
                "leaning.png",
                "walking.png",
                "running.png",
                "jumping.png",
                "climbing.png",
                "dancing.png",
                "flying.png",
                "riding.png",
                "reaching.png",
                "holding.png",
                "handstand.png",
                "front.png",
                "back_view.png",
                "side.png",
                "three_quarter.png",
                "above.png",
                "below.png",
                "close_up.png",
                "wide.png",
                "full_body.png",
                "medium_shot.png",
            ]
        );
        for cluster in [UNKNOWN_POSE, "stands", "../standing", "", "Standing"] {
            assert_eq!(pose_filename(cluster), None, "{cluster:?}");
        }
    }

    #[test]
    fn pose_words_are_known_whole_and_in_part() {
        for word in ["standing", "Standing", "back", "fours", "supine"] {
            assert!(pose_word(word), "{word}");
        }
        for word in ["kitchen", "ohwx", "stand", ""] {
            assert!(!pose_word(word), "{word:?}");
        }
    }

    #[test]
    fn a_spread_pose_histogram_stays_uniform() {
        let frames = [
            ("0000", "standing"),
            ("0001", "sitting"),
            ("0002", "lying"),
            ("0003", "standing"),
            ("0004", "sitting"),
            ("0005", "lying"),
        ];
        let balanced = rebalance(&frames);
        assert_eq!(balanced.clusters.get("standing"), Some(&2));
        assert!(
            balanced.weights.values().all(|weight| *weight == 1.0),
            "a dump under {REBALANCE_SHARE} stays uniform: {:?}",
            balanced.weights
        );
    }

    #[test]
    fn rebalance_downsamples_only_when_one_cluster_exceeds_the_share() {
        let even = [
            ("0000", "standing"),
            ("0001", "sitting"),
            ("0002", "lying"),
            ("0003", "kneeling"),
            ("0004", "standing"),
            ("0005", "sitting"),
            ("0006", "lying"),
            ("0007", "kneeling"),
        ];
        let even = rebalance(&even);
        assert!(
            even.weights.values().all(|weight| *weight == 1.0),
            "2/8 is not above {REBALANCE_SHARE}: {:?}",
            even.weights
        );

        let overflow = [
            ("0000", "standing"),
            ("0001", "standing"),
            ("0002", "standing"),
            ("0003", "standing"),
            ("0004", "standing"),
            ("0005", "sitting"),
            ("0006", "sitting"),
            ("0007", "lying"),
            ("0008", "lying"),
        ];
        let overflow = rebalance(&overflow);
        assert_eq!(overflow.clusters.get("standing"), Some(&5));
        assert_eq!(overflow.weights.get("0000"), Some(&(2.0 / 5.0)));
        assert_eq!(overflow.weights.get("0005"), Some(&1.0));
        assert_eq!(overflow.weights.get("0007"), Some(&1.0));
    }

    #[test]
    fn rebalance_counts_each_stem_once() {
        let frames = [
            ("0000", "standing"),
            ("0000", "standing"),
            ("0001", "sitting"),
            ("0002", "sitting"),
        ];
        let balanced = rebalance(&frames);
        assert_eq!(balanced.clusters.get("standing"), Some(&1));
        assert_eq!(balanced.clusters.get("sitting"), Some(&2));
        assert_eq!(balanced.weights.len(), 3);
    }

    #[test]
    fn an_empty_set_rebalances_to_nothing() {
        let frames: [(&str, &str); 0] = [];
        assert_eq!(rebalance(&frames), Rebalance::default());
    }
}
