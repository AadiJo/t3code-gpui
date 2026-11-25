// live-check phase `github-source`, after `seed`, on a server whose `gh` is a stand-in signed in
// as `quokka-dev` that sends GitHub API calls to a fake on a unix socket (`T3_FAKE_GITHUB` names
// the fake's state folder: `requests.jsonl` and `repos/quokka-dev/<name>`, the bare repository a
// publish pushes to). Nothing reaches GitHub. The Sources page lists "GitHub repository"; `owner/`
// is refused; an unknown repository fails its lookup; example-org/widgets is looked up and cloned
// (the server's git serves it from a local repository). New project offers "Create private
// repository on GitHub", and a checked one is created and published: the fake records a private
// repository and the bare repository receives the push. A clone this app did not start, failing,
// toasts with Remove project, which deletes the project. Each result is read back from the server
// or the fake.

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
    check.wait(Duration::from_millis(600));
}

fn close_palette(check: &mut Check) {
    let palette = palette(check);
    check.cx.update_window(check.window, |_, w, cx| palette.update(cx, |p, cx| p.close(w, cx))).ok();
    check.wait(Duration::from_millis(200));
}

/// Arrows to the row whose value is `value` and runs it.
fn run_value(check: &mut Check, value: &str) -> anyhow::Result<()> {
    let palette = palette(check);
    for _ in 0..20 {
        if check.cx.update(|cx| palette.read(cx).target_value()).as_deref() == Some(value) {
            check.press("enter");
            check.wait(Duration::from_millis(400));
            return Ok(());
        }
        check.press("down");
    }
    bail!("no row {value} in {:?}", titles(check))
}

fn set_query(check: &mut Check, text: &str) {
    for _ in 0..query(check).chars().count() {
        check.press("backspace");
    }
    check.type_text(text);
    check.wait(Duration::from_millis(300));
}

fn toast_shows(check: &mut Check, title: &str) -> bool {
    check.cx.update(|cx| Toaster::titles(cx).iter().any(|t| t == title))
}

/// The server's projects as (id, workspace root).
fn projects(check: &mut Check) -> Vec<(ProjectId, String)> {
    check.cx.update(|cx| {
        let environments = StateBridge::environments(cx).read(cx).list.clone();
        environments
            .iter()
            .flat_map(|env| env.read(cx).shell.snapshot.as_ref().map(|s| s.projects.iter().map(|p| (p.id.clone(), p.workspace_root.clone())).collect::<Vec<_>>()).unwrap_or_default())
            .collect()
    })
}

fn fake_requests(state: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(state.join("requests.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

pub(super) fn run_github_source(check: &mut Check) -> anyhow::Result<()> {
    let fake = PathBuf::from(std::env::var("T3_FAKE_GITHUB").context("T3_FAKE_GITHUB names the fake GitHub's state folder")?);
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    let stamp = now_ms() % 100_000;

    // The first open reads discovery; the Sources page offers GitHub from then on.
    open_sources(check);
    check.wait(Duration::from_secs(3));
    close_palette(check);
    open_sources(check);
    let shown = titles(check);
    check.check("the Sources page offers GitHub repository", shown.iter().any(|t| t == "GitHub repository"), format!("{shown:?}"));
    check.shot("github-00-sources.png")?;
    let environment = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.to_string());
    run_value(check, &format!("action:add-project:{environment}:github"))?;
    check.shot("github-01-repository-step.png")?;

    set_query(check, "example-org/");
    check.press("enter");
    check.wait(Duration::from_millis(300));
    let refused = toast_shows(check, "Repository name incomplete");
    check.check("owner/ alone is refused", refused, "");

    set_query(check, "example-org/missing-repo");
    check.press("enter");
    check.pump(|c| toast_shows(c, "Repository lookup failed"), Duration::from_secs(20), "the unknown repository's lookup fails")?;
    let asked = fake_requests(&fake).iter().any(|r| r["path"] == "/repos/example-org/missing-repo");
    check.check("the lookup reached the fake GitHub, not the network", asked, "");

    set_query(check, "example-org/widgets");
    check.press("enter");
    check.pump(|c| query(c).starts_with('~') && query(c).ends_with("/widgets"), Duration::from_secs(20), "the lookup leads to the destination")?;
    let destination = query(check);
    check.note(format!("destination {destination}"));
    check.shot("github-02-destination.png")?;
    let home = std::env::var("T3_SERVER_HOME").context("T3_SERVER_HOME names the server's home")?;
    // A folder of its own each run, so a clone from an earlier run cannot answer for this one.
    let destination = format!("~/widgets-{stamp}");
    set_query(check, &destination);
    let root = destination.replacen('~', &home, 1);
    let before = projects(check).len();
    check.press("enter");
    let r = root.clone();
    check.pump(move |c| projects(c).iter().any(|(_, w)| w.trim_end_matches('/') == r.trim_end_matches('/')), Duration::from_secs(30), "the server lists the cloned project")?;
    let cloned = Path::new(&root).join("README.md").exists();
    check.check("the clone has the repository's files", cloned, &root);
    let after = projects(check).len();
    check.check("one project was added", after == before + 1, format!("{before} -> {after}"));
    close_palette(check);

    // New project, published as a private repository.
    open_sources(check);
    run_value(check, &format!("action:add-project:{environment}:new"))?;
    let name = format!("Quokka Notes {stamp}");
    set_query(check, &name);
    let shown = titles(check);
    check.check("New project offers Create private repository on GitHub", shown.iter().any(|t| t == "Create private repository on GitHub"), format!("{shown:?}"));
    // Arrow onto the row (Enter with nothing highlighted creates the project).
    check.press("down");
    let p = palette(check);
    let target = check.cx.update(|cx| p.read(cx).target_value());
    check.check("the publish row is highlighted", target.as_deref() == Some("new-project:publish"), format!("{target:?}"));
    check.press("enter");
    check.wait(Duration::from_millis(300));
    check.shot("github-03-new-project-publish.png")?;
    check.press("ctrl-enter");
    let folder = format!("quokka-notes-{stamp}");
    check.pump(|c| toast_shows(c, "Published to GitHub") || toast_shows(c, "Could not create the GitHub repository"), Duration::from_secs(30), "the publish toast")?;
    let published = toast_shows(check, "Published to GitHub");
    let shown = check.cx.update(|cx| Toaster::titles(cx));
    check.check("the publish succeeds", published, format!("{shown:?}"));
    let f = folder.clone();
    check.pump(move |c| projects(c).iter().any(|(_, w)| w.ends_with(&f)), Duration::from_secs(30), "the server lists the new project")?;
    let created = fake_requests(&fake).iter().any(|r| r["method"] == "POST" && r["path"] == "/user/repos" && r["body"]["name"] == folder.as_str() && r["body"]["private"] == true);
    check.check("the fake GitHub created a private repository", created, &folder);
    let bare = fake.join("repos/quokka-dev").join(format!("{folder}.git"));
    let pushed = std::process::Command::new("git").args(["-C", &bare.display().to_string(), "for-each-ref", "--format=%(refname)"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
    check.check("the new project's branch was pushed", !pushed.is_empty(), &pushed);

    // A clone this app did not start: its failure toasts with Remove project.
    let id = t3code_contracts::EnvironmentId::new(environment.as_str());
    let link = check.cx.update(|cx| t3code_web::live::rpc::RpcLink::new(&id, cx)).context("rpc link")?;
    let destination = format!("{home}/elsewhere-{stamp}");
    let start = link.call::<t3code_contracts::rpc::methods::ProjectCloneStart>(t3code_contracts::ProjectCloneStartInput {
        project_id: ProjectId::new(uuid::Uuid::new_v4().to_string()),
        title: format!("elsewhere-{stamp}"),
        created_at: t3code_web::state::sidebar_actions::now_iso(),
        provider: None,
        repository: None,
        remote_url: Some(format!("{}/does-not-exist.git", fake.display())),
        destination_path: destination.clone(),
        protocol: None,
    });
    let started = check.block(start)?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let title = format!("Failed to clone elsewhere-{stamp}");
    let t = title.clone();
    check.pump(move |c| toast_shows(c, &t), Duration::from_secs(30), "the failed clone toasts")?;
    let toast = check.cx.update(|cx| Toaster::find(&title, cx)).context("toast")?;
    check.check("the failed clone offers Remove project and Retry", toast.secondary_actions.first().map(|a| a.0.as_ref()) == Some("Remove project") && toast.action.as_ref().map(|a| a.0.as_ref()) == Some("Retry"), "");
    check.shot("github-04-clone-failed-toast.png")?;
    if let Some((_, remove)) = toast.secondary_actions.into_iter().next() {
        check.cx.update_window(check.window, |_, w, cx| remove(w, cx)).ok();
    }
    let gone = started.project_id.clone();
    check.pump(move |c| !projects(c).iter().any(|(p, _)| *p == gone), Duration::from_secs(30), "Remove project deletes the project")?;
    Ok(())
}
