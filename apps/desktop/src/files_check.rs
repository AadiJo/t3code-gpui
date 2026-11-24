// Scripted end-to-end checks and measurements of the Files tab, headless,
// against real directories on disk read through `DirectoryFiles`.
// Usage: files-check <out-dir> [checks|perf|all]
// Writes checks.txt (PASS/FAIL lines), measurements.txt and PNG screenshots
// of each step in dark and light.
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, HeadlessAppContext, IntoElement, Keystroke, ParentElement as _,
    Render, Styled as _, Window, WindowBounds, WindowOptions, component::TitleBar, div, point, px, size,
};
use t3code_web::ActiveAppTheme as _;
use t3code_web::components::files::{
    editor_buffer::Pos,
    file_preview_panel::{FilesPanel, SaveState},
};
use t3code_web::file_source::DirectoryFiles;
use t3code_web::scenes::files::write_fixture;
use t3code_web::theme_palette::{AppTheme, Appearance, ThemeDocument, set_app_theme};

struct Host {
    panel: Entity<FilesPanel>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        div().size_full().bg(colors.canvas).child(self.panel.clone())
    }
}

struct Harness {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    panel: Entity<FilesPanel>,
    out: PathBuf,
    log: String,
    failures: usize,
}

impl Harness {
    fn new(root: &Path, appearance: Appearance, out: &Path, width: f32, height: f32) -> anyhow::Result<Self> {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(t3code_web::Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(t3code_web::init);
        let root = root.to_owned();
        let (window, panel) = cx.update(|cx| -> anyhow::Result<_> {
            set_app_theme(AppTheme::new(&ThemeDocument::graphite(), appearance), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(width), px(height)), cx)),
                show: false,
                focus: true,
                ..TitleBar::window_options()
            };
            let mut out = None;
            let handle = gpui_kit::open_window(options, cx, |window, cx| {
                gpui_kit::component::Theme::change(
                    if appearance == Appearance::Light { gpui_kit::component::ThemeMode::Light } else { gpui_kit::component::ThemeMode::Dark },
                    Some(window),
                    cx,
                );
                let files = DirectoryFiles::new(root.clone());
                let panel = cx.new(|cx| FilesPanel::new(files, "orbit".into(), window, cx));
                out = Some(panel.clone());
                cx.new(|_| Host { panel })
            })?;
            Ok((handle.0, out.context("panel")?))
        })?;
        let mut harness = Self { cx, window, panel, out: out.to_owned(), log: String::new(), failures: 0 };
        harness.settle();
        Ok(harness)
    }

    fn with<R>(&mut self, f: impl FnOnce(&Entity<FilesPanel>, &mut Window, &mut App) -> R) -> R {
        let panel = self.panel.clone();
        self.cx.update_window(self.window, |_, window, cx| f(&panel, window, cx)).expect("window")
    }

    fn read<R>(&mut self, f: impl FnOnce(&FilesPanel, &App) -> R) -> R {
        let panel = self.panel.clone();
        self.cx.update(|cx| f(panel.read(cx), cx))
    }

    /// Draws a frame from whatever is dirty, as the platform's frame callback does.
    fn frame(&mut self) {
        self.cx.update_window(self.window, |_, window, cx| window.draw(cx).clear(cx)).ok();
        self.cx.run_until_parked();
    }

    /// Lets background reads finish (they run on real threads) and draws.
    fn settle(&mut self) {
        for _ in 0..40 {
            self.cx.run_until_parked();
            self.frame();
            let busy = self.read(|panel, cx| panel.is_loading() || panel.tree().read(cx).is_loading());
            if !busy {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        self.frame();
    }

    fn wait_until(&mut self, what: &str, mut done: impl FnMut(&FilesPanel, &App) -> bool) -> bool {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(20) {
            self.cx.run_until_parked();
            self.frame();
            if self.read(|panel, cx| done(panel, cx)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        writeln!(self.log, "  timed out waiting for {what}").ok();
        false
    }

    fn key(&mut self, source: &str) {
        let os = self.cx.update(|cx| cx.platform_info().os);
        let mut keystroke = Keystroke::parse_with_os(source, os).expect("keystroke");
        if source.chars().count() == 1 {
            keystroke.key_char = Some(source.to_owned());
        }
        self.cx.update_window(self.window, |_, window, cx| window.dispatch_keystroke(keystroke, cx)).ok();
    }

    fn press(&mut self, source: &str) {
        self.key(source);
        self.frame();
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let key = match c {
                ' ' => "space".to_owned(),
                '\n' => "enter".to_owned(),
                c => c.to_string(),
            };
            self.press(&key);
        }
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame();
        let image = self.cx.capture_screenshot(self.window)?;
        let path = self.out.join("shots").join(format!("{name}.png"));
        image.save(&path)?;
        writeln!(self.log, "  screenshot {}", path.display()).ok();
        Ok(())
    }

    fn check(&mut self, name: &str, ok: bool, detail: impl std::fmt::Display) {
        if !ok {
            self.failures += 1;
        }
        writeln!(self.log, "{} {name}: {detail}", if ok { "PASS" } else { "FAIL" }).ok();
    }

    fn open(&mut self, path: &str, line: Option<u32>) {
        let path = path.to_owned();
        self.with(|panel, window, cx| panel.update(cx, |panel, cx| panel.open_file(path.into(), line, window, cx)));
        self.settle();
    }

    fn focus_editor(&mut self) {
        self.with(|panel, window, cx| {
            if let Some(editor) = panel.read(cx).editor().cloned() {
                let handle = gpui_kit::Focusable::focus_handle(editor.read(cx), cx);
                window.focus(&handle, cx);
            }
        });
        self.frame();
    }

    fn editor_text(&mut self) -> String {
        self.read(|panel, cx| panel.editor().map(|e| e.read(cx).text()).unwrap_or_default())
    }

    fn set_caret(&mut self, pos: Pos) {
        self.with(|panel, _, cx| {
            if let Some(editor) = panel.read(cx).editor().cloned() {
                editor.update(cx, |editor, cx| editor.set_caret(pos, cx));
            }
        });
        self.frame();
    }
}

fn modifier() -> &'static str {
    if cfg!(target_os = "macos") { "cmd" } else { "ctrl" }
}

fn checks(root: &Path, appearance: Appearance, out: &Path) -> anyhow::Result<(String, usize)> {
    let suffix = if appearance == Appearance::Dark { "dark" } else { "light" };
    write_fixture(root)?;
    std::fs::write(root.join("bin/blob.bin").tap_parent()?, [0u8, 159, 146, 150, 0, 1, 2, 3])?;
    let mut h = Harness::new(root, appearance, out, 540., 760.)?;
    writeln!(h.log, "== {suffix}").ok();
    let m = modifier();

    // The tree lists the root, folders first.
    let rows: Vec<String> = h.read(|p, cx| p.tree().read(cx).row_paths().map(|s| s.to_string()).collect());
    h.check("tree lists the root lazily", rows.first().map(String::as_str) == Some("assets") && rows.contains(&"README.md".into()) && !rows.contains(&"src/main.rs".into()), format!("{} rows: {:?}", rows.len(), &rows[..rows.len().min(6)]));
    h.shot(&format!("01-tree-{suffix}"))?;

    // Expand folders by clicking them.
    for folder in ["src", "data"] {
        h.with(|panel, _, cx| panel.read(cx).tree().clone().update(cx, |tree, cx| tree.activate_path(folder, cx)));
        h.settle();
    }
    let rows: Vec<String> = h.read(|p, cx| p.tree().read(cx).row_paths().map(|s| s.to_string()).collect());
    h.check("expanding src and data lists their children", rows.contains(&"src/main.rs".into()) && rows.contains(&"data/planets.csv".into()), format!("{} rows", rows.len()));
    let flattened = h.read(|p, cx| p.tree().read(cx).row_paths().any(|s| s.as_ref() == "docs"));
    h.check("docs is listed before its single child folder loads", flattened, "docs row present");
    h.shot(&format!("02-expanded-{suffix}"))?;

    // Keyboard: focus the tree, move down, open with Enter.
    h.with(|panel, window, cx| {
        let tree = panel.read(cx).tree().clone();
        let handle = gpui_kit::Focusable::focus_handle(tree.read(cx), cx);
        window.focus(&handle, cx);
    });
    h.frame();
    h.press("home");
    h.press("down");
    let cursor = h.read(|p, cx| p.tree().read(cx).cursor());
    h.press("right");
    h.settle();
    let after_keys: Vec<String> = h.read(|p, cx| p.tree().read(cx).row_paths().map(|s| s.to_string()).collect());
    h.check("home, down and right expand the second folder (bin)", cursor == Some(1) && after_keys.contains(&"bin/blob.bin".into()), format!("cursor {cursor:?}, {} -> {} rows", rows.len(), after_keys.len()));
    h.press("left");
    h.press("down");
    h.press("down");
    h.press("enter");
    h.settle();
    let opened = h.read(|p, _| p.open_path().map(|s| s.to_string()));
    h.check("left collapses, down and enter open the row under the cursor", opened.as_deref() == Some("data/moons.tsv"), format!("{opened:?}"));

    // Type to filter: typing in the tree searches and hides non-matches.
    h.type_text("orbit");
    h.settle();
    let filtered: Vec<String> = h.read(|p, cx| p.tree().read(cx).row_paths().map(|s| s.to_string()).collect());
    h.check("typing filters the tree to matches and their folders", filtered.iter().any(|p| p == "src/orbit.rs") && !filtered.iter().any(|p| p == "README.md"), format!("{filtered:?}"));
    h.shot(&format!("03-type-to-filter-{suffix}"))?;
    h.press("escape");
    h.settle();

    // Context menu on a file row.
    h.with(|panel, _, cx| panel.read(cx).tree().clone().update(cx, |tree, cx| tree.open_context_menu("src/main.rs", point(px(330.), px(300.)), cx)));
    h.frame();
    let menu = h.read(|p, cx| p.tree().read(cx).menu_open());
    h.check("right click opens the file context menu", menu, "menu open");
    h.shot(&format!("04-context-menu-{suffix}"))?;
    h.press("escape");
    let closed = h.read(|p, cx| !p.tree().read(cx).menu_open());
    h.check("escape closes the context menu", closed, "menu closed");

    // Text with highlighting and a revealed line.
    h.open("src/orbit.rs", Some(22));
    let caret = h.read(|p, cx| p.editor().map(|e| e.read(cx).caret()));
    h.check("a path with a line opens the file at that line", caret == Some(Pos::new(21, 0)), format!("{caret:?}"));
    h.shot(&format!("05-text-reveal-{suffix}"))?;

    // Markdown source and rendered.
    h.open("README.md", None);
    h.shot(&format!("06-markdown-source-{suffix}"))?;
    h.with(|panel, _, cx| panel.update(cx, |panel, cx| panel.set_rendered(true, cx)));
    h.frame();
    let rendered = h.read(|p, _| p.is_rendered());
    h.check("markdown toggles to rendered", rendered, "rendered");
    h.shot(&format!("07-markdown-rendered-{suffix}"))?;
    h.with(|panel, _, cx| panel.update(cx, |panel, cx| panel.set_rendered(false, cx)));

    // CSV as a table.
    h.open("data/planets.csv", None);
    let table_rows = h.read(|p, cx| p.table_view().map(|t| t.read(cx).row_count()));
    h.check("csv opens as a table with a header row", table_rows == Some(9), format!("{table_rows:?} data rows"));
    h.shot(&format!("08-csv-{suffix}"))?;

    // Image with zoom.
    h.open("assets/orbit.png", None);
    let size = h.read(|p, cx| p.image_view().map(|i| (i.read(cx).image().width, i.read(cx).image().height)));
    h.check("png decodes at its pixel size", size == Some((640, 400)), format!("{size:?}"));
    h.with(|panel, _, cx| {
        if let Some(image) = panel.read(cx).image_view().cloned() {
            image.update(cx, |image, cx| image.change_zoom(1.25 * 1.25 * 1.25 * 1.25 * 1.25, cx));
        }
    });
    h.frame();
    let scale = h.read(|p, cx| p.image_view().map(|i| i.read(cx).scale()));
    h.check("zoom in steps by 1.25x", scale.is_some_and(|s| s > 1.), format!("{scale:?}"));
    h.shot(&format!("09-image-zoomed-{suffix}"))?;

    // Expanded image dialog: gallery through the folder's images, zoom, close.
    h.with(|panel, _, cx| {
        if let Some(image) = panel.read(cx).image_view().cloned() {
            image.update(cx, |_, cx| cx.emit(t3code_web::components::files::preview_image::ExpandImage));
        }
    });
    h.settle();
    h.settle();
    let dialog = h.read(|p, cx| p.expanded_dialog().map(|d| d.read(cx).index()));
    h.check("clicking the image opens the expanded viewer", dialog.is_some(), format!("{dialog:?}"));
    h.shot(&format!("10-expanded-{suffix}"))?;
    h.press("right");
    h.settle();
    let index = h.read(|p, cx| p.expanded_dialog().map(|d| d.read(cx).index()));
    h.check("right arrow moves to the next image in the folder", index.is_some() && index != dialog, format!("{dialog:?} -> {index:?}"));
    h.press("+");
    h.frame();
    let zoom = h.read(|p, cx| p.expanded_dialog().and_then(|d| d.read(cx).image().map(|i| i.read(cx).zoom())));
    h.check("+ zooms the expanded image 1.5x", zoom == Some(1.5), format!("{zoom:?}"));
    h.shot(&format!("11-expanded-next-zoomed-{suffix}"))?;
    h.press("escape");
    h.frame();
    let closed = h.read(|p, _| p.expanded_dialog().is_none());
    h.check("escape closes the expanded viewer", closed, "closed");

    // Other kinds open in the default app.
    for (path, label) in [("assets/demo.mp4", "video"), ("assets/manual.pdf", "pdf"), ("bin/blob.bin", "binary")] {
        h.open(path, None);
        let external = h.read(|p, _| p.editor().is_none() && p.image_view().is_none() && !p.is_loading());
        h.check(&format!("{label} shows the open-in-default-app card"), external, path);
        h.shot(&format!("12-external-{label}-{suffix}"))?;
    }

    // Edit and save: type, Mod+S, bytes on disk.
    h.open("src/main.rs", None);
    let original = std::fs::read_to_string(root.join("src/main.rs"))?;
    h.focus_editor();
    h.set_caret(Pos::new(2, 0));
    h.type_text("// edited natively\n");
    let dirty = h.read(|p, cx| p.editor().is_some_and(|e| e.read(cx).is_dirty()));
    h.check("typing marks the file dirty", dirty, "dirty");
    h.press(&format!("{m}-s"));
    let saved = h.wait_until("save", |p, _| p.save_state() == Some(&SaveState::Saved));
    let disk = std::fs::read_to_string(root.join("src/main.rs"))?;
    let expected = original.replacen("mod orbit;", "// edited natively\nmod orbit;", 1);
    h.check("mod+s writes the editor's text to disk", saved && disk == expected, format!("{} bytes on disk, expected {}", disk.len(), expected.len()));
    h.shot(&format!("13-saved-{suffix}"))?;

    // Autosave 500ms after the last edit.
    h.type_text("x");
    // The headless executor's clock moves only when told to.
    h.cx.advance_clock(Duration::from_millis(650));
    h.wait_until("autosave", |p, _| p.save_state() == Some(&SaveState::Saved));
    let disk = std::fs::read_to_string(root.join("src/main.rs"))?;
    h.check("edits save themselves after 500ms", disk.contains("// edited natively\nxmod orbit;"), "x on disk");

    // Undo.
    h.press(&format!("{m}-z"));
    h.cx.advance_clock(Duration::from_millis(650));
    let text = h.editor_text();
    h.check("mod+z undoes the last typing", text.contains("// edited natively\nmod orbit;"), "x removed");

    // Find in the editor.
    h.with(|panel, window, cx| {
        if let Some(editor) = panel.read(cx).editor().cloned() {
            editor.update(cx, |editor, cx| editor.find("body", window, cx));
        }
    });
    h.frame();
    let status = h.read(|p, cx| p.editor().and_then(|e| e.read(cx).find_status()));
    h.check("find highlights every match", status.is_some_and(|(current, total)| current.is_some() && total >= 3), format!("{status:?}"));
    h.shot(&format!("14-find-{suffix}"))?;
    h.press("escape");
    h.focus_editor();

    // The web writes without comparing with the disk (`projects.writeFile`): the last write wins.
    h.wait_until("save before outside change", |p, _| p.save_state() == Some(&SaveState::Saved));
    std::fs::write(root.join("src/main.rs"), "// changed by someone else\n")?;
    h.set_caret(Pos::new(0, 0));
    h.type_text("y");
    h.press(&format!("{m}-s"));
    h.wait_until("overwrite", |p, _| p.save_state() == Some(&SaveState::Saved));
    let disk = std::fs::read_to_string(root.join("src/main.rs"))?;
    let editor = h.editor_text();
    h.check("a save writes the editor's text over an outside change, with no prompt", disk == editor && disk.starts_with('y'), format!("{} bytes", disk.len()));
    h.shot(&format!("15-saved-over-outside-change-{suffix}"))?;

    // Word wrap: the toolbar toggle wraps long lines into rows.
    h.set_caret(Pos::new(0, 0));
    h.type_text(&format!("// {}\n", "wrapped words ".repeat(24)));
    h.with(|panel, _, cx| panel.update(cx, |panel, cx| panel.set_word_wrap(false, cx)));
    h.frame();
    let unwrapped = h.read(|p, cx| p.editor().map(|e| (e.read(cx).row_count(), e.read(cx).line_count())));
    h.with(|panel, _, cx| panel.update(cx, |panel, cx| panel.set_word_wrap(true, cx)));
    h.frame();
    let wrapped = h.read(|p, cx| p.editor().map(|e| (e.read(cx).row_count(), e.read(cx).line_count())));
    h.check(
        "word wrap breaks a long line into rows, and off keeps one row per line",
        unwrapped.is_some_and(|(rows, lines)| rows == lines) && wrapped.is_some_and(|(rows, lines)| rows > lines),
        format!("off {unwrapped:?}, on {wrapped:?} (rows, lines)"),
    );
    h.shot(&format!("15a-word-wrap-{suffix}"))?;
    h.press(&format!("{m}-z"));
    h.cx.advance_clock(Duration::from_millis(650));
    h.wait_until("save after wrap test", |p, _| p.save_state() == Some(&SaveState::Saved));

    // Highlighting follows edits: opening a block comment recolours the lines below it.
    h.set_caret(Pos::new(4, 0));
    h.type_text("/*");
    h.frame();
    let below = h.read(|p, cx| p.editor().and_then(|e| e.read(cx).line_tokens(10)));
    let commented = below.as_ref().is_some_and(|t| !t.is_empty() && t.iter().all(|t| matches!(t, t3code_web::syntax::SyntaxToken::Comment | t3code_web::syntax::SyntaxToken::CommentDoc)));
    h.check("typing /* turns the lines below into a comment", commented, format!("{below:?}"));
    h.shot(&format!("15b-highlight-after-edit-{suffix}"))?;
    h.press("backspace");
    h.press("backspace");
    h.frame();
    let restored = h.read(|p, cx| p.editor().and_then(|e| e.read(cx).line_tokens(10)));
    let code = restored.as_ref().is_some_and(|t| t.iter().any(|t| !matches!(t, t3code_web::syntax::SyntaxToken::Comment | t3code_web::syntax::SyntaxToken::CommentDoc)));
    h.check("deleting it restores the code colours", code, format!("{restored:?}"));
    h.cx.advance_clock(Duration::from_millis(650));
    h.wait_until("save after comment test", |p, _| p.save_state() == Some(&SaveState::Saved));

    // Edits that cannot be written stay as the file's draft (`fileSaveSessions`):
    // leaving opens the other file at once, and coming back shows and writes them.
    let main = root.join("src/main.rs");
    let mut permissions = std::fs::metadata(&main)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&main, permissions.clone())?;
    let before = std::fs::read_to_string(&main)?;
    h.set_caret(Pos::new(0, 0));
    h.type_text("z");
    h.open("Cargo.toml", None);
    h.settle();
    let opened = h.read(|p, _| p.open_path().map(|s| s.to_string()));
    let pending = h.read(|p, _| p.has_unsaved());
    h.check("leaving a file whose write fails opens the next file with no prompt", opened.as_deref() == Some("Cargo.toml") && pending, format!("{opened:?}, unsaved {pending}"));
    h.check("the failed write left the disk copy alone", std::fs::read_to_string(&main)? == before, "unchanged");
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&main, permissions)?;
    h.open("src/main.rs", None);
    let draft = h.editor_text();
    h.check("reopening the file shows the unsaved draft", draft.starts_with('z'), format!("starts {:?}", draft.chars().take(8).collect::<String>()));
    h.cx.advance_clock(Duration::from_millis(650));
    h.wait_until("draft written", |p, _| p.save_state() == Some(&SaveState::Saved));
    let disk = std::fs::read_to_string(&main)?;
    let clean = !h.read(|p, _| p.has_unsaved());
    h.check("the draft is written once the file is writable again", disk == draft && clean, format!("{} bytes", disk.len()));
    h.shot(&format!("15c-draft-restored-{suffix}"))?;

    // Content search: query, move, open at the line.
    h.with(|panel, window, cx| panel.update(cx, |panel, cx| panel.open_content_search(window, cx)));
    h.frame();
    h.with(|panel, window, cx| {
        if let Some(search) = panel.read(cx).content_search().cloned() {
            search.update(cx, |search, cx| search.set_query_text("propagate", window, cx));
        }
    });
    h.wait_until("content search", |p, cx| p.content_search().is_some_and(|s| !s.read(cx).is_pending()));
    let count = h.read(|p, cx| p.content_search().map(|s| s.read(cx).match_count()));
    h.check("content search finds matches across files", count.is_some_and(|c| c >= 3), format!("{count:?}"));
    h.shot(&format!("16-content-search-{suffix}"))?;
    h.press("down");
    let target = h.read(|p, cx| p.content_search().and_then(|s| s.read(cx).selected_match().map(|m| (m.path.clone(), m.line_number))));
    h.press("enter");
    h.settle();
    let opened = h.read(|p, cx| (p.open_path().map(|s| s.to_string()), p.editor().map(|e| e.read(cx).caret().line + 1)));
    let expected = target.clone().map(|(path, line)| (Some(path), Some(line as usize)));
    h.check("enter opens the match at its line", expected == Some(opened.clone()), format!("{target:?} -> {opened:?}"));
    h.shot(&format!("17-search-jump-{suffix}"))?;

    // Regex with a bad pattern falls back to a literal search.
    h.with(|panel, window, cx| panel.update(cx, |panel, cx| panel.open_content_search(window, cx)));
    h.with(|panel, window, cx| {
        if let Some(search) = panel.read(cx).content_search().cloned() {
            search.update(cx, |search, cx| {
                search.set_options(false, false, true, cx);
                search.set_query_text("Body(", window, cx);
            });
        }
    });
    h.wait_until("regex search", |p, cx| p.content_search().is_some_and(|s| !s.read(cx).is_pending()));
    h.shot(&format!("18-search-invalid-regex-{suffix}"))?;
    h.press("escape");

    // Go to file: fuzzy query, enter opens.
    h.with(|panel, window, cx| panel.update(cx, |panel, cx| panel.open_file_picker(window, cx)));
    h.wait_until("picker", |p, cx| p.file_picker().is_some_and(|f| !f.read(cx).is_pending()));
    h.with(|panel, window, cx| {
        if let Some(picker) = panel.read(cx).file_picker().cloned() {
            picker.update(cx, |picker, cx| picker.set_query_text("orbrs", window, cx));
        }
    });
    h.wait_until("picker results", |p, cx| p.file_picker().is_some_and(|f| !f.read(cx).is_pending()));
    let first = h.read(|p, cx| p.file_picker().and_then(|f| f.read(cx).matches().first().map(|m| m.path.to_string())));
    h.check("go to file ranks src/orbit.rs first for orbrs", first.as_deref() == Some("src/orbit.rs"), format!("{first:?}"));
    h.shot(&format!("19-goto-{suffix}"))?;
    h.press("enter");
    h.settle();
    let opened = h.read(|p, _| p.open_path().map(|s| s.to_string()));
    h.check("enter opens the picked file", opened.as_deref() == Some("src/orbit.rs"), format!("{opened:?}"));
    Ok((h.log, h.failures))
}

trait TapParent {
    fn tap_parent(self) -> std::io::Result<Self>
    where
        Self: Sized;
}

impl TapParent for PathBuf {
    fn tap_parent(self) -> std::io::Result<Self> {
        if let Some(parent) = self.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(self)
    }
}

fn stats(samples: &mut [f64]) -> (f64, f64, f64) {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |q: f64| samples[((samples.len() as f64 - 1.) * q).round() as usize];
    (at(0.5), at(0.95), *samples.last().unwrap_or(&0.))
}

fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

/// 10,000 lines of plausible Rust.
fn big_rust() -> String {
    let mut text = String::new();
    for i in 0..2_000 {
        writeln!(text, "/// Step {i} of the propagation table.").ok();
        writeln!(text, "pub fn step_{i}(body: &mut Body, dt: f64) -> f64 {{").ok();
        writeln!(text, "    let radius = body.radius() * {i}.0 + dt; // \"units\": km").ok();
        writeln!(text, "    radius.sqrt()").ok();
        writeln!(text, "}}").ok();
    }
    text
}

/// 50,000 files in 500 folders of 100, in 25 top-level groups.
fn write_large_fixture(root: &Path) -> anyhow::Result<usize> {
    let marker = root.join(".complete-50000");
    if marker.exists() {
        return Ok(50_000);
    }
    let mut count = 0;
    for group in 0..25 {
        for folder in 0..20 {
            let dir = root.join(format!("group-{group:02}/module-{folder:02}"));
            std::fs::create_dir_all(&dir)?;
            for file in 0..100 {
                let ext = ["rs", "ts", "tsx", "md", "json"][file % 5];
                std::fs::write(dir.join(format!("item_{file:03}.{ext}")), b"")?;
                count += 1;
            }
        }
    }
    std::fs::write(marker, b"")?;
    Ok(count)
}

fn perf(out: &Path) -> anyhow::Result<String> {
    let mut report = String::new();
    // Large generated fixtures live in the temp dir, not beside the artifacts.
    let work = std::env::temp_dir().join("t3code-files-perf");
    std::fs::create_dir_all(work.join("src"))?;
    std::fs::write(work.join("src/big.rs"), big_rust())?;
    std::fs::write(work.join("src/big.txt"), big_rust())?;
    // A 5 MB text file (over the 1 MB read limit, so it opens read-only through the asset bytes).
    let mut five = String::with_capacity(5 * 1024 * 1024 + 100);
    let mut line = 0;
    while five.len() < 5 * 1024 * 1024 {
        writeln!(five, "{line:08} INFO request handled path=/api/v1/orbit/{line} status=200 duration_ms={}", line % 97).ok();
        line += 1;
    }
    std::fs::write(work.join("server.log"), &five)?;

    writeln!(report, "build: {}", if cfg!(debug_assertions) { "debug (dev profile: GPUI, text and syntect crates at opt-level 3)" } else { "release" })?;
    let rss_start = rss_kib();
    let mut h = Harness::new(&work, Appearance::Dark, out, 540., 760.)?;
    let rss_window = rss_kib();

    // Typing in a 10,000-line file.
    h.open("src/big.rs", Some(5_000));
    h.focus_editor();
    h.set_caret(Pos::new(5_001, 10));
    let lines = h.read(|p, cx| p.editor().map(|e| e.read(cx).line_count()));
    let chars: Vec<char> = "let quick = brown.fox(); ".chars().collect();
    let measure = |h: &mut Harness, count: usize| -> Vec<f64> {
        let mut samples = Vec::with_capacity(count);
        for i in 0..count {
            let c = chars[i % chars.len()];
            let key = if c == ' ' { "space".to_owned() } else { c.to_string() };
            let start = Instant::now();
            h.key(&key);
            h.cx.update_window(h.window, |_, window, cx| window.draw(cx).clear(cx)).ok();
            samples.push(start.elapsed().as_secs_f64() * 1000.);
            h.cx.run_until_parked();
        }
        samples
    };
    measure(&mut h, 40);
    let mut samples = measure(&mut h, 500);
    let (median, p95, max) = stats(&mut samples);
    writeln!(report, "editor typing, {} lines, caret at line 5,002 (highlighted Rust): dispatch_keystroke + draw, ms over 500 keys", lines.unwrap_or(0))?;
    writeln!(report, "  median {median:.3}  p95 {p95:.3}  max {max:.3}  (frame budget 8.33 at 120 Hz, 16.7 at 60 Hz)")?;
    // Enter splits lines: the highlighter re-parses below the caret, within its per-frame budget.
    let mut enters = Vec::new();
    for _ in 0..50 {
        let start = Instant::now();
        h.key("enter");
        h.cx.update_window(h.window, |_, window, cx| window.draw(cx).clear(cx)).ok();
        enters.push(start.elapsed().as_secs_f64() * 1000.);
        h.cx.run_until_parked();
    }
    let (median, p95, max) = stats(&mut enters);
    writeln!(report, "editor enter (line inserted), same file: median {median:.3}  p95 {p95:.3}  max {max:.3} ms over 50")?;
    h.shot("perf-editor-typing")?;

    // The same file as plain text: typing without syntax highlighting.
    h.open("src/big.txt", Some(5_000));
    h.focus_editor();
    h.set_caret(Pos::new(5_001, 10));
    measure(&mut h, 40);
    let mut plain = measure(&mut h, 500);
    let (median, p95, max) = stats(&mut plain);
    writeln!(report, "editor typing, same 10,000 lines as plain text (no highlighter): median {median:.3}  p95 {p95:.3}  max {max:.3} ms over 500 keys")?;

    // Resident memory with a 5 MB file open.
    let before = rss_kib();
    h.open("server.log", None);
    h.wait_until("large file", |p, _| p.large_view().is_some());
    h.settle();
    let after = rss_kib();
    writeln!(report, "resident memory: process start {} MiB; window with the Files tab and tree {} MiB; after two 10,000-line files (highlighted, then plain) {} MiB; with server.log ({} bytes) open {} MiB (+{} MiB)",
        rss_start / 1024, rss_window / 1024, before / 1024, five.len(), after / 1024, (after.saturating_sub(before)) / 1024)?;
    h.shot("perf-large-file")?;
    drop(h);

    // Tree scrolling and go to file on 50,000 files.
    let large = std::env::temp_dir().join("t3code-files-perf-50k");
    let count = write_large_fixture(&large)?;
    let mut h = Harness::new(&large, Appearance::Dark, out, 540., 760.)?;
    let started = Instant::now();
    h.with(|panel, _, cx| panel.read(cx).tree().clone().update(cx, |tree, cx| tree.set_all_expanded(true, cx)));
    h.wait_until("all folders listed", |p, cx| !p.tree().read(cx).is_loading() && p.tree().read(cx).row_count() > 50_000);
    let rows = h.read(|p, cx| p.tree().read(cx).row_count());
    writeln!(report, "tree: {count} files, expand all listed {} folders into {rows} rows in {:.0} ms (fixture reads on disk)", 25 + 500, started.elapsed().as_secs_f64() * 1000.)?;
    let scroll = h.read(|p, cx| p.tree().read(cx).scroll_handle().clone());
    let mut frames = Vec::new();
    for i in 0..600 {
        // 600 frames at 48px each, then a jump every 100 frames.
        let y = if i % 100 == 99 { (i as f32) * 2_000. } else { i as f32 * 48. };
        scroll.0.borrow().base_handle.set_offset(point(px(0.), px(-y)));
        h.cx.update_window(h.window, |_, window, _| window.refresh()).ok();
        let start = Instant::now();
        h.cx.update_window(h.window, |_, window, cx| window.draw(cx).clear(cx)).ok();
        frames.push(start.elapsed().as_secs_f64() * 1000.);
    }
    let (median, p95, max) = stats(&mut frames);
    writeln!(report, "tree scroll frame (draw after moving the offset), {rows} rows: median {median:.3}  p95 {p95:.3}  max {max:.3} ms over 600 frames")?;
    h.shot("perf-tree-50k")?;

    // Go to file: each keystroke narrows the shown list in the same frame; the
    // fixture's ranked search over 50,000 paths lands after.
    h.with(|panel, window, cx| panel.update(cx, |panel, cx| panel.open_file_picker(window, cx)));
    h.wait_until("picker", |p, cx| p.file_picker().is_some_and(|f| !f.read(cx).is_pending()));
    let mut keystroke_frames = Vec::new();
    let mut search_times = Vec::new();
    let mut query = String::new();
    for c in "grp03mod07item042".chars() {
        query.push(c);
        let start = Instant::now();
        h.with(|panel, window, cx| {
            if let Some(picker) = panel.read(cx).file_picker().cloned() {
                let q = query.clone();
                picker.update(cx, |picker, cx| picker.set_query_text(&q, window, cx));
            }
        });
        h.cx.update_window(h.window, |_, window, cx| window.draw(cx).clear(cx)).ok();
        keystroke_frames.push(start.elapsed().as_secs_f64() * 1000.);
        h.wait_until("picker results", |p, cx| p.file_picker().is_some_and(|f| !f.read(cx).is_pending()));
        search_times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    let first = h.read(|p, cx| p.file_picker().and_then(|f| f.read(cx).matches().first().map(|m| m.path.to_string())));
    let (median, p95, max) = stats(&mut keystroke_frames);
    writeln!(report, "go to file over {count} paths, query typed one key at a time: keystroke to drawn frame median {median:.3}  p95 {p95:.3}  max {max:.3} ms")?;
    let (median, p95, max) = stats(&mut search_times);
    writeln!(report, "  keystroke to ranked results drawn (fixture search on a background thread, plus up to 10ms wait loop): median {median:.1}  p95 {p95:.1}  max {max:.1} ms; top result {first:?}")?;
    h.shot("perf-goto-50k")?;
    Ok(report)
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().context("usage: files-check <out-dir> [checks|perf|all]")?);
    let mode = args.next().unwrap_or_else(|| "all".into());
    std::fs::create_dir_all(out.join("shots"))?;
    if mode == "checks" || mode == "all" {
        let mut log = String::new();
        let mut failures = 0;
        for appearance in [Appearance::Dark, Appearance::Light] {
            let root = out.join(format!("workspace-{}", if appearance == Appearance::Dark { "dark" } else { "light" }));
            if root.exists() {
                std::fs::remove_dir_all(&root)?;
            }
            let (part, failed) = checks(&root, appearance, &out)?;
            log.push_str(&part);
            failures += failed;
        }
        writeln!(log, "{failures} failed").ok();
        std::fs::write(out.join("checks.txt"), &log)?;
        print!("{log}");
    }
    if mode == "perf" || mode == "all" {
        let report = perf(&out)?;
        std::fs::write(out.join(if cfg!(debug_assertions) { "measurements-debug.txt" } else { "measurements-release.txt" }), &report)?;
        print!("{report}");
    }
    Ok(())
}
