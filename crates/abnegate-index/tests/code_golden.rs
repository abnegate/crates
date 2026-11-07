use std::collections::BTreeSet;
use std::fs;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use abnegate_index::ChunkConfig;
use abnegate_index::CodeChunk;
use abnegate_index::CodeLanguage;
use abnegate_index::TextChunk;
use abnegate_index::chunk_code;
use abnegate_index::chunk_code_with_config;
use abnegate_index::chunk_text;
use abnegate_index::estimate_tokens;
use abnegate_index::smart_chunk;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;

enum Chunks {
    Code(Vec<CodeChunk>),
    Text(Vec<TextChunk>),
    Estimate(usize),
}

struct Input {
    content: String,
    generated: bool,
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/code")
}

fn read_json(path: &Path) -> Value {
    let raw =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn cases() -> Vec<Value> {
    match read_json(&fixtures().join("cases.json")) {
        Value::Array(cases) => cases,
        other => panic!("cases.json is not a list: {other}"),
    }
}

fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("expected a string, got {value}"))
}

fn optional_usize(value: &Value) -> Option<usize> {
    value.as_u64().map(|value| value as usize)
}

fn required_usize(case: &Value, field: &str) -> usize {
    optional_usize(&case[field]).unwrap_or_else(|| panic!("case {} needs {field}", case["name"]))
}

fn required_bool(case: &Value, field: &str) -> bool {
    case[field]
        .as_bool()
        .unwrap_or_else(|| panic!("case {} needs {field}", case["name"]))
}

fn case_input(case: &Value) -> Input {
    match &case["input"] {
        Value::String(name) => Input {
            content: fs::read_to_string(fixtures().join("inputs").join(name))
                .unwrap_or_else(|error| panic!("read input {name}: {error}")),
            generated: false,
        },
        Value::Object(generator) => Input {
            content: text(&generator["repeat"])
                .repeat(optional_usize(&generator["count"]).expect("generator count")),
            generated: true,
        },
        other => panic!("unsupported input {other}"),
    }
}

fn case_language(case: &Value) -> CodeLanguage {
    match case["extension"].as_str() {
        Some(extension) => CodeLanguage::from_extension(extension),
        None => CodeLanguage::from_content_type(case["content_type"].as_str().unwrap_or("")),
    }
}

fn case_config(case: &Value) -> ChunkConfig {
    let config = ChunkConfig::default()
        .with_maximum_tokens(required_usize(case, "maximum_tokens"))
        .with_minimum_tokens(required_usize(case, "minimum_tokens"))
        .with_maximum_import_tokens(required_usize(case, "maximum_import_tokens"))
        .with_generate_skeletons(required_bool(case, "generate_skeletons"))
        .with_generate_api_surface(required_bool(case, "generate_api_surface"));
    match case["file_path"].as_str() {
        Some(file_path) => config.with_file_path(file_path),
        None => config,
    }
}

fn run_case(case: &Value, content: &str) -> Chunks {
    match text(&case["entry"]) {
        "smart_chunk" => Chunks::Text(smart_chunk(
            content,
            case["content_type"].as_str().unwrap_or(""),
            case["extension"].as_str(),
            case["file_path"].as_str(),
            required_usize(case, "maximum_tokens"),
            required_usize(case, "overlap_tokens"),
        )),
        "chunk_code" => Chunks::Code(chunk_code(
            content,
            case_language(case),
            required_usize(case, "maximum_tokens"),
        )),
        "chunk_code_with_config" => Chunks::Code(chunk_code_with_config(
            content,
            case_language(case),
            &case_config(case),
        )),
        "chunk_text" => Chunks::Text(chunk_text(
            content,
            required_usize(case, "maximum_tokens"),
            required_usize(case, "overlap_tokens"),
        )),
        "estimate_tokens" => Chunks::Estimate(estimate_tokens(content)),
        other => panic!("unknown entry {other}"),
    }
}

fn hexadecimal_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn canonical_text_chunks(chunks: &[TextChunk]) -> String {
    chunks
        .iter()
        .map(|chunk| {
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1e}",
                chunk.index, chunk.start_offset, chunk.end_offset, chunk.text
            )
        })
        .collect()
}

fn canonical_code_chunks(chunks: &[CodeChunk]) -> String {
    let mut canonical = String::new();
    for chunk in chunks {
        let metadata = &chunk.metadata;
        let identifiers = &chunk.identifiers;
        let fields = [
            chunk.index.to_string(),
            format!("{:?}", chunk.chunk_type),
            chunk.start_offset.to_string(),
            chunk.end_offset.to_string(),
            chunk.text.clone(),
            format!("{:?}", metadata.path),
            format!("{:?}", metadata.symbol),
            format!("{:?}", metadata.kind),
            format!("{:?}", metadata.visibility),
            format!("{:?}", metadata.parents),
            format!("{:?}", metadata.module),
            metadata.is_test.to_string(),
            metadata.is_generated.to_string(),
            metadata.start_byte.to_string(),
            metadata.end_byte.to_string(),
            metadata.content_hash.clone(),
            format!("{:?}", identifiers.defined_symbol),
            format!("{:?}", identifiers.parameters),
            format!("{:?}", identifiers.referenced_types),
            format!("{:?}", identifiers.referenced_functions),
            format!("{:?}", identifiers.key_strings),
        ];
        canonical.push_str(&fields.join("\u{1f}"));
        canonical.push('\u{1e}');
    }
    canonical
}

fn text_chunk_json(chunk: &TextChunk) -> Value {
    json!({
        "index": chunk.index,
        "text": chunk.text,
        "start_offset": chunk.start_offset,
        "end_offset": chunk.end_offset,
    })
}

fn code_chunk_json(chunk: &CodeChunk) -> Value {
    let metadata = &chunk.metadata;
    let identifiers = &chunk.identifiers;
    json!({
        "index": chunk.index,
        "chunk_type": format!("{:?}", chunk.chunk_type),
        "text": chunk.text,
        "start_offset": chunk.start_offset,
        "end_offset": chunk.end_offset,
        "metadata": {
            "path": metadata.path,
            "symbol": metadata.symbol,
            "kind": metadata.kind.as_ref().map(|kind| format!("{kind:?}")),
            "visibility": format!("{:?}", metadata.visibility),
            "parents": metadata.parents,
            "module": metadata.module,
            "is_test": metadata.is_test,
            "is_generated": metadata.is_generated,
            "start_byte": metadata.start_byte,
            "end_byte": metadata.end_byte,
            "content_hash": metadata.content_hash,
        },
        "identifiers": {
            "defined_symbol": identifiers.defined_symbol,
            "parameters": identifiers.parameters,
            "referenced_types": identifiers.referenced_types,
            "referenced_functions": identifiers.referenced_functions,
            "key_strings": identifiers.key_strings,
        },
    })
}

fn actual_golden(chunks: Chunks, generated: bool) -> Value {
    match (chunks, generated) {
        (Chunks::Estimate(estimate), _) => json!({ "estimate": estimate }),
        (Chunks::Code(chunks), true) => json!({
            "count": chunks.len(),
            "digest": hexadecimal_digest(canonical_code_chunks(&chunks).as_bytes()),
        }),
        (Chunks::Text(chunks), true) => json!({
            "count": chunks.len(),
            "digest": hexadecimal_digest(canonical_text_chunks(&chunks).as_bytes()),
        }),
        (Chunks::Code(chunks), false) => Value::Array(chunks.iter().map(code_chunk_json).collect()),
        (Chunks::Text(chunks), false) => Value::Array(chunks.iter().map(text_chunk_json).collect()),
    }
}

fn describe(value: Option<&Value>) -> String {
    value.map_or_else(|| "<missing>".to_string(), Value::to_string)
}

fn compare_fields(
    location: &str,
    field: &str,
    expected: &Value,
    actual: &Value,
    mismatches: &mut Vec<String>,
) {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            let keys: BTreeSet<&String> = expected.keys().chain(actual.keys()).collect();
            for key in keys {
                let path = if field.is_empty() {
                    key.clone()
                } else {
                    format!("{field}.{key}")
                };
                match (expected.get(key), actual.get(key)) {
                    (Some(expected), Some(actual)) => {
                        compare_fields(location, &path, expected, actual, mismatches)
                    }
                    (expected, actual) => mismatches.push(format!(
                        "{location} field {path}: expected {}, got {}",
                        describe(expected),
                        describe(actual)
                    )),
                }
            }
        }
        _ if expected != actual => mismatches.push(format!(
            "{location} field {field}: expected {expected}, got {actual}"
        )),
        _ => {}
    }
}

fn compare_case(name: &str, expected: &Value, actual: &Value, mismatches: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Array(expected), Value::Array(actual)) => {
            if expected.len() != actual.len() {
                mismatches.push(format!(
                    "case {name}: expected {} chunks, got {}",
                    expected.len(),
                    actual.len()
                ));
            }
            for (index, (expected, actual)) in expected.iter().zip(actual).enumerate() {
                let location = format!("case {name} chunk {index}");
                compare_fields(&location, "", expected, actual, mismatches);
            }
        }
        _ => compare_fields(&format!("case {name}"), "", expected, actual, mismatches),
    }
}

fn replay(generated: bool) -> usize {
    let mut mismatches = Vec::new();
    let mut replayed = 0;
    for case in cases() {
        let name = text(&case["name"]).to_string();
        let input = case_input(&case);
        if input.generated != generated {
            continue;
        }
        replayed += 1;
        let expected = read_json(&fixtures().join("golden").join(format!("{name}.json")));
        let outcome = catch_unwind(AssertUnwindSafe(|| run_case(&case, &input.content)));
        match outcome {
            Ok(chunks) => compare_case(
                &name,
                &expected,
                &actual_golden(chunks, input.generated),
                &mut mismatches,
            ),
            Err(_) => mismatches.push(format!("case {name}: the chunker panicked")),
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} golden mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    replayed
}

#[test]
fn stored_inputs_chunk_exactly_as_the_donor_did() {
    let replayed = replay(false);
    assert_eq!(replayed, 76, "stored input cases replayed");
}

#[test]
fn generated_inputs_match_the_donor_digests() {
    let replayed = replay(true);
    assert_eq!(replayed, 4, "generated input cases replayed");
}

fn nested_arrays(depth: usize) -> String {
    format!("{}{}", "[\n".repeat(depth), "]\n".repeat(depth))
}

fn nested_arrow_functions(depth: usize) -> String {
    format!(
        "{}{}{}",
        "const f = () => {\n".repeat(depth),
        "  const value = compute(input);\n".repeat(80),
        "};\n".repeat(depth)
    )
}

fn nested_arrow_functions_among_small_siblings(depth: usize) -> String {
    format!(
        "{}{}{}",
        "const small = 1;\nconst f = () => {\n".repeat(depth),
        "  const value = compute(input);\n".repeat(80),
        "};\nconst after = 2;\n".repeat(depth)
    )
}

#[test]
fn nested_symbols_over_the_token_budget_match_the_donor_digests() {
    let cases = [
        (
            nested_arrays(513),
            CodeLanguage::Json,
            485,
            "1fb6be071204517ffc1d66f2424d62b910fd933eef1c520ab5edd4b4ab1b543b",
        ),
        (
            nested_arrays(517),
            CodeLanguage::Json,
            485,
            "85a2934672c1d84a8280c7350b2993c1f3f541295f5b86357b4d763a0f8fece6",
        ),
        (
            nested_arrow_functions(3),
            CodeLanguage::TypeScript,
            4,
            "be01ffc11dfca60d046bd49ea3e8fec7e772679ce630ab5a7033f2d329cc2a0f",
        ),
        (
            nested_arrow_functions(6),
            CodeLanguage::TypeScript,
            5,
            "2e3fc6e51e43e170c43b5d074ccdd3346702d3e3839ebc12171640b39ea1bc64",
        ),
        (
            nested_arrow_functions_among_small_siblings(3),
            CodeLanguage::TypeScript,
            7,
            "59a4f3859aebc34997bdcaaf21eb719a28143e1f84cae4305c599e77158771f0",
        ),
        (
            nested_arrow_functions_among_small_siblings(5),
            CodeLanguage::TypeScript,
            7,
            "638b22ff4fbb89dc556715c290b9bac01ab77962378609d56ae18b3943f01651",
        ),
    ];
    for (source, language, count, digest) in cases {
        let chunks = chunk_code(&source, language, 512);
        assert_eq!(
            chunks.len(),
            count,
            "{language:?} of {} bytes",
            source.len()
        );
        assert_eq!(
            hexadecimal_digest(canonical_code_chunks(&chunks).as_bytes()),
            digest,
            "{language:?} of {} bytes",
            source.len()
        );
    }
}

#[test]
fn nesting_over_the_token_budget_finishes_promptly() {
    const DEADLINE: Duration = Duration::from_secs(60);
    const STACK_BYTES: usize = 64 * 1024 * 1024;

    let cases = [
        (nested_arrays(600), CodeLanguage::Json),
        (nested_arrow_functions(24), CodeLanguage::TypeScript),
    ];
    for (source, language) in cases {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .stack_size(STACK_BYTES)
            .spawn(move || {
                let _ = sender.send(chunk_code(&source, language, 512).len());
            })
            .expect("spawn chunking thread");
        let chunks = receiver
            .recv_timeout(DEADLINE)
            .unwrap_or_else(|error| panic!("{language:?} did not finish: {error}"));
        assert!(chunks > 0, "{language:?} produced no chunks");
    }
}

#[test]
fn every_golden_file_belongs_to_a_case() {
    let names: BTreeSet<String> = cases()
        .iter()
        .map(|case| text(&case["name"]).to_string())
        .collect();
    let goldens: BTreeSet<String> = fs::read_dir(fixtures().join("golden"))
        .expect("read golden directory")
        .map(|entry| {
            let path = entry.expect("golden entry").path();
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .expect("golden file name")
                .to_string()
        })
        .collect();
    assert_eq!(names.len(), cases().len(), "case names are unique");
    assert_eq!(goldens, names);
}

#[test]
fn language_detection_matches_the_donor() {
    let languages = read_json(&fixtures().join("languages.json"));
    let mut mismatches = Vec::new();
    let extensions = languages["extensions"].as_array().expect("extension rows");
    for row in extensions {
        let extension = text(&row["extension"]);
        let actual = CodeLanguage::from_extension(extension).as_str();
        if actual != text(&row["language"]) {
            mismatches.push(format!(
                "extension {extension:?}: expected {}, got {actual}",
                row["language"]
            ));
        }
    }
    let content_types = languages["content_types"]
        .as_array()
        .expect("content type rows");
    for row in content_types {
        let content_type = text(&row["content_type"]);
        let actual = CodeLanguage::from_content_type(content_type).as_str();
        if actual != text(&row["language"]) {
            mismatches.push(format!(
                "content type {content_type:?}: expected {}, got {actual}",
                row["language"]
            ));
        }
    }
    assert!(!extensions.is_empty() && !content_types.is_empty());
    assert!(
        mismatches.is_empty(),
        "{} language mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
