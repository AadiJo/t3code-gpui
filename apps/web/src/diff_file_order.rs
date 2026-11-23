//! Review order for changed files. Ports `packages/client-runtime/src/diffFileOrder.ts`:
//! dependencies before dependents, tests after their source, generated files
//! last, then regrouped so each directory stays contiguous and a file tree
//! built from [`diff_file_tree_positions`] lists files in the same order.

use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

use regex::Regex;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tier {
    Source,
    Test,
    Generated,
}

const GENERATED_FILE_NAMES: &[&str] = &[
    "pnpm-lock.yaml",
    "yarn.lock",
    "package-lock.json",
    "bun.lockb",
    "Cargo.lock",
    "go.sum",
    "composer.lock",
    "Gemfile.lock",
];
const GENERATED_DIRECTORIES: &[&str] = &["__snapshots__", "__generated__", "dist", "build", "vendor"];
const TEST_DIRECTORIES: &[&str] = &["__tests__", "tests", "test"];
// `.mts` also ends in `.ts`, so longer extensions come first.
const MODULE_EXTENSIONS: &[&str] = &[".mts", ".cts", ".mjs", ".cjs", ".tsx", ".jsx", ".ts", ".js"];

static GENERATED_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.generated\.").unwrap());
static TEST_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.(?:test|spec)\.").unwrap());
static TEST_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.(?:test|spec)\..*$").unwrap());
static IMPORT_SPECIFIER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:\bfrom|\bimport|\brequire\s*\()\s*\(?\s*["']([^"']+)["']"#).unwrap());

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn directories(path: &str) -> impl Iterator<Item = &str> {
    let mut segments: Vec<&str> = path.split('/').collect();
    segments.pop();
    segments.into_iter()
}

fn tier(path: &str) -> Tier {
    let name = file_name(path);
    if GENERATED_FILE_NAMES.contains(&name)
        || name.ends_with(".snap")
        || name.ends_with(".min.js")
        || name.ends_with(".min.css")
        || GENERATED_NAME.is_match(name)
        || directories(path).any(|segment| GENERATED_DIRECTORIES.contains(&segment))
    {
        return Tier::Generated;
    }
    if TEST_NAME.is_match(name) || directories(path).any(|segment| TEST_DIRECTORIES.contains(&segment)) {
        return Tier::Test;
    }
    Tier::Source
}

fn strip_extension(path: &str) -> &str {
    let dot = path.rfind('.');
    let slash = path.rfind('/');
    match (dot, slash) {
        (Some(dot), Some(slash)) if dot > slash => &path[..dot],
        (Some(dot), None) => &path[..dot],
        _ => path,
    }
}

fn base_name(path: &str) -> &str {
    strip_extension(file_name(path))
}

fn strip_module_extension(path: &str) -> &str {
    MODULE_EXTENSIONS
        .iter()
        .find_map(|extension| path.strip_suffix(extension))
        .unwrap_or(path)
}

fn module_name(path: &str) -> &str {
    strip_module_extension(file_name(path))
}

fn resolve_relative(from_path: &str, specifier: &str) -> String {
    let mut segments: Vec<&str> = directories(from_path).collect();
    for part in specifier.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

fn imported_paths(
    path: &str,
    lines: &[&str],
    by_module_path: &HashMap<String, String>,
    by_module_name: &HashMap<String, String>,
) -> HashSet<String> {
    let mut imported = HashSet::new();
    for line in lines {
        for capture in IMPORT_SPECIFIER.captures_iter(line) {
            // Bundler queries such as `?worker` or `?url` name the same file.
            let specifier = capture[1].split('?').next().unwrap_or_default();
            let resolved_owned;
            let resolved = if specifier.starts_with('.') {
                resolved_owned = resolve_relative(path, specifier);
                strip_module_extension(&resolved_owned)
            } else {
                strip_module_extension(specifier)
            };
            let target = by_module_path
                .get(resolved)
                .or_else(|| by_module_path.get(&format!("{resolved}/index")))
                .or_else(|| by_module_name.get(module_name(resolved)));
            if let Some(target) = target.filter(|target| target.as_str() != path) {
                imported.insert(target.clone());
            }
        }
    }
    imported
}

fn order_by_imports(paths: &[String], imports: &HashMap<String, HashSet<String>>) -> Vec<String> {
    let mut remaining: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let mut sorted: Vec<&str> = paths.iter().map(String::as_str).collect();
    sorted.sort();
    let mut ordered = Vec::with_capacity(paths.len());
    while !remaining.is_empty() {
        let candidates: Vec<&str> = sorted.iter().copied().filter(|path| remaining.contains(path)).collect();
        let next = candidates
            .iter()
            .copied()
            .find(|path| {
                imports
                    .get(*path)
                    .is_none_or(|deps| deps.iter().all(|dep| !remaining.contains(dep.as_str())))
            })
            .unwrap_or(candidates[0]);
        remaining.remove(next);
        ordered.push(next.to_owned());
    }
    ordered
}

// `a.logic.test.ts` covers `a.logic.ts` while `a.browser.test.ts` covers `a.ts`, so the
// longest dotted prefix that names a source file wins.
fn tested_source_position(path: &str, source_positions: &HashMap<String, usize>) -> usize {
    let mut name = TEST_SUFFIX.replace(file_name(path), "").into_owned();
    loop {
        if let Some(position) = source_positions.get(&name) {
            return *position;
        }
        match name.rfind('.') {
            Some(dot) if dot > 0 => name.truncate(dot),
            _ => return usize::MAX,
        }
    }
}

/// Where each file, and each directory (with a trailing slash), first appears in `paths`.
pub fn diff_file_tree_positions<'a>(paths: impl IntoIterator<Item = &'a str>) -> HashMap<String, usize> {
    let mut positions = HashMap::new();
    for (index, path) in paths.into_iter().enumerate() {
        positions.insert(path.to_owned(), index);
        let mut directory = String::new();
        for segment in directories(path) {
            directory.push_str(segment);
            directory.push('/');
            positions.entry(directory.clone()).or_insert(index);
        }
    }
    positions
}

fn order_like_tree<F>(files: Vec<F>, path_of: impl Fn(&F) -> &str) -> Vec<F> {
    let positions = diff_file_tree_positions(files.iter().map(&path_of));
    let mut keyed: Vec<(Vec<usize>, F)> = files
        .into_iter()
        .map(|file| {
            let path = path_of(&file);
            let mut key = Vec::new();
            let mut directory = String::new();
            for segment in directories(path) {
                directory.push_str(segment);
                directory.push('/');
                key.push(positions[&directory]);
            }
            key.push(positions[path]);
            (key, file)
        })
        .collect();
    keyed.sort_by(|(left, _), (right, _)| left.cmp(right));
    keyed.into_iter().map(|(_, file)| file).collect()
}

/// Review order shared by every client. `lines_of` yields the file's added and
/// deleted lines, scanned for imports.
pub fn order_diff_files_for_review<F>(
    files: Vec<F>,
    path_of: impl Fn(&F) -> &str,
    lines_of: impl Fn(&F) -> Vec<&str>,
) -> Vec<F> {
    // A type change (file to symlink) is a delete and an add under one path; keep both.
    let mut path_order: Vec<String> = Vec::new();
    let mut by_path: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, file) in files.iter().enumerate() {
        let path = path_of(file).to_owned();
        by_path.entry(path.clone()).or_insert_with(|| {
            path_order.push(path.clone());
            Vec::new()
        });
        by_path.get_mut(&path).expect("inserted").push(index);
    }
    let tiers: HashMap<&str, Tier> = path_order.iter().map(|path| (path.as_str(), tier(path))).collect();
    let source_paths: Vec<String> = path_order.iter().filter(|p| tiers[p.as_str()] == Tier::Source).cloned().collect();

    let mut by_module_path = HashMap::new();
    let mut by_module_name = HashMap::new();
    let mut ambiguous = HashSet::new();
    for path in &source_paths {
        by_module_path.insert(strip_module_extension(path).to_owned(), path.clone());
        let name = module_name(path).to_owned();
        if by_module_name.contains_key(&name) {
            ambiguous.insert(name.clone());
        }
        by_module_name.insert(name, path.clone());
    }
    for name in ambiguous {
        by_module_name.remove(&name);
    }
    let imports: HashMap<String, HashSet<String>> = source_paths
        .iter()
        .map(|path| {
            let lines: Vec<&str> = by_path[path].iter().flat_map(|&i| lines_of(&files[i])).collect();
            (path.clone(), imported_paths(path, &lines, &by_module_path, &by_module_name))
        })
        .collect();
    let ordered_source = order_by_imports(&source_paths, &imports);

    let mut source_positions = HashMap::new();
    for (index, path) in ordered_source.iter().enumerate() {
        source_positions.entry(base_name(path).to_owned()).or_insert(index);
    }
    let mut ordered_tests: Vec<String> = path_order.iter().filter(|p| tiers[p.as_str()] == Tier::Test).cloned().collect();
    ordered_tests.sort_by(|left, right| {
        tested_source_position(left, &source_positions)
            .cmp(&tested_source_position(right, &source_positions))
            .then_with(|| left.cmp(right))
    });
    let mut ordered_generated: Vec<String> =
        path_order.iter().filter(|p| tiers[p.as_str()] == Tier::Generated).cloned().collect();
    ordered_generated.sort();

    let order: Vec<usize> = ordered_source
        .iter()
        .chain(&ordered_tests)
        .chain(&ordered_generated)
        .flat_map(|path| by_path[path].iter().copied())
        .collect();
    let mut slots: Vec<Option<F>> = files.into_iter().map(Some).collect();
    let tiered: Vec<F> = order.into_iter().filter_map(|index| slots[index].take()).collect();
    order_like_tree(tiered, path_of)
}
