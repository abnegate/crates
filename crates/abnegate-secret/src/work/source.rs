//! The source rule behind `assert_linear`: outside their tests, the scanners
//! walk the text only through `work`, whose count `assert_linear` bounds.
//!
//! The rule reads each scanner file as statements, with comments, literals
//! and test modules blanked, and refuses each statement it cannot see taking
//! constant time outside `work`:
//!
//! - a loop;
//! - a method called on anything but a table or the value of a `work`
//!   helper, unless it is on `CONSTANT_TIME`, on `NUMERIC` and called on a
//!   number, on `BOUNDED_BY_ARGUMENT` and handed a constant, or on
//!   `SCANNER_METHODS`, whose bodies the rule reads. A table is a capitalised
//!   name reached without a call and indexed at most, as `QUOTES`,
//!   `INVISIBLE_CHARACTERS[range]` and `CharacterSet::Token` are, and the
//!   value of a helper is an index, a flag or the indices it found;
//! - a method on a table that is handed what is recognisably the text, or
//!   that is handed anything but a constant and is not on `TABLE_LOOKUPS`;
//! - a function or a macro the rule does not read, as `str::find`,
//!   `String::from_utf8_lossy` or `format!`, but for those on
//!   `CONSTANT_TIME_FUNCTIONS` and `CONSTANT_TIME_MACROS`, and a function
//!   called through a constant or a bracketed expression;
//! - an operator applied to what is recognisably the text, unless the other
//!   side is a constant;
//! - a scanner function that calls itself, directly or through others;
//! - a path, an import or a module declaration that reaches a module the rule
//!   does not read, a `#[path]`, or a `static`, which could hold the text
//!   under a table's name.
//!
//! Each statement it refuses that walks something other than the text is on
//! `ALLOWED`, with what it walks instead.
//!
//! The rule reads the source, not its types, so it refuses the walks it can
//! recognise and no more: the text bound to a new name and compared, for one,
//! passes it. The `linear` integration test is the backstop for what it
//! misses, timing the scanners on adversarial input in a release build.

mod allowance;
mod call;
mod expression;
mod refusal;
mod scope;
mod statement;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::work::source::allowance::ALLOWED;
use crate::work::source::call::Call;
use crate::work::source::expression::Expression;
use crate::work::source::expression::arguments;
use crate::work::source::expression::closing;
use crate::work::source::expression::identifier_end;
use crate::work::source::expression::identifiers;
use crate::work::source::expression::is_closure;
use crate::work::source::expression::is_identifier;
use crate::work::source::refusal::Refusal;
use crate::work::source::scope::Scope;
use crate::work::source::scope::WORK;
use crate::work::source::statement::Statement;
use crate::work::source::statement::statements;

/// Every scanner file, by its path under `src`.
const SCANNERS: &[(&str, &str)] = &[
    ("redact.rs", include_str!("../redact.rs")),
    (
        "redact/character_set.rs",
        include_str!("../redact/character_set.rs"),
    ),
    (
        "redact/credential.rs",
        include_str!("../redact/credential.rs"),
    ),
    ("sanitize.rs", include_str!("../sanitize.rs")),
    (
        "sanitize/terminators.rs",
        include_str!("../sanitize/terminators.rs"),
    ),
];

/// The directories whose every file is a scanner file.
const SCANNER_DIRECTORIES: &[&str] = &["redact", "sanitize"];

/// Methods that take constant time on anything a scanner holds, the text
/// included: on a string, a slice or a vector, only its length, whether it is
/// empty, a view of its bytes, one element or range of it, or one element
/// pushed onto it, and otherwise methods of numbers, characters, ranges,
/// flags and options, which the text has none of. `filter` and `map` also
/// adapt an iterator, which walks nothing until a consumer drives it, and no
/// consumer is on this list.
const CONSTANT_TIME: &[&str] = &[
    "as_bytes",
    "checked_sub",
    "encode_utf8",
    "end",
    "filter",
    "get",
    "is_ascii_alphabetic",
    "is_ascii_alphanumeric",
    "is_ascii_digit",
    "is_ascii_hexdigit",
    "is_ascii_lowercase",
    "is_ascii_uppercase",
    "is_ascii_whitespace",
    "is_control",
    "is_empty",
    "is_none",
    "is_none_or",
    "is_some",
    "is_some_and",
    "len",
    "len_utf8",
    "map",
    "map_or",
    "map_or_else",
    "or_else",
    "push",
    "saturating_sub",
    "start",
    "then_some",
    "unwrap_or",
];

/// Methods that take constant time on a number, but compare the text element
/// by element: allowed only on a number.
const NUMERIC: &[&str] = &["max"];

/// Methods that read no further than their argument: allowed only with a
/// constant one.
const BOUNDED_BY_ARGUMENT: &[&str] = &[
    "eq_ignore_ascii_case",
    "ends_with",
    "push_str",
    "starts_with",
    "strip_prefix",
    "strip_suffix",
];

/// Methods a table may be handed something other than a constant for: each
/// looks one entry up, or compares each entry with what it is handed, and
/// reads no further than an entry.
const TABLE_LOOKUPS: &[&str] = &["contains", "get"];

/// Methods the scanners declare, whose bodies the rule reads. None shares its
/// name with a method of a string, a slice, a vector, an option or an
/// iterator, whose calls the rule could not then tell from these.
const SCANNER_METHODS: &[&str] = &[
    "device_control",
    "end_at",
    "operating_system_command",
    "run",
];

/// Functions from outside the scanners that take constant time.
const CONSTANT_TIME_FUNCTIONS: &[&str] = &[
    "String::with_capacity",
    "Vec::new",
    "char::from",
    "f64::from",
    "usize::from",
];

/// Macros that take constant time: a pattern reads no further than itself.
const CONSTANT_TIME_MACROS: &[&str] = &["matches"];

/// Operators that compare or join what they are applied to, element by
/// element when it is the text.
const OPERATORS: &[&str] = &[" == ", " != ", " <= ", " >= ", " < ", " > ", " + ", " += "];

/// Assert that the scanners walk the text only through `work`, as far as the
/// rule can recognise a walk, so that its count sees every walk it can.
#[track_caller]
pub(super) fn assert_every_walk_counted() {
    let unreported = unreported();
    assert!(
        unreported.is_empty(),
        "a walk outside work is invisible to its count:\n{}",
        unreported.join("\n")
    );
}

/// Every statement of the scanners the rule refuses that is not allowed, and
/// every allowance that does not match exactly one refused statement.
fn unreported() -> Vec<String> {
    let mut matched = vec![0; ALLOWED.len()];
    let mut unreported = Vec::new();
    for refusal in refusals(SCANNERS) {
        match ALLOWED
            .iter()
            .position(|allowance| allowance.allows(refusal.file, &refusal.code))
        {
            Some(allowance) => matched[allowance] += 1,
            None => unreported.push(refusal.to_string()),
        }
    }
    for (allowance, matched) in ALLOWED.iter().zip(matched) {
        if matched != 1 {
            unreported.push(format!(
                "src/{}: `{}` is allowed once, as it walks {}, but is refused {matched} times",
                allowance.file, allowance.code, allowance.walks
            ));
        }
    }
    unreported
}

/// Every statement the rule refuses in `files`, each a scanner file's path
/// under `src` and its source.
fn refusals<'a>(files: &[(&'a str, &str)]) -> Vec<Refusal<'a>> {
    let parsed: Vec<(&str, String, Vec<Statement>)> = files
        .iter()
        .map(|(file, source)| (*file, module(file), statements(source)))
        .collect();
    let scope = Scope::of(
        parsed
            .iter()
            .map(|(_, module, statements)| (module.as_str(), statements.as_slice())),
    );
    let mut calls: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (_, module, statements) in &parsed {
        for statement in statements
            .iter()
            .filter(|statement| !statement.function.is_empty())
        {
            calls
                .entry(statement.function.as_str())
                .or_default()
                .extend(callees(statement, module, &scope));
        }
    }
    let mut refusals = Vec::new();
    for (file, module, statements) in &parsed {
        for statement in statements {
            let mut reasons = reasons(statement, module, &scope);
            reasons.extend(recursions(statement, module, &scope, &calls));
            if !reasons.is_empty() {
                refusals.push(Refusal {
                    file,
                    line: statement.line,
                    code: statement.code.clone(),
                    reasons,
                });
            }
        }
    }
    refusals
}

/// The path in the crate of the module in `file`, a path under `src`.
fn module(file: &str) -> String {
    file.trim_end_matches(".rs")
        .trim_end_matches("/mod")
        .replace('/', "::")
}

/// Why the rule refuses `statement`, a statement of `module`, but for
/// recursion: empty when it does not.
fn reasons(statement: &Statement, module: &str, scope: &Scope) -> Vec<String> {
    let (attributes, code) = split_attributes(&statement.code);
    let mut reasons = Vec::new();
    if identifiers(attributes).any(|(_, word)| word == "path") {
        reasons.push(String::from(
            "moves a module to a file the rule may not read",
        ));
    }
    if code
        .as_bytes()
        .windows(2)
        .any(|pair| matches!(pair, [b')' | b']' | b'}', b'(']))
    {
        reasons.push(String::from(
            "calls what a bracketed expression holds, which the rule does not read",
        ));
    }
    reasons.extend(keyword_reasons(code, module, scope));
    match imported(code) {
        Some(tree) => reasons.extend(import_reasons(tree, module, scope)),
        None => reasons.extend(path_reasons(code, module, scope)),
    }
    reasons.extend(
        Call::every(code)
            .iter()
            .filter_map(|call| call_reason(call, &statement.text, module, scope)),
    );
    reasons.extend(operator_reasons(code, &statement.text));
    reasons
}

/// The attributes `code` starts with, and the rest of it.
fn split_attributes(code: &str) -> (&str, &str) {
    let mut end = 0;
    loop {
        let rest = code[end..].trim_start();
        let start = code.len() - rest.len();
        let open = if rest.starts_with("#[") {
            start + 1
        } else if rest.starts_with("#![") {
            start + 2
        } else {
            break;
        };
        end = (closing(code.as_bytes(), open) + 1).min(code.len());
    }
    code.split_at(end)
}

/// Why the rule refuses the loops, statics and module declarations in `code`,
/// a statement of `module`.
fn keyword_reasons(code: &str, module: &str, scope: &Scope) -> Vec<String> {
    let implementation = identifiers(code)
        .next()
        .is_some_and(|(_, word)| matches!(word, "impl" | "unsafe"));
    identifiers(code)
        .filter_map(|(start, word)| {
            let after = &code[start + word.len()..];
            match word {
                "for" if implementation || after.starts_with('<') => None,
                "for" | "loop" | "while" => Some(String::from("loops")),
                "static" => Some(String::from(
                    "declares a static, which can hold the text under a table's name",
                )),
                "mod" => {
                    let path = format!("{module}::{}", declared_module(after)?);
                    (!scope.modules.contains(&path))
                        .then(|| format!("declares `{path}`, a module the rule does not read"))
                }
                _ => None,
            }
        })
        .collect()
}

/// The module that `after`, what follows `mod` in a statement, declares in a
/// file of its own, if it declares one.
fn declared_module(after: &str) -> Option<&str> {
    let after = after.trim_start();
    let name = identifier_end(after.as_bytes(), 0);
    after[name..]
        .trim_start()
        .starts_with(';')
        .then(|| &after[..name])
}

/// The tree of paths `code` imports, if it is a `use` statement.
fn imported(code: &str) -> Option<&str> {
    let mut rest = code.trim_start();
    if let Some(visible) = rest.strip_prefix("pub") {
        rest = visible.trim_start();
        if rest.starts_with('(') {
            rest = rest
                .get(closing(rest.as_bytes(), 0) + 1..)
                .unwrap_or_default();
        }
    }
    Some(
        rest.trim_start()
            .strip_prefix("use ")?
            .trim()
            .trim_end_matches(';'),
    )
}

/// Why the rule refuses the imports in `tree`, a `use` statement's tree in
/// `module`: a path that leaves the modules it reads, or a function or module
/// from outside the crate.
fn import_reasons(tree: &str, module: &str, scope: &Scope) -> Vec<String> {
    expanded(tree)
        .into_iter()
        .filter_map(|path| {
            let read = match absolute(&path, module) {
                Some(absolute) => reads(&absolute, scope),
                None => {
                    !is_crate_path(&path)
                        && is_capitalised(path.rsplit("::").next().unwrap_or_default())
                }
            };
            (!read).then(|| format!("imports `{path}`, which the rule does not read"))
        })
        .collect()
}

/// Every path `tree`, the tree of a `use` statement, imports, less any
/// rename.
fn expanded(tree: &str) -> Vec<String> {
    let tree = tree.trim();
    let Some(open) = tree.find('{') else {
        let path = tree.split(" as ").next().unwrap_or(tree);
        return vec![path.split_whitespace().collect()];
    };
    let prefix: String = tree[..open].split_whitespace().collect();
    arguments(
        tree.get(open + 1..closing(tree.as_bytes(), open))
            .unwrap_or_default(),
    )
    .into_iter()
    .flat_map(expanded)
    .map(|leaf| format!("{prefix}{leaf}"))
    .collect()
}

/// Why the rule refuses the paths from `crate`, `super` or `self` in `code`,
/// a statement of `module`: a path to a module it does not read.
fn path_reasons(code: &str, module: &str, scope: &Scope) -> Vec<String> {
    let bytes = code.as_bytes();
    identifiers(code)
        .filter(|(start, word)| {
            matches!(*word, "crate" | "self" | "super")
                && code[start + word.len()..].starts_with("::")
                && !code[..*start].ends_with("::")
        })
        .filter_map(|(start, _)| {
            let end = bytes[start..]
                .iter()
                .position(|byte| !is_identifier(*byte) && *byte != b':')
                .map_or(bytes.len(), |end| start + end);
            let path = code[start..end].trim_end_matches(':');
            let read = absolute(path, module).is_some_and(|absolute| reads(&absolute, scope));
            (!read).then(|| format!("names `{path}`, which the rule does not read"))
        })
        .collect()
}

/// The path in the crate that `path`, named in `module`, reaches, if it
/// starts from `crate`, `super` or `self` and stays in the crate.
fn absolute<'a>(path: &'a str, module: &'a str) -> Option<Vec<&'a str>> {
    let mut segments = path.split("::").peekable();
    let mut absolute: Vec<&str> = match segments.next()? {
        "crate" => Vec::new(),
        "self" => module.split("::").collect(),
        "super" => {
            let mut parent: Vec<&str> = module.split("::").collect();
            parent.pop()?;
            while segments.next_if_eq(&"super").is_some() {
                parent.pop()?;
            }
            parent
        }
        _ => return None,
    };
    absolute.extend(segments.filter(|segment| *segment != "self"));
    Some(absolute)
}

/// Whether `path` starts from `crate`, `super` or `self`.
fn is_crate_path(path: &str) -> bool {
    matches!(path.split("::").next(), Some("crate" | "self" | "super"))
}

/// Whether `absolute`, a path in the crate, is a module the rule reads, or an
/// item or a type's item in one.
fn reads(absolute: &[&str], scope: &Scope) -> bool {
    (1..=absolute.len())
        .rev()
        .find(|end| scope.modules.contains(&absolute[..*end].join("::")))
        .is_some_and(|end| match absolute[end..] {
            [] | [_] => true,
            [item, ..] => is_capitalised(item),
        })
}

/// Why the rule refuses `call`, in a statement of `module` in a function whose
/// `text` parameters hold the text, if it does.
fn call_reason(call: &Call<'_>, text: &[String], module: &str, scope: &Scope) -> Option<String> {
    let name = call.name;
    if call.expands {
        return (!CONSTANT_TIME_MACROS.contains(&name))
            .then(|| format!("expands `{name}!`, which the rule does not read"));
    }
    if let Some(receiver) = call.receiver {
        return method_reason(call, receiver, text);
    }
    if is_constant_name(name) {
        return Some(format!(
            "calls the function held in `{}{name}`, which the rule does not read",
            call.path
        ));
    }
    let read = if call.path.is_empty() {
        is_capitalised(name) || scope.functions.contains(name) || scope.closures.contains(name)
    } else {
        reads_function(call, module, scope)
    };
    (!read).then(|| format!("calls `{}{name}`, which the rule does not read", call.path))
}

/// Why the rule refuses `call`, a method called on `receiver`, if it does.
fn method_reason(call: &Call<'_>, receiver: Expression<'_>, text: &[String]) -> Option<String> {
    let name = call.name;
    if SCANNER_METHODS.contains(&name) || receiver.is_work_result() {
        return None;
    }
    if receiver.is_on_table() {
        let lookup = [TABLE_LOOKUPS, CONSTANT_TIME, BOUNDED_BY_ARGUMENT]
            .iter()
            .any(|methods| methods.contains(&name));
        return arguments(call.arguments)
            .into_iter()
            .filter(|argument| !is_closure(argument))
            .map(Expression)
            .find(|argument| {
                if lookup {
                    argument.is_text(text)
                } else {
                    !argument.is_constant()
                }
            })
            .map(|argument| {
                format!(
                    "hands `{}` to `{name}` on the table `{}`, which may walk it",
                    argument.0.trim(),
                    receiver.0
                )
            });
    }
    let bounded = CONSTANT_TIME.contains(&name)
        || (NUMERIC.contains(&name) && receiver.is_number())
        || (BOUNDED_BY_ARGUMENT.contains(&name) && Expression(call.arguments).is_constant());
    (!bounded).then(|| format!("calls `{name}` on `{}`, which may walk it", receiver.0))
}

/// Whether the rule reads the function `call` names by its path, from
/// `module`: a `work` helper, a constant-time function, a constructor, or a
/// function of a scanner type or module.
fn reads_function(call: &Call<'_>, module: &str, scope: &Scope) -> bool {
    let qualified = call.qualified();
    let owner = call.owner();
    CONSTANT_TIME_FUNCTIONS.contains(&qualified.as_str())
        || is_capitalised(call.name)
        || owner == "Self"
        || scope.types.contains(&owner)
        || call.path == format!("{WORK}::")
        || absolute(&qualified, module).is_some_and(|absolute| reads(&absolute, scope))
}

/// Why the rule refuses the operators in `code` applied to what is
/// recognisably the text, `text` being the parameters that hold it.
fn operator_reasons(code: &str, text: &[String]) -> Vec<String> {
    OPERATORS
        .iter()
        .flat_map(|operator| code.match_indices(operator))
        .filter_map(|(at, operator)| {
            let left = Expression::before(code, at);
            let right = Expression::after(code, at + operator.len());
            let walks = (left.is_text(text) || right.is_text(text))
                && !left.is_constant()
                && !right.is_constant();
            walks.then(|| {
                format!(
                    "applies `{}` to `{}` and `{}`, which may be the text",
                    operator.trim(),
                    left.0,
                    right.0
                )
            })
        })
        .collect()
}

/// The scanner functions `statement`, a statement of `module`, calls.
fn callees<'a>(statement: &'a Statement, module: &str, scope: &Scope) -> Vec<&'a str> {
    Call::every(&statement.code)
        .into_iter()
        .filter(|call| {
            let owner = call.owner();
            let scanner = call.path.is_empty()
                || owner == "Self"
                || scope.types.contains(&owner)
                || absolute(&call.qualified(), module).is_some_and(|absolute| {
                    absolute.first() != Some(&WORK) && reads(&absolute, scope)
                });
            !call.expands && scope.functions.contains(call.name) && scanner
        })
        .map(|call| call.name)
        .collect()
}

/// How `statement`, a statement of `module`, recurses: each call it makes
/// that leads back to the function it is in, by what `calls` holds each
/// scanner function to call.
fn recursions<'a>(
    statement: &'a Statement,
    module: &str,
    scope: &Scope,
    calls: &BTreeMap<&'a str, BTreeSet<&'a str>>,
) -> Vec<String> {
    let function = statement.function.as_str();
    if function.is_empty() {
        return Vec::new();
    }
    callees(statement, module, scope)
        .into_iter()
        .filter(|callee| reaches(calls, callee, function))
        .map(|callee| {
            if callee == function {
                format!("calls `{function}` from inside itself")
            } else {
                format!("calls `{callee}`, which calls `{function}` back")
            }
        })
        .collect()
}

/// Whether calling `from` can lead to calling `to`, by what `calls` holds
/// each scanner function to call.
fn reaches<'a>(calls: &BTreeMap<&'a str, BTreeSet<&'a str>>, from: &'a str, to: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut pending = vec![from];
    while let Some(function) = pending.pop() {
        if function == to {
            return true;
        }
        if seen.insert(function) {
            pending.extend(calls.get(function).into_iter().flatten().copied());
        }
    }
    false
}

fn is_capitalised(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_uppercase())
}

/// Whether `name` is a constant's, in SCREAMING_CASE, rather than a type's or
/// a variant's.
fn is_constant_name(name: &str) -> bool {
    name.len() > 1
        && name.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
}

/// Why the rule refuses `code`, as the body of a scanner function.
fn refusals_of(code: &str) -> Vec<String> {
    let probe = format!("fn line_end(bytes: &[u8], at: usize) -> usize {{\n{code}\n}}\n");
    let mut files: Vec<(&str, &str)> = SCANNERS.to_vec();
    files.push(("probe.rs", &probe));
    refusals(&files)
        .into_iter()
        .filter(|refusal| refusal.file == "probe.rs")
        .flat_map(|refusal| refusal.reasons)
        .collect()
}

#[test]
fn every_walk_in_the_scanners_goes_through_work() {
    assert_every_walk_counted();
}

#[test]
fn every_scanner_file_is_checked() {
    for directory in SCANNER_DIRECTORIES {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join(directory);
        for entry in fs::read_dir(path).unwrap() {
            let name = format!(
                "{directory}/{}",
                entry.unwrap().file_name().to_string_lossy()
            );
            assert!(
                SCANNERS.iter().any(|(file, _)| *file == name),
                "src/{name} is a scanner file the walk rule does not check"
            );
        }
    }
}

#[test]
fn every_scanner_method_is_one_the_scanners_declare() {
    let parsed: Vec<(String, Vec<Statement>)> = SCANNERS
        .iter()
        .map(|(file, source)| (module(file), statements(source)))
        .collect();
    let scope = Scope::of(
        parsed
            .iter()
            .map(|(module, statements)| (module.as_str(), statements.as_slice())),
    );
    for method in SCANNER_METHODS {
        assert!(
            scope.functions.contains(*method),
            "`{method}` is allowed as a scanner method, but no scanner declares it"
        );
    }
}

#[test]
fn every_name_the_rule_allows_is_one_a_scanner_calls() {
    let statements: Vec<Statement> = SCANNERS
        .iter()
        .flat_map(|(_, source)| statements(source))
        .collect();
    let calls: Vec<Call<'_>> = statements
        .iter()
        .flat_map(|statement| Call::every(&statement.code))
        .collect();
    let called = |allowed: &str| {
        calls.iter().any(|call| match call.receiver {
            Some(_) => call.name == allowed,
            None if call.expands => call.name == allowed,
            None => call.qualified() == allowed,
        })
    };
    for allowed in [
        CONSTANT_TIME,
        NUMERIC,
        BOUNDED_BY_ARGUMENT,
        TABLE_LOOKUPS,
        SCANNER_METHODS,
        CONSTANT_TIME_FUNCTIONS,
        CONSTANT_TIME_MACROS,
    ]
    .concat()
    {
        assert!(
            called(allowed),
            "`{allowed}` is allowed, but no scanner calls it"
        );
    }
}

#[test]
fn refuses_every_way_of_walking_the_text_outside_work() {
    for code in [
        "(from..bytes.len()).find_map(|index| string_terminator_end(bytes, index))",
        "while index < bytes.len() && bytes[index] != BELL {",
        "for index in from..bytes.len() {",
        "loop {",
        "bytes[from..].iter().position(|byte| *byte == BELL)",
        "if !bytes[from..].contains(&BELL) {",
        "let end = rest.find('\\n')?;",
        "text.lines().next()",
        "str::find(text, closing)",
        "<[u8]>::iter(bytes).any(|byte| *byte == BELL)",
        "CREDENTIALS.iter().any(|_| bytes[index..].iter().any(|byte| *byte == BELL))",
        "memchr::memchr(BELL, &bytes[from..])",
        "(from..bytes.len()).map(|index| usize::from(bytes[index] == BELL)).sum::<usize>()",
        "let end = Some(&bytes[index..]).into_iter().flatten().position(|byte| URL_USERINFO_DELIMITERS.contains(byte));",
        "let end = String::from_utf8_lossy(&bytes[index..]).find(['/', '@']);",
        "let line = work::find(bytes, index, |at| bytes[at] == b'=').map_or(\"\", |at| &text[at..]).lines().next();",
        "let label = work::find(bytes, 0, |at| bytes[at] == b'\\n').map_or(text, |end| &text[..end]).chars().count();",
        "let end = (index..bytes.len()).map(|at| (bytes[at] != b'@', at)).min();",
        "let widest = (index..bytes.len()).max_by_key(|at| bytes[*at]);",
        "let same = bytes[a..b] == bytes[c..d];",
        "let same = bytes[a..b]\n    == bytes[c..d];",
        "let ascii = bytes[index..].is_ascii();",
        "let valid = std::str::from_utf8(&bytes[index..]).is_ok();",
        "let copy = text[index..].to_owned();",
        "let lower = text[index..].to_ascii_lowercase();",
        "let number = text[index..].parse::<u64>();",
        "let closed = bytes[at..].starts_with(closing.as_bytes());",
        "match bytes.get(at) { Some(b'\\n') | None => at, Some(_) => line_end(bytes, at + 1) }",
        "let end = Some(&bytes[index..])\n    .into_iter()\n    .flatten()\n    .position(|byte| URL_USERINFO_DELIMITERS.contains(byte))\n    .map_or(bytes.len(), |length| index + length);",
        "fn walk(bytes: &[u8]) -> usize { walk(&bytes[1..]) }",
        "fn ping(bytes: &[u8]) -> usize { pong(bytes) }\nfn pong(bytes: &[u8]) -> usize { ping(bytes) }",
        "let found = QUOTES.iter().chain(bytes).count();",
        "let found = PEM_BEGIN.contains(&text[at..]);",
        "let found = Walker.find(bytes);",
        "let same = bytes == other;",
        "output += &text[at..];",
        "let copy = text.to_string();",
        "let label = format!(\"{text}\");",
        "use crate::helper::walk;",
        "use std::str::from_utf8;",
        "use super::super::helper::*;",
        "crate::helper::walk(bytes)",
        "super::value::walk(bytes)",
        "crate::redact::helper::walk(bytes)",
        "mod helper;",
        "#[path = \"../helper.rs\"]\nmod scanner;",
        "static TEXT: Mutex<String> = Mutex::new(String::new());",
        "const CONTAINS: fn(&[u8], &u8) -> bool = <[u8]>::contains;\nlet found = CONTAINS(bytes, &b'x');",
        "let found = (self.walk)(bytes);",
        "let found = WALKERS[0](bytes);",
    ] {
        assert!(!refusals_of(code).is_empty(), "{code} was not refused");
    }
}

#[test]
fn allows_walks_through_work_and_over_constants() {
    for code in [
        "work::find_map(bytes, from, |index| string_terminator_end(bytes, index))",
        "crate::work::find(bytes, from, |index| bytes[index] == BELL)",
        "work::occurrences(text, closing).find(|at| text[at..].starts_with(PEM_DASHES))",
        "let end = work::run(bytes, from, |byte| CharacterSet::Token.contains(byte));",
        "let end = work::find(bytes, at, |index| bytes[index] == BELL).unwrap_or(bytes.len());",
        "QUOTES.contains(&byte)",
        "INVISIBLE_CHARACTERS[range].contains(&character)",
        "CREDENTIALS.iter().find_map(|credential| credential.end_at(bytes, index))",
        "bytes[index..].starts_with(URL_SCHEME_SEPARATOR)",
        "bytes[at..].starts_with(&STRING_TERMINATOR)",
        "let line = text.strip_suffix('\\r').unwrap_or(text);",
        "output.push_str(REDACTED);",
        "let end = bytes.len().max(at);",
        "let end = CharacterSet::Token.run(bytes, at);",
        "let terminators = Terminators::new(bytes);",
        "let previous = at.checked_sub(1).map(|previous| bytes[previous]);",
        "if bytes.get(at) == Some(&b'\\n') { return at; }",
        "use crate::redact::character_set::CharacterSet;",
        "use std::borrow::Cow;",
        "const FOR: &str = \"for (x) in .iter() while\";",
        "let found = 'x'; // for each byte, while it lasts",
        "let separator = r#\"\\\"#; /* while */ let found = bytes.get(at);",
    ] {
        let refusals = refusals_of(code);
        assert!(
            refusals.is_empty(),
            "{code} was refused: {}",
            refusals.join("; ")
        );
    }
}
