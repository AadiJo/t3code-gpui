//! Themes from Open VSX. Ports `openVsxThemes.ts`: search the registry for
//! colour theme extensions with a permissive license, then download one,
//! check its SHA-256, open the VSIX safely, resolve each contributed theme
//! (following `include`), convert it with [`crate::vscode_theme_import`],
//! pair light and dark variants and give every theme a stable id.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::vscode_theme_import::{
    ImportError, ParsedTheme, humanize_theme_name, is_vscode_theme_file, pair_vscode_themes, parse_theme_file,
    parse_vscode_theme_file, strip_jsonc, theme_id_from_name,
};

const OPEN_VSX_SEARCH_URL: &str = "https://open-vsx.org/api/-/search";
const MAX_VSIX_BYTES: usize = 20 * 1024 * 1024;
const MAX_THEME_BYTES: u64 = 256 * 1024;
const MAX_ZIP_ENTRIES: usize = 5_000;
const MAX_UNCOMPRESSED_BYTES: u64 = 100 * 1024 * 1024;
const MAX_COMPRESSION_RATIO: u64 = 200;
const MAX_THEMES_PER_EXTENSION: usize = 40;
const MAX_INCLUDE_DEPTH: usize = 8;
const MAX_PACKAGE_PATH_LENGTH: usize = 1_024;
const MAX_COLOR_VALUE_LENGTH: usize = 128;
const MAX_RESOLVED_THEME_FILES: usize = MAX_THEMES_PER_EXTENSION * MAX_INCLUDE_DEPTH;
const SUPPORTED_LICENSES: [&str; 9] =
    ["0BSD", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "CC0-1.0", "ISC", "MIT", "MPL-2.0", "Unlicense"];
const USED_WORKBENCH_COLORS: [&str; 46] = [
    "activityBar.background", "activityBarBadge.background", "badge.background", "button.background", "button.foreground",
    "contrastBorder", "descriptionForeground", "disabledForeground", "dropdown.background", "dropdown.border",
    "editor.background", "editor.foreground", "editor.findMatchBackground", "editor.findMatchForeground",
    "editor.findMatchHighlightBackground", "editor.findMatchHighlightForeground", "editor.selectionBackground",
    "editorCursor.foreground", "editorError.foreground", "editorGroup.border", "editorPane.background",
    "editorWarning.foreground", "editorWidget.background", "errorForeground", "focusBorder", "foreground", "input.border",
    "input.placeholderForeground", "list.activeSelectionBackground", "list.hoverBackground",
    "list.inactiveSelectionBackground", "menu.background", "panel.background", "panel.border", "progressBar.background",
    "quickInput.background", "scrollbarSlider.background", "sideBar.background", "sideBar.border", "sideBar.foreground",
    "terminal.background", "terminal.foreground", "terminal.selectionBackground", "terminalCursor.foreground",
    "textCodeBlock.background", "textLink.foreground",
];

fn fail<T>(message: &str) -> Result<T, ImportError> {
    Err(ImportError(message.to_owned()))
}

/// One extension from a search, with the URLs the import needs.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenVsxThemeExtension {
    pub id: String,
    pub collection_id: String,
    pub name: String,
    pub publisher: String,
    pub description: String,
    pub download_count: u64,
    pub icon_url: Option<String>,
    pub manifest_url: String,
    pub sha256_url: String,
    pub vsix_url: String,
    pub version: String,
    pub license: String,
}

/// A theme imported from an extension, in its collection.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenVsxTheme {
    pub theme: ParsedTheme,
    pub collection_id: String,
    pub collection_label: String,
}

impl OpenVsxTheme {
    /// The T3 theme file to save, with its collection.
    pub fn to_file(&self) -> Value {
        let mut file = self.theme.to_file();
        if let Value::Object(map) = &mut file {
            map.insert("collection".into(), serde_json::json!({ "id": self.collection_id, "label": self.collection_label }));
        }
        file
    }
}

fn short_hash(value: &str) -> String {
    Sha256::digest(value.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect()
}

fn open_vsx_theme_id(extension_id: &str, source: &str) -> String {
    format!("ovx-theme-{}", short_hash(&format!("{extension_id}:{source}")))
}

fn open_vsx_collection_id(extension_id: &str) -> String {
    let normalized = format!("open-vsx:{}", extension_id.to_lowercase());
    let valid = normalized.len() <= 128
        && normalized.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | ':' | '-'));
    if valid { normalized } else { format!("open-vsx:{}", short_hash(extension_id)) }
}

fn trusted_url(value: Option<&Value>) -> Option<String> {
    let url = url::Url::parse(value?.as_str()?).ok()?;
    (url.scheme() == "https" && url.host_str()?.eq_ignore_ascii_case("open-vsx.org")).then(|| url.to_string())
}

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).map(str::trim).unwrap_or_default()
}

/// `extensionFromDetail`: `None` for an unsupported license.
pub fn extension_from_detail(value: &Value) -> Result<Option<OpenVsxThemeExtension>, ImportError> {
    let files = value.get("files").filter(|f| f.is_object());
    let Some(files) = files else { return fail("Open VSX returned malformed theme details.") };
    let namespace = str_field(value, "namespace");
    let extension_name = str_field(value, "name");
    let display = Some(str_field(value, "displayName")).filter(|d| !d.is_empty()).unwrap_or(extension_name);
    let version = str_field(value, "version");
    let license = str_field(value, "license");
    let (Some(manifest_url), Some(sha256_url), Some(vsix_url)) =
        (trusted_url(files.get("manifest")), trusted_url(files.get("sha256")), trusted_url(files.get("download")))
    else {
        return fail("Open VSX returned malformed theme details.");
    };
    if namespace.is_empty() || extension_name.is_empty() || version.is_empty() {
        return fail("Open VSX returned malformed theme details.");
    }
    if !SUPPORTED_LICENSES.contains(&license) {
        return Ok(None);
    }
    let id = format!("{namespace}.{extension_name}");
    Ok(Some(OpenVsxThemeExtension {
        collection_id: open_vsx_collection_id(&id),
        id,
        name: display.to_owned(),
        publisher: namespace.to_owned(),
        description: value.get("description").and_then(Value::as_str).unwrap_or_default().to_owned(),
        download_count: value.get("downloadCount").and_then(Value::as_u64).unwrap_or(0),
        icon_url: trusted_url(files.get("icon")),
        manifest_url,
        sha256_url,
        vsix_url,
        version: version.to_owned(),
        license: license.to_owned(),
    }))
}

fn parse_jsonc_object(source: &str, description: &str) -> Result<Map<String, Value>, ImportError> {
    match serde_json::from_str::<Value>(&strip_jsonc(source)) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(ImportError(format!("{description} is not valid JSON."))),
    }
}

fn theme_contributions(manifest: &Map<String, Value>) -> Vec<Map<String, Value>> {
    manifest
        .get("contributes")
        .and_then(|c| c.get("themes"))
        .and_then(Value::as_array)
        .map(|themes| themes.iter().filter_map(|t| t.as_object().cloned()).collect())
        .unwrap_or_default()
}

fn license_matches(manifest: &Map<String, Value>, license: &str) -> bool {
    manifest.get("license").and_then(Value::as_str).is_some_and(|l| l.trim().eq_ignore_ascii_case(license))
}

/// `searchOpenVsxThemes`: up to 8 theme extensions with a permissive
/// license whose manifest contributes colour themes.
pub async fn search_open_vsx_themes(query: &str) -> Result<Vec<OpenVsxThemeExtension>, ImportError> {
    let text = query.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let client = t3code_client_runtime::rpc::http::http_client();
    let get_json = |url: String| {
        let client = client.clone();
        async move {
            let response = client.get(url).timeout(std::time::Duration::from_secs(10)).send().await.ok()?;
            if !response.status().is_success() {
                return None;
            }
            response.json::<Value>().await.ok()
        }
    };
    let mut url = url::Url::parse(OPEN_VSX_SEARCH_URL).expect("search url");
    url.query_pairs_mut()
        .append_pair("query", text)
        .append_pair("category", "Themes")
        .append_pair("sortBy", "downloadCount")
        .append_pair("sortOrder", "desc")
        .append_pair("size", "16");
    let Some(value) = get_json(url.to_string()).await else { return fail("Open VSX search is unavailable right now.") };
    let Some(extensions) = value.get("extensions").and_then(Value::as_array) else {
        return fail("Open VSX returned an unreadable search response.");
    };
    let identities: Vec<(String, String)> = extensions
        .iter()
        .filter_map(|e| Some((e.get("namespace")?.as_str()?.to_owned(), e.get("name")?.as_str()?.to_owned())))
        .take(16)
        .collect();
    let details = futures::future::join_all(identities.iter().map(|(namespace, name)| {
        let get_json = get_json.clone();
        let client = client.clone();
        async move {
            let detail = get_json(format!("https://open-vsx.org/api/{namespace}/{name}")).await?;
            let extension = extension_from_detail(&detail).ok()??;
            let manifest = client.get(&extension.manifest_url).send().await.ok()?.text().await.ok()?;
            let manifest = parse_jsonc_object(&manifest, "Extension manifest").ok()?;
            (!theme_contributions(&manifest).is_empty() && license_matches(&manifest, &extension.license)).then_some(extension)
        }
    }))
    .await;
    Ok(details.into_iter().flatten().take(8).collect())
}

/// `importOpenVsxThemeExtension`: downloads, verifies and converts.
pub async fn import_open_vsx_theme_extension(extension: &OpenVsxThemeExtension) -> Result<Vec<OpenVsxTheme>, ImportError> {
    let client = t3code_client_runtime::rpc::http::http_client();
    let fetch = |url: &str| {
        let request = client.get(url);
        async move {
            let response = request.send().await.ok().filter(|r| r.status().is_success())?;
            response.bytes().await.ok()
        }
    };
    let Some(manifest) = fetch(&extension.manifest_url).await else {
        return fail("That Open VSX extension has no readable manifest.");
    };
    let manifest = parse_jsonc_object(&String::from_utf8_lossy(&manifest), "Extension manifest")?;
    let advertised = theme_contributions(&manifest).len();
    if advertised == 0 {
        return fail("That extension does not contain color themes.");
    }
    if advertised > MAX_THEMES_PER_EXTENSION {
        return fail("That extension contains too many color themes to import safely.");
    }
    let Some(package) = fetch(&extension.vsix_url).await else { return fail("That Open VSX theme could not be downloaded.") };
    if package.len() > MAX_VSIX_BYTES {
        return fail("That theme extension is too large to import safely.");
    }
    let Some(checksum) = fetch(&extension.sha256_url).await else { return fail("That Open VSX theme has no readable checksum.") };
    let expected = String::from_utf8_lossy(&checksum).split_whitespace().next().unwrap_or_default().to_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return fail("That Open VSX theme has an invalid checksum.");
    }
    let actual: String = Sha256::digest(&package).iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return fail("That Open VSX theme failed its integrity check.");
    }
    import_package(extension, &package)
}

/// `normalizePackagePath`: a path inside `extension/`, relative to a file.
fn normalize_package_path(path: &str, relative_to: &str) -> Result<String, ImportError> {
    let bytes = path.as_bytes();
    if path.len() > MAX_PACKAGE_PATH_LENGTH
        || path.contains('\0')
        || path.starts_with('/')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return fail("Theme path is not a safe relative package path.");
    }
    let normalized = path.replace('\\', "/");
    let mut segments: Vec<&str> = relative_to.split('/').collect();
    segments.pop();
    for segment in normalized.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.len() <= 1 {
                    return fail("Theme path escapes the extension package.");
                }
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    if segments.first() != Some(&"extension") {
        segments.insert(0, "extension");
    }
    Ok(segments.join("/"))
}

/// The files of a VSIX, checked for the web's size, count and ratio limits.
struct Package {
    files: HashMap<String, (u64, Vec<u8>)>,
}

impl Package {
    fn open(bytes: &[u8]) -> Result<Self, ImportError> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|_| ImportError("That Open VSX extension package could not be opened.".into()))?;
        if archive.len() > MAX_ZIP_ENTRIES {
            return fail("That extension package has too many files.");
        }
        let mut total = 0u64;
        let mut files = HashMap::new();
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|_| ImportError("That Open VSX extension package could not be opened.".into()))?;
            let (compressed, size) = (entry.compressed_size(), entry.size());
            total += size;
            if total > MAX_UNCOMPRESSED_BYTES {
                return fail("That extension package expands beyond the safe import limit.");
            }
            if size > 0 && (compressed == 0 || size / compressed.max(1) > MAX_COMPRESSION_RATIO) {
                return fail("That extension package has an unsafe compression ratio.");
            }
            let name = entry.name().to_owned();
            normalize_package_path(&name, "extension/")?;
            if entry.is_dir() || size > MAX_THEME_BYTES {
                files.insert(name, (size, Vec::new()));
                continue;
            }
            let mut contents = Vec::with_capacity(size as usize);
            entry.by_ref().take(MAX_THEME_BYTES + 1).read_to_end(&mut contents).map_err(|_| {
                ImportError("That Open VSX extension package could not be opened.".into())
            })?;
            files.insert(name, (size, contents));
        }
        Ok(Self { files })
    }

    fn text(&self, path: &str, description: &str) -> Result<String, ImportError> {
        let Some((size, contents)) = self.files.get(path) else {
            return Err(ImportError(format!("{description} is missing from the extension package.")));
        };
        if *size > MAX_THEME_BYTES {
            return Err(ImportError(format!("{description} is too large.")));
        }
        Ok(String::from_utf8_lossy(contents).into_owned())
    }
}

fn sanitize_theme_object(value: &Map<String, Value>) -> Map<String, Value> {
    let mut colors = Map::new();
    if let Some(raw) = value.get("colors").and_then(Value::as_object) {
        for (key, color) in raw {
            if USED_WORKBENCH_COLORS.contains(&key.as_str())
                && color.as_str().is_some_and(|c| c.len() <= MAX_COLOR_VALUE_LENGTH)
            {
                colors.insert(key.clone(), color.clone());
            }
        }
    }
    let mut out = Map::new();
    if let Some(include) = value.get("include").filter(|i| i.is_string()) {
        out.insert("include".into(), include.clone());
    }
    out.insert("colors".into(), Value::Object(colors));
    out
}

fn load_theme_object(
    package: &Package,
    path: &str,
    cache: &mut HashMap<String, Map<String, Value>>,
    budget: &mut usize,
    ancestors: &HashSet<String>,
) -> Result<Map<String, Value>, ImportError> {
    if ancestors.len() >= MAX_INCLUDE_DEPTH {
        return fail("Theme includes are nested too deeply.");
    }
    if ancestors.contains(path) {
        return fail("Theme includes contain a cycle.");
    }
    if let Some(cached) = cache.get(path) {
        return Ok(cached.clone());
    }
    *budget += 1;
    if *budget > MAX_RESOLVED_THEME_FILES {
        return fail("That extension references too many theme files.");
    }
    let value = sanitize_theme_object(&parse_jsonc_object(&package.text(path, path)?, path)?);
    let Some(include) = value.get("include").and_then(Value::as_str) else {
        cache.insert(path.to_owned(), value.clone());
        return Ok(value);
    };
    let include_path = normalize_package_path(include, path)?;
    let mut next = ancestors.clone();
    next.insert(path.to_owned());
    let base = load_theme_object(package, &include_path, cache, budget, &next)?;
    let mut resolved = base.clone();
    for (key, v) in &value {
        resolved.insert(key.clone(), v.clone());
    }
    let mut colors = base.get("colors").and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(own) = value.get("colors").and_then(Value::as_object) {
        colors.extend(own.clone());
    }
    resolved.insert("colors".into(), Value::Object(colors));
    cache.insert(path.to_owned(), resolved.clone());
    Ok(resolved)
}

fn contribution_type(ui_theme: Option<&Value>) -> Option<&'static str> {
    match ui_theme?.as_str()? {
        "vs" => Some("light"),
        "vs-dark" => Some("dark"),
        "hc-black" => Some("hc-black"),
        "hc-light" => Some("hc-light"),
        _ => None,
    }
}

fn rename(theme: &ParsedTheme, name: &str) -> Option<ParsedTheme> {
    let name: String = name.chars().take(48).collect();
    let mut file = theme.to_file();
    let map = file.as_object_mut()?;
    map.remove("id");
    map.insert("name".into(), name.into());
    parse_theme_file(&file).ok()
}

/// `resolveThemeLabelCollisions`: themes sharing an id are renamed after
/// their file, then numbered.
fn resolve_label_collisions(entries: Vec<(ParsedTheme, Option<String>)>) -> Vec<ParsedTheme> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (theme, _) in &entries {
        *counts.entry(theme.id.clone()).or_default() += 1;
    }
    let relabelled: Vec<ParsedTheme> = entries
        .into_iter()
        .map(|(theme, source)| {
            if counts[&theme.id] < 2 {
                return theme;
            }
            let stem = source.map(|s| s.rsplit_once('.').map_or(s.clone(), |(stem, _)| stem.to_owned()));
            let from_file = stem.map(|s| humanize_theme_name(&s));
            from_file
                .filter(|f| f.to_lowercase() != theme.label.to_lowercase())
                .and_then(|f| rename(&theme, &f))
                .unwrap_or(theme)
        })
        .collect();
    let input_ids: HashSet<String> = relabelled.iter().map(|t| t.id.clone()).collect();
    let mut seen = HashSet::new();
    relabelled
        .into_iter()
        .map(|theme| {
            if seen.insert(theme.id.clone()) {
                return theme;
            }
            for suffix in 2..100 {
                let suffix_text = suffix.to_string();
                let mut base: Vec<char> = theme.label.chars().collect();
                let mut stem: String = base.iter().collect::<String>().trim_end().to_owned();
                let mut name = format!("{stem} {suffix_text}");
                let mut id = theme_id_from_name(&name);
                while name.chars().count() > 48 || id.rsplit('-').next() != Some(&suffix_text) || id == theme_id_from_name(&stem) {
                    if base.pop().is_none() {
                        break;
                    }
                    stem = base.iter().collect::<String>().trim_end().to_owned();
                    name = format!("{stem} {suffix_text}");
                    id = theme_id_from_name(&name);
                }
                if let Some(candidate) = rename(&theme, &name)
                    && !input_ids.contains(&candidate.id)
                    && seen.insert(candidate.id.clone())
                {
                    return candidate;
                }
            }
            theme
        })
        .collect()
}

/// The part of the import after the download: open the VSIX, check it
/// matches the extension, convert every contributed theme.
pub fn import_package(extension: &OpenVsxThemeExtension, bytes: &[u8]) -> Result<Vec<OpenVsxTheme>, ImportError> {
    let package = Package::open(bytes)?;
    let manifest = parse_jsonc_object(&package.text("extension/package.json", "Extension manifest")?, "Extension manifest")?;
    let publisher = manifest.get("publisher").and_then(Value::as_str).unwrap_or_default();
    let name = manifest.get("name").and_then(Value::as_str).unwrap_or_default();
    if !publisher.eq_ignore_ascii_case(&extension.publisher)
        || !format!("{publisher}.{name}").eq_ignore_ascii_case(&extension.id)
        || manifest.get("version").and_then(Value::as_str) != Some(extension.version.as_str())
    {
        return fail("That extension package does not match the selected Open VSX theme.");
    }
    if !license_matches(&manifest, &extension.license) {
        return fail("That extension package does not match its advertised license.");
    }
    let contributions = theme_contributions(&manifest);
    if contributions.is_empty() {
        return fail("That extension does not contain color themes.");
    }
    if contributions.len() > MAX_THEMES_PER_EXTENSION {
        return fail("That extension contains too many color themes to import safely.");
    }
    let mut parsed: Vec<(ParsedTheme, String, String)> = Vec::new();
    let mut failed = false;
    let mut cache = HashMap::new();
    let mut budget = 0;
    for contribution in &contributions {
        let Some(raw_path) = contribution.get("path").and_then(Value::as_str) else {
            failed = true;
            continue;
        };
        let result = (|| {
            let path = normalize_package_path(raw_path, "extension/")?;
            let mut value = load_theme_object(&package, &path, &mut cache, &mut budget, &HashSet::new())?;
            let label = contribution
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .unwrap_or(&extension.name);
            value.insert("displayName".into(), label.into());
            if let Some(kind) = contribution_type(contribution.get("uiTheme")) {
                value.insert("type".into(), kind.into());
            }
            let value = Value::Object(value);
            if !is_vscode_theme_file(&value) {
                return fail("not a VS Code color theme");
            }
            let source_name = path.rsplit('/').next().unwrap_or_default().to_owned();
            Ok((parse_vscode_theme_file(&value)?, source_name, path))
        })();
        match result {
            Ok(entry) => parsed.push(entry),
            Err(_) => failed = true,
        }
    }
    if failed {
        return fail("One or more color themes in that extension could not be imported safely.");
    }
    if parsed.is_empty() {
        return fail("That extension has no compatible color themes.");
    }
    let extension_id = extension.id.to_lowercase();
    let mut path_counts: HashMap<String, usize> = HashMap::new();
    for (_, _, path) in &parsed {
        *path_counts.entry(path.clone()).or_default() += 1;
    }
    let mut occurrences: HashMap<String, usize> = HashMap::new();
    let identities: Vec<String> = parsed
        .iter()
        .map(|(_, _, path)| {
            if path_counts[path] == 1 {
                return path.clone();
            }
            let occurrence = occurrences.entry(path.clone()).or_default();
            let identity = if *occurrence == 0 { path.clone() } else { format!("{path}\0{occurrence}") };
            *occurrence += 1;
            identity
        })
        .collect();
    let resolved: Vec<ParsedTheme> =
        resolve_label_collisions(parsed.into_iter().map(|(theme, source, _)| (theme, Some(source))).collect())
            .into_iter()
            .zip(&identities)
            .map(|(mut theme, identity)| {
                theme.id = open_vsx_theme_id(&extension_id, identity);
                theme
            })
            .collect();
    let by_id: BTreeMap<String, ParsedTheme> = resolved.iter().map(|t| (t.id.clone(), t.clone())).collect();
    let paired = pair_vscode_themes(resolved)
        .into_iter()
        .map(|mut theme| {
            // A pair takes the id of its two halves (`pairedId`).
            if !by_id.contains_key(&theme.id) && !theme.variants.is_empty() {
                let halves: Vec<&ParsedTheme> = by_id
                    .values()
                    .filter(|t| t.modes() == 1 && strip_mode(&t.label) == theme.label)
                    .collect();
                let mut ids: Vec<&str> = halves.iter().map(|t| t.id.as_str()).collect();
                ids.sort();
                theme.id = open_vsx_theme_id(&extension_id, &ids.join(":"));
            }
            theme
        })
        .collect::<Vec<_>>();
    let themes = resolve_label_collisions(paired.into_iter().map(|t| (t, None)).collect());
    let label: String = extension.name.chars().take(48).collect();
    Ok(themes
        .into_iter()
        .map(|theme| OpenVsxTheme { theme, collection_id: extension.collection_id.clone(), collection_label: label.clone() })
        .collect())
}

fn strip_mode(label: &str) -> String {
    label.split_whitespace().filter(|w| !w.eq_ignore_ascii_case("light") && !w.eq_ignore_ascii_case("dark")).collect::<Vec<_>>().join(" ")
}

/// For `theme-import-check`: a fixture `{ "extension": detail, "vsix": base64 }`
/// run through the import after the download.
pub fn check_fixture(source: &str) -> Value {
    use base64::Engine as _;
    let run = || -> Result<Value, ImportError> {
        let fixture: Value = serde_json::from_str(source).map_err(|e| ImportError(e.to_string()))?;
        let extension = extension_from_detail(&fixture["extension"])?.ok_or_else(|| ImportError("unsupported license".into()))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(fixture["vsix"].as_str().unwrap_or_default())
            .map_err(|e| ImportError(e.to_string()))?;
        let themes = import_package(&extension, &bytes)?;
        Ok(Value::Array(themes.iter().map(OpenVsxTheme::to_file).collect()))
    };
    run().unwrap_or_else(|error| serde_json::json!({ "error": error.0 }))
}
