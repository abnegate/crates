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

use crate::caption::content_words;
use serde::Serialize;
use std::collections::HashSet;

/// A measured eight-image run improved the subject by 34.72%, so the floor sits
/// below it: under five, the set is also too small for the checks below to say
/// anything about it.
const MINIMUM: usize = 5;
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

/// What the subject is doing and where the camera is, grouped by the pose each
/// term names so that paraphrase does not read as variety: a set that says
/// "standing" in half its descriptions and "stands" in the rest is one pose.
const POSES: &[&[&str]] = &[
    &["standing", "stands", "stood", "upright"],
    &["sitting", "sits", "seated", "sat", "perched"],
    &["lying", "lies", "laying", "reclining", "sprawled", "curled"],
    &[
        "crouching",
        "crouched",
        "squatting",
        "kneeling",
        "knelt",
        "hunched",
    ],
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Concern {
    TooFew,
    LowVariety,
    MixedSubjects,
    LowPoseVariety,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub concern: Concern,
    pub detail: String,
}

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
    let words: Vec<String> = description
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    POSES
        .iter()
        .enumerate()
        .filter(|(_, terms)| terms.iter().any(|term| mentions(&words, term)))
        .map(|(index, _)| index)
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
}
