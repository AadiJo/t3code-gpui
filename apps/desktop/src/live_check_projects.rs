// live-check phase `add-project`, after `seed` on one server running on this machine: the
// palette's Add project flows against the real server. `T3_ADD_ROOT` is a folder holding
// `alpha/` and a local bare repository `widgets.git`. Local folder lists that folder from
// `filesystem.browse`, adds `alpha` and creates a missing `gamma`; Git URL clones the bare
// repository to `widgets-clone` and shows its toast; a URL that does not exist fails with a
// toast; New project creates "Quokka Notes" under the server's new projects root. Each project
// is read back from the server's shell, and each folder from disk.

use super::*;
use t3code_web::command_palette_store::OpenIntent;
use t3code_web::components::ui::toast::Toaster;

fn palette(check: &mut Check) -> Entity<t3code_web::components::command_palette::CommandPalette> {
    let shortcuts = check.shortcuts.clone();
    check.cx.update(|cx| shortcuts.read(cx).palette().clone())
}

fn titles(check: &mut Check) -> Vec<String> {
    let palette = palette(check);
    check.cx.update(|cx| palette.read(cx).shown_titles().iter().map(|t| t.to_string()).collect())
}

fn query(check: &mut Check) -> String {
    let palette = palette(check);
    check.cx.update(|cx| palette.read(cx).query().map(|q| q.to_string()).unwrap_or_default())
}

fn open_sources(check: &mut Check) {
    let palette = palette(check);
    check.cx.update_window(check.window, |_, w, cx| palette.update(cx, |p, cx| p.open(OpenIntent::AddProject, w, cx))).ok();
    check.wait(Duration::from_millis(400));
}

/// Arrows down to the row titled `title` and runs it.
fn run_row(check: &mut Check, title: &str) -> anyhow::Result<()> {
    let index = titles(check).iter().position(|t| t == title).with_context(|| format!("row {title} in {:?}", titles(check)))?;
    for _ in 0..=index {
        check.press("down");
    }
    check.press("enter");
    check.wait(Duration::from_millis(400));
    Ok(())
}

/// Clears the palette query and types `text`.
fn set_query(check: &mut Check, text: &str) {
    for _ in 0..query(check).chars().count() {
        check.press("backspace");
    }
    check.type_text(text);
    check.wait(Duration::from_millis(300));
}

/// The project the server's shell lists at `root`, if any.
fn project_at(check: &mut Check, root: &str) -> Option<String> {
    let root = root.trim_end_matches('/').to_string();
    check.cx.update(|cx| {
        let environments = StateBridge::environments(cx).read(cx).list.clone();
        environments.iter().find_map(|env| {
            let env = env.read(cx);
            env.shell.snapshot.as_ref()?.projects.iter().find(|p| p.workspace_root.trim_end_matches('/') == root).map(|p| p.title.clone())
        })
    })
}

fn wait_project(check: &mut Check, root: &str, what: &str) -> anyhow::Result<String> {
    let path = root.to_string();
    check.pump(move |c| project_at(c, &path).is_some(), Duration::from_secs(30), what)?;
    Ok(project_at(check, root).unwrap_or_default())
}

fn toast_shows(check: &mut Check, prefix: &str) -> bool {
    check.cx.update(|cx| Toaster::titles(cx).iter().any(|t| t.starts_with(prefix)))
}

pub(super) fn run_add_project(check: &mut Check) -> anyhow::Result<()> {
    let root = std::env::var("T3_ADD_ROOT").context("T3_ADD_ROOT names the folder to add projects from")?;
    let root = root.trim_end_matches('/').to_string();
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let stamp = now_ms() % 100_000;

    open_sources(check);
    let rows = titles(check);
    // The default host follows, ready or "Setup Required" (`sortAddProjectProviderSources`).
    check.check("Sources lists New project, Local folder and Git URL", rows.starts_with(&["New project".to_owned(), "Local folder".to_owned(), "Git URL".to_owned()]) && rows.get(3).is_none_or(|r| r == "GitHub repository"), format!("{rows:?}"));
    check.shot("add-project-00-sources.png")?;

    // Local folder: the listing comes from the server.
    run_row(check, "Local folder")?;
    let start = query(check);
    check.check("Local folder opens on the base directory", start == "~/", start.clone());
    set_query(check, &format!("{root}/"));
    check.pump(|c| titles(c).iter().any(|t| t == "alpha"), Duration::from_secs(10), "filesystem.browse lists alpha")?;
    let rows = titles(check);
    check.check("the folder lists .. and alpha", rows.first().map(String::as_str) == Some("..") && rows.iter().any(|t| t == "alpha"), format!("{rows:?}"));
    check.shot("add-project-01-browse.png")?;
    check.type_text("al");
    check.press("tab");
    check.wait(Duration::from_millis(300));
    let entered = query(check);
    check.check("Tab enters alpha", entered == format!("{root}/alpha/"), entered.clone());
    check.press("enter");
    let alpha = format!("{root}/alpha");
    let title = wait_project(check, &alpha, "the server lists the alpha project")?;
    check.check("the server's alpha project is titled alpha", title == "alpha", title.clone());
    check.pump(|c| matches!(c.route(), Route::Draft(_)), Duration::from_secs(5), "a draft opens in alpha")?;
    check.shot("add-project-02-added.png")?;

    // Adding the same folder again opens it instead of creating a second project.
    open_sources(check);
    run_row(check, "Local folder")?;
    set_query(check, &format!("{alpha}/"));
    check.pump(|c| titles(c).first().is_some_and(|t| t == ".."), Duration::from_secs(10), "alpha lists")?;
    check.press("enter");
    check.wait(Duration::from_millis(800));
    let count = check.cx.update(|cx| {
        let environments = StateBridge::environments(cx).read(cx).list.clone();
        environments.iter().filter_map(|env| env.read(cx).shell.snapshot.clone()).map(|s| s.projects.iter().filter(|p| p.workspace_root.trim_end_matches('/') == alpha).count()).sum::<usize>()
    });
    check.check("adding alpha again keeps one project", count == 1, format!("{count} projects at alpha"));

    // A missing folder: Create & Add makes it.
    open_sources(check);
    run_row(check, "Local folder")?;
    let gamma = format!("{root}/gamma-{stamp}");
    set_query(check, &gamma);
    check.pump(|c| titles(c) == [".."], Duration::from_secs(10), "the missing folder shows only ..")?;
    check.shot("add-project-03-create.png")?;
    check.press("enter");
    wait_project(check, &gamma, "the server lists the created folder's project")?;
    check.check("the server created the folder", std::path::Path::new(&gamma).is_dir(), gamma.clone());

    // Git URL: clone the local bare repository.
    open_sources(check);
    run_row(check, "Git URL")?;
    check.shot("add-project-04-clone-url.png")?;
    check.type_text(&format!("{root}/widgets.git"));
    check.press("enter");
    check.wait(Duration::from_millis(300));
    let destination = query(check);
    check.check("the destination defaults to ~/widgets", destination == "~/widgets", destination.clone());
    check.shot("add-project-05-clone-destination.png")?;
    let clone = format!("{root}/widgets-clone-{stamp}");
    set_query(check, &clone);
    check.press("enter");
    wait_project(check, &clone, "the server lists the cloned project")?;
    check.pump(|c| toast_shows(c, "Cloned "), Duration::from_secs(60), "the Cloned toast shows")?;
    check.shot("add-project-06-cloned.png")?;
    check.check("the clone has the repository's README", std::path::Path::new(&clone).join("README.md").is_file(), clone.clone());

    // A URL with no repository behind it fails with the clone toast.
    open_sources(check);
    run_row(check, "Git URL")?;
    check.type_text(&format!("{root}/missing-{stamp}.git"));
    check.press("enter");
    check.wait(Duration::from_millis(300));
    set_query(check, &format!("{root}/missing-clone-{stamp}"));
    check.press("enter");
    check.pump(|c| toast_shows(c, "Failed to clone ") || toast_shows(c, "Clone failed"), Duration::from_secs(60), "the failed clone toasts")?;
    check.shot("add-project-07-clone-failed.png")?;
    let palette_entity = palette(check);
    check.pump(move |c| !c.cx.update(|cx| palette_entity.read(cx).is_open()), Duration::from_secs(10), "the palette closes once the clone starts")?;

    // New project.
    open_sources(check);
    run_row(check, "New project")?;
    let name = format!("Quokka Notes {stamp}");
    check.type_text(&name);
    check.wait(Duration::from_millis(200));
    check.shot("add-project-08-new-project.png")?;
    check.press("enter");
    let n = name.clone();
    check.pump(
        move |c| {
            let n = n.clone();
            c.cx.update(|cx| {
                let environments = StateBridge::environments(cx).read(cx).list.clone();
                environments.iter().any(|env| env.read(cx).shell.snapshot.as_ref().is_some_and(|s| s.projects.iter().any(|p| p.title == n)))
            })
        },
        Duration::from_secs(30),
        "the server lists the new project",
    )?;
    check.pump(|c| toast_shows(c, "Created "), Duration::from_secs(10), "the Created toast shows")?;
    check.pump(|c| matches!(c.route(), Route::Draft(_)), Duration::from_secs(10), "a draft opens in the new project")?;
    check.shot("add-project-09-created.png")?;
    Ok(())
}
