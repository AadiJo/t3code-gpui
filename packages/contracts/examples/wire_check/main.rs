//! Decodes real encoded values with the generated types, encodes them again and compares.
//!
//!   cargo run -p t3code-contracts --example wire_check -- fixtures [report dir]
//!   cargo run -p t3code-contracts --example wire_check -- id-table [report dir]
//!   cargo run -p t3code-contracts --example wire_check -- frames <transcript> [report dir]
//!
//! `fixtures` checks every value in `schema/wire-fixtures.json` (sampled by effect's Arbitrary and
//! encoded with the TypeScript codec). `id-table` expands each sample in
//! `schema/id-table-fixtures.json` and compares it with the same value encoded without the table.
//! `frames` checks every RPC payload, chunk value, exit value
//! and typed error in a WebSocket transcript: one frame per line, JSON starting at the first `{`.
//! Each mode prints a summary and, with a report dir, writes `<mode>-report.json` and `.md`.
//!
//! Two differences count as equal, because the TypeScript decoder treats them as equal:
//! numbers compare as f64 (JavaScript has one number type), and an optional key that arrived as
//! `null` may be omitted (Effect decodes a missing key and `null` the same way). The report counts
//! every use of either rule per type.

mod registry;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, value::RawValue};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use t3code_contracts::rpc::RpcMethod;

/// Which schema of an RPC a value belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Part {
    Payload,
    Success,
    Error,
}

/// Decodes `text` as `T` and encodes the result.
pub fn roundtrip<T: DeserializeOwned + Serialize>(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::to_value(serde_json::from_str::<T>(text)?)
}

/// Decodes `text` as one part of RPC `M` and encodes the result.
pub fn roundtrip_part<M: RpcMethod>(part: Part, text: &str) -> Result<Value, serde_json::Error>
where
    M::Payload: DeserializeOwned,
    M::Success: Serialize,
    M::Error: Serialize,
{
    match part {
        Part::Payload => roundtrip::<M::Payload>(text),
        Part::Success => roundtrip::<M::Success>(text),
        Part::Error => roundtrip::<M::Error>(text),
    }
}

/// Per-type result.
#[derive(Debug, Default, Serialize)]
struct TypeResult {
    values: usize,
    passed: usize,
    /// Numbers equal as f64 but written differently (e.g. `1` and `1.0`).
    number_repr: usize,
    /// Optional keys that arrived as null and were omitted on encode.
    null_omitted: usize,
    failures: Vec<String>,
}

#[derive(Default)]
struct Notes {
    number_repr: usize,
    null_omitted: usize,
}

/// Compares the original value with the re-encoded one; Err names the first difference.
fn compare(orig: &Value, back: &Value, path: &str, notes: &mut Notes) -> Result<(), String> {
    match (orig, back) {
        (Value::Number(a), Value::Number(b)) => {
            if a == b {
                Ok(())
            } else if a.as_f64() == b.as_f64() {
                notes.number_repr += 1;
                Ok(())
            } else {
                Err(format!("{path}: number {a} became {b}"))
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            for (k, va) in a {
                let p = format!("{path}.{k}");
                match b.get(k) {
                    Some(vb) => compare(va, vb, &p, notes)?,
                    None if va.is_null() => notes.null_omitted += 1,
                    None => return Err(format!("{p}: dropped {}", trunc(va))),
                }
            }
            match b.keys().find(|k| !a.contains_key(*k)) {
                Some(k) => Err(format!("{path}.{k}: added {}", trunc(&b[k]))),
                None => Ok(()),
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => a
            .iter()
            .zip(b)
            .enumerate()
            .try_for_each(|(i, (x, y))| compare(x, y, &format!("{path}[{i}]"), notes)),
        _ if orig == back => Ok(()),
        _ => Err(format!("{path}: {} became {}", trunc(orig), trunc(back))),
    }
}

fn trunc(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 160 { format!("{}…", s.chars().take(160).collect::<String>()) } else { s }
}

/// Checks one value and records the outcome under its type.
fn check(result: &mut TypeResult, text: &str, decoded: Result<Value, serde_json::Error>) {
    result.values += 1;
    let orig: Value = serde_json::from_str(text).expect("input is JSON");
    let outcome = decoded.map_err(|e| format!("decode: {e}")).and_then(|back| {
        let mut notes = Notes::default();
        compare(&orig, &back, "$", &mut notes)?;
        Ok(notes)
    });
    match outcome {
        Ok(notes) => {
            result.passed += 1;
            result.number_repr += notes.number_repr;
            result.null_omitted += notes.null_omitted;
        }
        Err(e) => {
            if result.failures.len() < 5 {
                result.failures.push(format!("{e}\n    input: {}", trunc(&orig)));
            }
        }
    }
}

fn check_fixtures() -> BTreeMap<String, TypeResult> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/wire-fixtures.json");
    let text = std::fs::read_to_string(&path).expect("read wire-fixtures.json");
    let fixtures: BTreeMap<String, Vec<Box<RawValue>>> = serde_json::from_str(&text).expect("parse fixtures");
    let mut results = BTreeMap::new();
    for (name, values) in &fixtures {
        let result: &mut TypeResult = results.entry(name.clone()).or_default();
        for raw in values {
            let decoded = registry::by_name(name, raw.get()).expect("registry covers every fixture type");
            check(result, raw.get(), decoded);
        }
    }
    results
}

/// Expands each id-table sample and compares it with the same value encoded without the table.
fn check_id_tables() -> BTreeMap<String, TypeResult> {
    use t3code_contracts::orchestration_v2_id_table as id_table;
    #[derive(serde::Deserialize)]
    struct Pair {
        plain: Box<RawValue>,
        compact: Box<RawValue>,
    }
    fn expand<T: id_table::RecordIdTable + Serialize>(text: &str) -> Result<Value, serde_json::Error> {
        serde_json::to_value(id_table::from_str::<T>(text)?)
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/id-table-fixtures.json");
    let text = std::fs::read_to_string(&path).expect("read id-table-fixtures.json");
    let pairs: BTreeMap<String, Vec<Pair>> = serde_json::from_str(&text).expect("parse id-table fixtures");
    let mut results = BTreeMap::new();
    for (name, pairs) in &pairs {
        let result: &mut TypeResult = results.entry(name.clone()).or_default();
        for pair in pairs {
            let decoded = match name.as_str() {
                "OrchestrationV2ThreadBoundedSnapshot" => expand::<t3code_contracts::OrchestrationV2ThreadBoundedSnapshot>(pair.compact.get()),
                "OrchestrationV2ThreadHistoryPage" => expand::<t3code_contracts::OrchestrationV2ThreadHistoryPage>(pair.compact.get()),
                "OrchestrationV2ThreadStreamItem" => expand::<t3code_contracts::OrchestrationV2ThreadStreamItem>(pair.compact.get()),
                other => panic!("no id-table type {other}"),
            };
            check(result, pair.plain.get(), decoded);
        }
    }
    results
}

/// Checks every RPC value in a transcript, matching responses to requests by id.
fn check_frames(transcript: &Path) -> BTreeMap<String, TypeResult> {
    let text = std::fs::read_to_string(transcript).expect("read transcript");
    let mut tags: HashMap<String, String> = HashMap::new();
    let mut results: BTreeMap<String, TypeResult> = BTreeMap::new();
    let mut skipped = 0usize;
    let record = |tag: &str, part: Part, value: &Value, results: &mut BTreeMap<String, TypeResult>| {
        let text = value.to_string();
        if let Some(decoded) = registry::by_tag(tag, part, &text) {
            check(results.entry(format!("{tag} {part:?}")).or_default(), &text, decoded);
        }
    };
    for line in text.lines() {
        let Some(start) = line.find('{') else { continue };
        let Ok(frame) = serde_json::from_str::<Map<String, Value>>(&line[start..]) else {
            skipped += 1;
            continue;
        };
        let id = |key: &str| frame.get(key).map(Value::to_string).unwrap_or_default();
        match frame.get("_tag").and_then(Value::as_str) {
            Some("Request") => {
                let tag = frame["tag"].as_str().unwrap_or_default().to_owned();
                if let Some(payload) = frame.get("payload") {
                    record(&tag, Part::Payload, payload, &mut results);
                }
                tags.insert(id("id"), tag);
            }
            Some("Chunk") => {
                let Some(tag) = tags.get(&id("requestId")) else { continue };
                for v in frame["values"].as_array().into_iter().flatten() {
                    record(tag, Part::Success, v, &mut results);
                }
            }
            Some("Exit") => {
                let Some(tag) = tags.get(&id("requestId")) else { continue };
                let exit = &frame["exit"];
                match exit["_tag"].as_str() {
                    Some("Success") => {
                        let value = exit.get("value").unwrap_or(&Value::Null);
                        // A stream ends with a null exit value; its chunks were checked already.
                        if !value.is_null() {
                            record(tag, Part::Success, value, &mut results);
                        }
                    }
                    Some("Failure") => {
                        for cause in exit["cause"].as_array().into_iter().flatten() {
                            if cause["_tag"] == "Fail" {
                                record(tag, Part::Error, &cause["error"], &mut results);
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if skipped > 0 {
        eprintln!("skipped {skipped} lines that are not one complete JSON frame");
    }
    results
}

fn write_report(mode: &str, results: &BTreeMap<String, TypeResult>, out: Option<&Path>) {
    let total = results.len();
    let passed = results.values().filter(|r| r.failures.is_empty() && r.passed == r.values).count();
    let values: usize = results.values().map(|r| r.values).sum();
    let values_passed: usize = results.values().map(|r| r.passed).sum();
    let mut md = format!(
        "# wire_check {mode}\n\nTypes: {passed}/{total} pass. Values: {values_passed}/{values} pass.\n\n| Type | Values | Pass | Number repr | Null omitted | Result |\n|---|---:|---:|---:|---:|---|\n"
    );
    for (name, r) in results {
        let ok = r.failures.is_empty();
        let _ = writeln!(
            md,
            "| `{name}` | {} | {} | {} | {} | {} |",
            r.values,
            r.passed,
            r.number_repr,
            r.null_omitted,
            if ok { "pass" } else { "FAIL" }
        );
    }
    for (name, r) in results.iter().filter(|(_, r)| !r.failures.is_empty()) {
        let _ = writeln!(md, "\n## {name}\n");
        for f in &r.failures {
            let _ = writeln!(md, "- {f}");
        }
    }
    println!("{mode}: types {passed}/{total} pass, values {values_passed}/{values} pass");
    for (name, r) in results.iter().filter(|(_, r)| !r.failures.is_empty()) {
        println!("  FAIL {name}: {}", r.failures[0]);
    }
    if let Some(dir) = out {
        std::fs::create_dir_all(dir).expect("create report dir");
        std::fs::write(dir.join(format!("{mode}-report.md")), md).expect("write report");
        let json = serde_json::to_string_pretty(results).expect("encode report");
        std::fs::write(dir.join(format!("{mode}-report.json")), json).expect("write report");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("fixtures") => {
            let out = args.get(1).map(PathBuf::from);
            write_report("fixtures", &check_fixtures(), out.as_deref());
        }
        Some("id-table") => {
            let out = args.get(1).map(PathBuf::from);
            write_report("id-table", &check_id_tables(), out.as_deref());
        }
        Some("frames") => {
            let transcript = PathBuf::from(args.get(1).expect("frames <transcript> [report dir]"));
            let out = args.get(2).map(PathBuf::from);
            write_report("frames", &check_frames(&transcript), out.as_deref());
        }
        _ => {
            eprintln!("usage: wire_check fixtures|id-table [report dir] | frames <transcript> [report dir]");
            std::process::exit(2);
        }
    }
}
