//! Scene `diffs`: the diff panel over a fixture patch with every file status
//! and a 5,000-line file. `T3_DIFFS_SCENE` picks a variant as a comma list:
//! `light`, `split`, `nowrap`, `expanded`, `tree`, `find`, `comment`,
//! `select`, `context`, `perf` (300 files, 60,000 changed lines), `file`
//! (the plain code view).

use std::{collections::HashSet, fmt::Write as _, sync::Arc};

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window, div};

use crate::{
    components::{
        diff_panel::{CommitSummary, DiffPanel, DiffPanelContent, DiffPanelInput, TurnSummary},
        diffs::{CodeView, ViewMode},
    },
    diff_panel_store::{DiffLayout, DiffPanelStore},
    diff_patch::{FileContents, renderable_patch},
    theme_palette::{AppTheme, Appearance, ThemeDocument, set_app_theme},
};

/// Variant flags from `T3_DIFFS_SCENE`.
pub fn scene_flags() -> HashSet<String> {
    std::env::var("T3_DIFFS_SCENE").unwrap_or_default().split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()
}

struct DiffsScene {
    body: gpui_kit::AnyView,
}

impl Render for DiffsScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.body.clone())
    }
}

pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    let flags = scene_flags();
    let appearance = if flags.contains("light") { Appearance::Light } else { Appearance::Dark };
    set_app_theme(AppTheme::new(&ThemeDocument::graphite(), appearance), cx);
    // Text inputs come from gpui-component, which keeps its own light/dark theme.
    gpui_kit::component::Theme::change(
        if appearance == Appearance::Light { gpui_kit::component::ThemeMode::Light } else { gpui_kit::component::ThemeMode::Dark },
        Some(window),
        cx,
    );
    let store = cx.default_global::<DiffPanelStore>();
    if flags.contains("split") {
        store.settings.diff_layout = DiffLayout::Split;
    }
    if flags.contains("nowrap") {
        store.settings.word_wrap = false;
    }
    store.file_tree_open = flags.contains("tree");

    if flags.contains("file") {
        let fixture = Fixture::standard();
        let view = cx.new(|cx| {
            let mut view = CodeView::new(ViewMode::File, window, cx);
            view.set_file_contents("apps/web/src/components/ChatView.tsx".into(), &fixture.chat_view.1, cx);
            view
        });
        view.update(cx, |view, cx| view.reveal_line(42, cx));
        return cx.new(|_| DiffsScene { body: view.into() }).into();
    }

    let fixture = if flags.contains("perf") { Fixture::perf(300, 60_000) } else { Fixture::standard() };
    let panel = cx.new(|cx| DiffPanel::new(window, cx));
    let input = fixture_input(&fixture);
    panel.update(cx, |panel, cx| {
        panel.set_input(input, cx);
        if flags.contains("expanded") {
            panel.toggle_all_files(cx);
        }
        if flags.contains("context") {
            let key = format!("{0}\0{0}\0change", fixture.chat_view_path);
            panel.set_file_contents(&key, FileContents::new(&fixture.chat_view.0, &fixture.chat_view.1), cx);
        }
    });
    if flags.contains("context") {
        let key: gpui_kit::SharedString = format!("{0}\0{0}\0change", fixture.chat_view_path).into();
        panel.read(cx).code_view().clone().update(cx, |view, cx| {
            view.reveal_file(&key, cx);
            view.expand_gap(0, 1, true, false, cx);
            view.expand_gap(0, 1, false, false, cx);
        });
    }
    if flags.contains("find") {
        let view = panel.read(cx).code_view().clone();
        view.update(cx, |view, cx| view.find("message", window, cx));
    }
    cx.new(|_| DiffsScene { body: panel.into() }).into()
}

/// The panel input for a fixture: a thread with three turns and a few commits.
pub fn fixture_input(fixture: &Fixture) -> DiffPanelInput {
    let patch = renderable_patch(&fixture.patch).expect("fixture patch is not empty");
    DiffPanelInput {
        thread_key: Some("env-local:thread-fixture".into()),
        workspace_root: Some("/home/dev/t3code".into()),
        turns: (1..=3)
            .rev()
            .map(|n| TurnSummary { turn_id: format!("turn-{n}").into(), turn_count: n, completed_label: format!("10:4{n} AM").into() })
            .collect(),
        commits: vec![
            CommitSummary { sha: "4f1c2ab9d0e".into(), subject: "feat(web): diff panel".into(), relative_label: "2h ago".into() },
            CommitSummary { sha: "9a8b7c6d5e4".into(), subject: "fix(server): stream turn diffs".into(), relative_label: "1d ago".into() },
        ],
        head_ref: Some("diff-panel".into()),
        base_ref: Some("origin/main".into()),
        can_refresh: true,
        refreshing: false,
        content: DiffPanelContent::Patch { patch: Arc::new(patch), notices: Vec::new(), stats: None },
    }
}

/// A fixture patch, plus full contents of one file so its context can expand.
pub struct Fixture {
    pub patch: String,
    pub chat_view_path: &'static str,
    pub chat_view: (String, String),
}

/// One edit: replace `remove` old lines starting at 0-based `at` with `insert`.
struct Edit {
    at: usize,
    remove: usize,
    insert: Vec<String>,
}

/// Builds a git patch for `base` with `edits` applied, with 3 lines of context.
/// Returns the patch section and the old and new contents.
fn make_patch(path: &str, base: &[String], edits: &[Edit], no_newline: bool) -> (String, String, String) {
    let mut new_lines: Vec<String> = Vec::new();
    let mut cursor = 0;
    for edit in edits {
        new_lines.extend_from_slice(&base[cursor..edit.at]);
        new_lines.extend(edit.insert.iter().cloned());
        cursor = edit.at + edit.remove;
    }
    new_lines.extend_from_slice(&base[cursor..]);

    let mut out = format!("diff --git a/{path} b/{path}\nindex 1a2b3c4..5d6e7f8 100644\n--- a/{path}\n+++ b/{path}\n");
    let mut shift: i64 = 0;
    let mut index = 0;
    while index < edits.len() {
        // Merge edits whose context windows touch.
        let mut group = vec![&edits[index]];
        while index + 1 < edits.len() && edits[index + 1].at <= group.last().map_or(0, |e| e.at + e.remove) + 6 {
            index += 1;
            group.push(&edits[index]);
        }
        let first = group[0];
        let last = group[group.len() - 1];
        let start = first.at.saturating_sub(3);
        let end = (last.at + last.remove + 3).min(base.len());
        let mut body = String::new();
        let (mut old_count, mut new_count) = (0, 0);
        let mut position = start;
        let group_shift = shift;
        for edit in &group {
            for line in &base[position..edit.at] {
                let _ = writeln!(body, " {line}");
                old_count += 1;
                new_count += 1;
            }
            for line in &base[edit.at..edit.at + edit.remove] {
                let _ = writeln!(body, "-{line}");
                old_count += 1;
            }
            for line in &edit.insert {
                let _ = writeln!(body, "+{line}");
                new_count += 1;
            }
            shift += edit.insert.len() as i64 - edit.remove as i64;
            position = edit.at + edit.remove;
        }
        for line in &base[position..end] {
            let _ = writeln!(body, " {line}");
            old_count += 1;
            new_count += 1;
        }
        let new_start = start as i64 + 1 + group_shift;
        let _ = writeln!(out, "@@ -{},{} +{},{} @@ export function render() {{", start + 1, old_count, new_start, new_count);
        out.push_str(&body);
        if no_newline && end == base.len() {
            out.push_str("\\ No newline at end of file\n");
        }
        index += 1;
    }
    (out, base.join("\n") + "\n", new_lines.join("\n") + "\n")
}

fn ts_line(i: usize) -> String {
    match i % 9 {
        0 => format!("export function handleMessage{i}(message: ChatMessage): string {{"),
        1 => format!("  const label = `turn ${{message.turnCount}} of {i}`; // keep the label short"),
        2 => "  if (message.role === \"assistant\" && message.streaming) {".to_owned(),
        3 => format!("    return formatDiffCount({i}) + label.padStart(8, \" \");"),
        4 => "  }".to_owned(),
        5 => format!("  const total = items.reduce((sum, item) => sum + item.additions, {i});"),
        6 => "  void scheduleRender({ immediate: true, reason: \"message\" });".to_owned(),
        7 => "  return label;".to_owned(),
        _ => "}".to_owned(),
    }
}

fn rs_line(i: usize) -> String {
    match i % 8 {
        0 => format!("/// Computes row {i} of the layout cache."),
        1 => format!("pub fn compute_row_{i}(input: &LayoutInput, rows: &mut Vec<Row>) -> Option<f32> {{"),
        2 => "    let height = input.wrap_columns.map_or(20.0, |columns| columns as f32 * 0.5);".to_owned(),
        3 => format!("    if rows.len() > {i} {{ return None; }}"),
        4 => "    rows.push(Row { kind: RowKind::Notice(Notice::Empty), top: 0.0, height });".to_owned(),
        5 => format!("    let message = \"row {i}\\t ready\"; // tab in a string"),
        6 => "    Some(height)".to_owned(),
        _ => "}".to_owned(),
    }
}

fn py_line(i: usize) -> String {
    match i % 5 {
        0 => format!("def build_{i}(items, *, limit={i}):"),
        1 => "\t\"\"\"Return the first items, tab indented.\"\"\"".to_owned(),
        2 => "\treturn [item for item in items if item.enabled][:limit]".to_owned(),
        3 => "".to_owned(),
        _ => format!("RATIO_{i} = 0.{i}  # comment"),
    }
}

impl Fixture {
    /// One file of every kind the panel draws.
    pub fn standard() -> Self {
        let mut patch = String::new();
        let chat_path = "apps/web/src/components/ChatView.tsx";
        let base: Vec<String> = (0..260).map(ts_line).collect();
        let edits = vec![
            Edit { at: 40, remove: 2, insert: vec![ts_line(1001), ts_line(1003), "  // A long comment line that keeps going so that word wrap has to break it across more than one visual row in a narrow panel.".into()] },
            Edit { at: 120, remove: 0, insert: (0..6).map(|i| ts_line(2000 + i)).collect() },
            Edit { at: 200, remove: 4, insert: vec![] },
        ];
        let (section, old, new) = make_patch(chat_path, &base, &edits, false);
        patch.push_str(&section);

        // New file.
        let new_file: Vec<String> = (0..24).map(ts_line).collect();
        let _ = write!(patch, "diff --git a/packages/shared/src/changedFileKind.ts b/packages/shared/src/changedFileKind.ts\nnew file mode 100644\nindex 0000000..3f4e5d6\n--- /dev/null\n+++ b/packages/shared/src/changedFileKind.ts\n@@ -0,0 +1,{} @@\n", new_file.len());
        for line in &new_file {
            let _ = writeln!(patch, "+{line}");
        }
        // Deleted file.
        let deleted: Vec<String> = (300..312).map(ts_line).collect();
        let _ = write!(patch, "diff --git a/apps/web/src/legacy/oldHelpers.ts b/apps/web/src/legacy/oldHelpers.ts\ndeleted file mode 100644\nindex 7a8b9c0..0000000\n--- a/apps/web/src/legacy/oldHelpers.ts\n+++ /dev/null\n@@ -1,{} +0,0 @@\n", deleted.len());
        for line in &deleted {
            let _ = writeln!(patch, "-{line}");
        }
        // Pure rename, rename with changes, mode change, binary.
        patch.push_str("diff --git a/docs/guide.md b/docs/user/guide.md\nsimilarity index 100%\nrename from docs/guide.md\nrename to docs/user/guide.md\n");
        patch.push_str("diff --git a/apps/server/src/config.ts b/apps/server/src/settings.ts\nsimilarity index 91%\nrename from apps/server/src/config.ts\nrename to apps/server/src/settings.ts\nindex 1111111..2222222 100644\n--- a/apps/server/src/config.ts\n+++ b/apps/server/src/settings.ts\n@@ -3,7 +3,7 @@ export const DEFAULTS = {\n   port: 3773,\n   host: \"127.0.0.1\",\n   logLevel: \"info\",\n-  maxDiffBytes: 1_000_000,\n+  maxDiffBytes: 4_000_000,\n   telemetry: false,\n   updates: \"stable\",\n   theme: \"graphite\",\n");
        patch.push_str("diff --git a/scripts/release.sh b/scripts/release.sh\nold mode 100644\nnew mode 100755\n");
        patch.push_str("diff --git a/assets/logo.png b/assets/logo.png\nindex 3333333..4444444 100644\nBinary files a/assets/logo.png and b/assets/logo.png differ\n");
        // No newline at end of file.
        patch.push_str("diff --git a/apps/web/src/version.ts b/apps/web/src/version.ts\nindex 5555555..6666666 100644\n--- a/apps/web/src/version.ts\n+++ b/apps/web/src/version.ts\n@@ -1,3 +1,3 @@\n // Bumped by the release script.\n-export const VERSION = \"0.1.10\";\n+export const VERSION = \"0.1.11\";\n export const CHANNEL = \"stable\";\n\\ No newline at end of file\n");
        // A test file, for the hide-tests toggle.
        let test_base: Vec<String> = (0..40).map(ts_line).collect();
        let (test_section, _, _) = make_patch(
            "apps/web/src/components/ChatView.test.tsx",
            &test_base,
            &[Edit { at: 10, remove: 1, insert: vec!["  expect(screen.getByText(\"message\")).toBeVisible();".into()] }],
            false,
        );
        patch.push_str(&test_section);
        // Tab-indented Python.
        let py_base: Vec<String> = (0..30).map(py_line).collect();
        let (py_section, _, _) = make_patch("tools/gen_fixtures.py", &py_base, &[Edit { at: 6, remove: 1, insert: vec![py_line(16), py_line(17)] }], false);
        patch.push_str(&py_section);
        // A quoted path with a space and a non-ASCII name.
        patch.push_str("diff --git \"a/docs/caf\\303\\251 notes.md\" \"b/docs/caf\\303\\251 notes.md\"\nindex 7777777..8888888 100644\n--- \"a/docs/caf\\303\\251 notes.md\"\n+++ \"b/docs/caf\\303\\251 notes.md\"\n@@ -1,2 +1,2 @@\n # Café notes\n-Draft.\n+Final draft with **bold** text and `code`.\n");
        // The large file: 5,000 new lines.
        let large: Vec<String> = (0..5_000).map(rs_line).collect();
        let _ = write!(patch, "diff --git a/crates/engine/src/large.rs b/crates/engine/src/large.rs\nnew file mode 100644\nindex 0000000..9999999\n--- /dev/null\n+++ b/crates/engine/src/large.rs\n@@ -0,0 +1,{} @@\n", large.len());
        for line in &large {
            let _ = writeln!(patch, "+{line}");
        }
        Fixture { patch, chat_view_path: chat_path, chat_view: (old, new) }
    }

    /// `files` changed files with about `changed_lines` changed lines in all.
    pub fn perf(files: usize, changed_lines: usize) -> Self {
        let mut patch = String::new();
        let per_file = changed_lines / files;
        let mut chat_view = (String::new(), String::new());
        for file in 0..files {
            let directory = ["apps/web/src/components", "apps/server/src/orchestration", "packages/shared/src", "crates/engine/src"][file % 4];
            if file % 3 == 0 {
                let path = format!("{directory}/generated_{file}.rs");
                let _ = write!(patch, "diff --git a/{path} b/{path}\nnew file mode 100644\nindex 0000000..1234567\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{per_file} @@\n");
                for i in 0..per_file {
                    let _ = writeln!(patch, "+{}", rs_line(i + file));
                }
            } else {
                let path = format!("{directory}/module_{file}.tsx");
                let base: Vec<String> = (0..per_file * 2).map(|i| ts_line(i + file)).collect();
                let hunks = 4;
                let size = per_file / (hunks * 2);
                let edits: Vec<Edit> = (0..hunks)
                    .map(|h| Edit { at: h * (base.len() / hunks) + 5, remove: size, insert: (0..size).map(|i| ts_line(i + 7_000 + h)).collect() })
                    .collect();
                let (section, old, new) = make_patch(&path, &base, &edits, false);
                if file == 1 {
                    chat_view = (old, new);
                }
                patch.push_str(&section);
            }
        }
        Fixture { patch, chat_view_path: "apps/server/src/orchestration/module_1.tsx", chat_view }
    }
}

/// Builds a panel entity over a fixture, for harnesses outside the scene registry.
pub fn fixture_panel(fixture: &Fixture, window: &mut Window, cx: &mut App) -> Entity<DiffPanel> {
    let panel = cx.new(|cx| DiffPanel::new(window, cx));
    let input = fixture_input(fixture);
    panel.update(cx, |panel, cx| panel.set_input(input, cx));
    panel
}
