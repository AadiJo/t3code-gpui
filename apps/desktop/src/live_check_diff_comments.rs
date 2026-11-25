// live-check phase `diff-comments`, after `seed` on one server running on this machine: the Diff
// tab's server events. The seeded workspace's branch diff loads; expanding a folded gap asks the
// server for both sides of the file (`review.getDiffFileContents`); a review comment from the
// diff lands in the composer as a review-comment chip, stays in the draft across a thread switch,
// and leaves when the comment is deleted; "Open file in preview browser" on a workspace HTML file
// opens a browser tab from `preview.open`.

use super::*;
use t3code_web::components::diff_panel::DiffPanelEvent;
use t3code_web::components::diffs::review_comment::{AnnotationSide, ReviewComment, ReviewCommentSelection};
use t3code_web::diff_panel_store::{DiffPanelStore, GitScope};
use t3code_web::right_panel_store::RightPanelSurface;
use t3code_web::shortcuts::commands::ToggleDiff;

fn composer_text(check: &mut Check) -> String {
    check.read(|root, cx| root.composer(cx).read(cx).editor().read(cx).text().to_owned())
}

pub(super) fn run_diff_comments(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = thread_with_turn(check, &state, &format!("Diff comments {}", now_ms() % 100_000))?;
    let other = thread_with_turn(check, &state, &format!("Diff comments other {}", now_ms() % 100_000))?;
    check.go(&key);
    check.wait(Duration::from_millis(500));
    let scoped = t3code_web::fixtures::scoped_key(&key).to_string();
    let scope = scoped.clone();
    check.cx.update(|cx| cx.global_mut::<DiffPanelStore>().select_git_scope(&scope, GitScope::Branch));
    check.focus_composer();
    check.action(ToggleDiff);
    check.pump(|c| c.read(|root, cx| root.right_panel().read(cx).diff_loaded(cx)), Duration::from_secs(30), "the branch diff loads")?;
    check.wait(Duration::from_millis(500));
    check.shot("diff-00-branch.png")?;

    // Expanding a folded gap reads the whole file from the server.
    let code_view = check.read(|root, cx| root.right_panel().read(cx).diff_panel().map(|d| d.read(cx).code_view().clone())).context("the diff panel")?;
    check.cx.update(|cx| code_view.update(cx, |view, cx| view.expand_gap(0, 0, false, true, cx)));
    check.pump(|c| c.read(|root, cx| !root.right_panel().read(cx).contents_loaded().is_empty()), Duration::from_secs(20), "the server returns the file's contents")?;
    let loaded = check.read(|root, cx| root.right_panel().read(cx).contents_loaded().to_vec());
    check.check("review.getDiffFileContents answered for the expanded file", !loaded.is_empty(), format!("{loaded:?}"));
    check.shot("diff-01-expanded.png")?;

    // A review comment from the diff becomes a chip in the composer.
    let comment = ReviewComment {
        id: format!("live-{}", now_ms()).into(),
        section_id: "branch".into(),
        section_title: "Branch".into(),
        file_path: "src/math.rs".into(),
        start_index: 4,
        end_index: 6,
        range_label: "+4 to +6".into(),
        text: "Should clamp take a range?".into(),
        diff: "+pub fn clamp(v: i32, lo: i32, hi: i32) -> i32 {".into(),
        fence_language: "rust".into(),
        selection: ReviewCommentSelection { start: 4, side: AnnotationSide::Additions, end: 6, end_side: AnnotationSide::Additions },
    };
    let diff = check.read(|root, cx| root.right_panel().read(cx).diff_panel().cloned()).context("the diff panel")?;
    let event = DiffPanelEvent::ReviewComment(comment.clone());
    check.cx.update(|cx| diff.update(cx, |_, cx| cx.emit(event)));
    check.wait(Duration::from_millis(300));
    let text = composer_text(check);
    check.check("the composer holds the comment's chip", text.contains("[math.rs L4 to L6](t3-context://v1/review-comment/"), text.clone());
    check.shot("diff-02-comment-chip.png")?;
    // The same comment again adds no second chip.
    let again = DiffPanelEvent::ReviewComment(comment.clone());
    check.cx.update(|cx| diff.update(cx, |_, cx| cx.emit(again)));
    check.wait(Duration::from_millis(200));
    let text = composer_text(check);
    check.check("a repeated comment keeps one chip", text.matches("review-comment/").count() == 1, text.clone());

    // The draft keeps the chip and its comment across a thread switch.
    check.go(&other);
    check.wait(Duration::from_millis(600));
    check.go(&key);
    check.wait(Duration::from_millis(600));
    let text = composer_text(check);
    let kept = t3code_web::state::pull_request_mentions::comments_in(&text);
    check.check("the draft brings the chip back", text.contains("review-comment/"), text.clone());
    check.check("the draft keeps the comment's text", kept.iter().any(|c| c.text == "Should clamp take a range?"), format!("{} kept", kept.len()));

    // Deleting the comment removes its chip.
    let diff = check.read(|root, cx| root.right_panel().read(cx).diff_panel().cloned()).context("the diff panel")?;
    let delete = DiffPanelEvent::DeleteReviewComment(comment.id.clone());
    check.cx.update(|cx| diff.update(cx, |_, cx| cx.emit(delete)));
    check.wait(Duration::from_millis(300));
    let text = composer_text(check);
    check.check("deleting the comment removes its chip", !text.contains("review-comment/"), text.clone());

    // Open file in preview browser.
    let root_dir = std::env::var("T3_PROJECT_ROOT").context("T3_PROJECT_ROOT")?;
    let html = format!("{root_dir}/docs/index.html");
    std::fs::create_dir_all(format!("{root_dir}/docs"))?;
    std::fs::write(&html, "<!doctype html><title>Quokka</title><h1>Quokka preview</h1>")?;
    let panel = check.read(|root, _| root.right_panel().clone());
    check
        .cx
        .update_window(check.window, |_, window, cx| panel.update(cx, |panel, cx| panel.open_file("docs/index.html".into(), None, window, cx)))
        .ok();
    let files_panel = panel.clone();
    check.pump(move |c| c.cx.update(|cx| files_panel.update(cx, |p, cx| p.files_panel(cx)).is_some()), Duration::from_secs(10), "the Files tab opens")?;
    check.wait(Duration::from_millis(800));
    check.shot("diff-03-html-file.png")?;
    let files = check.cx.update(|cx| panel.update(cx, |p, cx| p.files_panel(cx))).context("the files panel")?;
    let open = t3code_web::components::files::file_preview_panel::FilesPanelEvent::OpenInPreview { absolute_path: html.clone() };
    check.cx.update(|cx| files.update(cx, |_, cx| cx.emit(open)));
    let panel_key = scoped.clone();
    check.pump(
        move |c| {
            c.read(|root, cx| {
                let state = root.right_panels().read(cx).state(&panel_key);
                matches!(state.active(), Some(RightPanelSurface::Preview { .. }))
            })
        },
        Duration::from_secs(30),
        "the file opens in a browser tab",
    )?;
    check.wait(Duration::from_millis(1500));
    check.shot("diff-04-file-in-browser.png")?;
    Ok(())
}
