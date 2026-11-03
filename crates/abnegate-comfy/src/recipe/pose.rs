use serde_json::Map;
use serde_json::Value;
use std::collections::HashMap;

use super::link_node;
use crate::client::Error;

const APPLY_ADVANCED: &str = "ControlNetApplyAdvanced";
const APPLY: &str = "ControlNetApply";
const LOADER: &str = "ControlNetLoader";

/// Whether a ControlNet apply node reads the image at `pose`, so a pose
/// uploaded there steers the sampler.
pub(crate) fn is_read(workflow: &Value, pose: &str) -> bool {
    workflow.as_object().is_some_and(|nodes| {
        nodes.values().any(|node| {
            matches!(
                node.get("class_type").and_then(Value::as_str),
                Some(APPLY | APPLY_ADVANCED)
            ) && node.pointer("/inputs/image").and_then(link_node) == Some(pose)
        })
    })
}

/// Removes the ControlNet branch the image at `pose` feeds, so the sampler
/// reads the conditioning the branch would have wrapped.
pub(crate) fn strip(workflow: &mut Value, pose: &str) -> Result<(), Error> {
    let nodes = workflow
        .as_object_mut()
        .ok_or(Error::Configuration("workflow is not an object"))?;
    let mut replacements = HashMap::new();
    let mut appliers = Vec::new();
    let mut loaders = Vec::new();
    for (id, node) in nodes.iter() {
        if id == pose || node.pointer("/inputs/image").and_then(link_node) != Some(pose) {
            continue;
        }
        let inputs = node.get("inputs").and_then(Value::as_object);
        let class = node.get("class_type").and_then(Value::as_str);
        let outputs = match class {
            Some(APPLY_ADVANCED) => vec![
                input(inputs, "positive")?.clone(),
                input(inputs, "negative")?.clone(),
            ],
            Some(APPLY) => vec![input(inputs, "conditioning")?.clone()],
            _ => {
                return Err(Error::Configuration(
                    "the pose image feeds a node that is not a ControlNet apply",
                ));
            }
        };
        for (index, link) in outputs.into_iter().enumerate() {
            replacements.insert((id.clone(), index as u64), link);
        }
        loaders.extend(
            inputs
                .and_then(|inputs| inputs.get("control_net"))
                .and_then(link_node)
                .map(str::to_string),
        );
        appliers.push(id.clone());
    }
    nodes.remove(pose);
    for id in &appliers {
        nodes.remove(id);
    }
    for node in nodes.values_mut() {
        let Some(inputs) = node.get_mut("inputs").and_then(Value::as_object_mut) else {
            continue;
        };
        for value in inputs.values_mut() {
            *value = resolve(value, &replacements)?;
        }
    }
    let referenced: Vec<String> = nodes
        .values()
        .filter_map(|node| node.get("inputs").and_then(Value::as_object))
        .flat_map(Map::values)
        .filter_map(|value| link_node(value).map(str::to_string))
        .collect();
    nodes.retain(|id, node| {
        !loaders.contains(id)
            || node.get("class_type").and_then(Value::as_str) != Some(LOADER)
            || referenced.contains(id)
    });
    if referenced
        .iter()
        .any(|id| id == pose || appliers.contains(id))
    {
        return Err(Error::Configuration(
            "the pose branch is still referenced after it was removed",
        ));
    }
    Ok(())
}

fn input<'a>(inputs: Option<&'a Map<String, Value>>, name: &str) -> Result<&'a Value, Error> {
    inputs
        .and_then(|inputs| inputs.get(name))
        .ok_or(Error::Configuration(
            "a ControlNet apply node is missing its conditioning",
        ))
}

fn resolve(value: &Value, replacements: &HashMap<(String, u64), Value>) -> Result<Value, Error> {
    let mut current = value.clone();
    for _ in 0..=replacements.len() {
        let Some(key) = link_key(&current) else {
            return Ok(current);
        };
        match replacements.get(&key) {
            Some(next) => current = next.clone(),
            None => return Ok(current),
        }
    }
    Err(Error::Configuration("the pose branch links form a cycle"))
}

fn link_key(value: &Value) -> Option<(String, u64)> {
    let link = value.as_array().filter(|link| link.len() == 2)?;
    Some((link[0].as_str()?.to_string(), link[1].as_u64()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn branch() -> Value {
        json!({
            "3": { "class_type": "KSampler", "inputs": {
                "positive": ["22", 0], "negative": ["22", 1], "model": ["4", 0]
            }},
            "4": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": "x" } },
            "6": { "class_type": "CLIPTextEncode", "inputs": { "clip": ["4", 1] } },
            "7": { "class_type": "CLIPTextEncode", "inputs": { "clip": ["4", 1] } },
            "20": { "class_type": "LoadImage", "inputs": { "image": "pose.png" } },
            "21": { "class_type": "ControlNetLoader", "inputs": { "control_net_name": "c" } },
            "22": { "class_type": APPLY_ADVANCED, "inputs": {
                "positive": ["6", 0], "negative": ["7", 0],
                "control_net": ["21", 0], "image": ["20", 0]
            }}
        })
    }

    #[test]
    fn the_sampler_reads_the_plain_conditioning_once_the_branch_is_gone() {
        let mut workflow = branch();
        strip(&mut workflow, "20").unwrap();
        for id in ["20", "21", "22"] {
            assert!(workflow.get(id).is_none(), "{id}");
        }
        assert_eq!(workflow["3"]["inputs"]["positive"], json!(["6", 0]));
        assert_eq!(workflow["3"]["inputs"]["negative"], json!(["7", 0]));
        assert_eq!(workflow["3"]["inputs"]["model"], json!(["4", 0]));
    }

    #[test]
    fn chained_appliers_resolve_to_the_conditioning_beneath_them() {
        let mut workflow = branch();
        workflow["23"] = json!({ "class_type": APPLY, "inputs": {
            "conditioning": ["22", 0], "control_net": ["21", 0], "image": ["20", 0]
        }});
        workflow["3"]["inputs"]["positive"] = json!(["23", 0]);
        strip(&mut workflow, "20").unwrap();
        assert!(workflow.get("23").is_none());
        assert_eq!(workflow["3"]["inputs"]["positive"], json!(["6", 0]));
    }

    #[test]
    fn a_loader_another_node_still_uses_is_kept() {
        let mut workflow = branch();
        workflow["30"] = json!({ "class_type": "Custom", "inputs": { "control_net": ["21", 0] } });
        strip(&mut workflow, "20").unwrap();
        assert!(workflow.get("21").is_some());
    }

    /// A ControlNet the pose branch never used, such as one an overlay graph
    /// wires up later, is not part of that branch.
    #[test]
    fn a_loader_the_branch_does_not_use_is_kept() {
        let mut workflow = branch();
        workflow["40"] = json!({ "class_type": LOADER, "inputs": { "control_net_name": "d" } });
        strip(&mut workflow, "20").unwrap();
        assert!(workflow.get("21").is_none());
        assert!(workflow.get("40").is_some());
    }

    #[test]
    fn a_pose_feeding_anything_else_is_refused() {
        let mut workflow = branch();
        workflow["30"] = json!({ "class_type": "VAEEncode", "inputs": { "image": ["20", 0] } });
        assert!(strip(&mut workflow, "20").is_err());
    }
}
