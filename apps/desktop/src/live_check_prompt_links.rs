// live-check phase `prompt-links`, after `seed`: prompt links opened the way the OS hands them to
// the app (`t3code-gpui://new?…` and the hosted `https://app.t3.codes/new?…`). The phase makes a
// project whose origin is github.com/example-org/quokka-notes (never fetched). A link naming it
// lands in a draft there with the prompt and the link's model; an unknown model falls back to
// the provider's default; a missing repository offers a clone, and cloning
// github.com/example-org/widgets (served from a local bare repository through the server's git
// `insteadOf`, `T3_CLONE_ROOT` names its folder) opens a draft in the new project; a pull request
// link asks before checking out; a link with no prompt fails. "Copy prompt link with repo and
// model" copies a link that opens the same draft. `/welcome` shows the no-projects hero.
// Projects and drafts are read back from the server's shell.

use super::*;
use t3code_shared::prompt_links::parse_new_thread_link;
use t3code_web::command_palette_store::OpenIntent;
use t3code_web::components::prompt_links::new_thread_link_view::LinkState;
use t3code_web::components::ui::toast::Toaster;
use t3code_web::router::route_from_app_link;

const SCHEME: &str = "t3code-gpui";
const REPOSITORY: &str = "example-org/quokka-notes";

/// Opens `url` as the app does for an OS link and returns the `/new` page's state once settled
/// (`Opened` when the link already replaced the page with what it opened).
fn open_link(check: &mut Check, url: &str, settled: fn(&LinkState) -> bool, what: &str) -> anyhow::Result<LinkState> {
    let route = route_from_app_link(url, SCHEME).with_context(|| format!("{url} is a link the app opens"))?;
    let root = check.root.clone();
    check.cx.update(|cx| root.read(cx).router().clone().update(cx, |router, cx| router.navigate(route, cx)));
    check.frame();
    let state = |c: &mut Check| {
        let root = c.root.clone();
        c.cx.update(|cx| match root.read(cx).router().read(cx).current() {
            Route::New { .. } => root.read(cx).new_thread_link_view(cx).map(|v| v.read(cx).state().clone()),
            _ => Some(LinkState::Opened),
        })
    };
    check.pump(move |c| state(c).is_some_and(|s| settled(&s)), Duration::from_secs(30), what)?;
    state(check).context("link view")
}

fn current_route(check: &mut Check) -> Route {
    let root = check.root.clone();
    check.cx.update(|cx| root.read(cx).router().read(cx).current().clone())
}

/// Waits for the route to become a draft and returns its project's grouping key on the server,
/// the composer's text and its model.
fn landed_draft(check: &mut Check, what: &str) -> anyhow::Result<(String, String, String)> {
    check.pump(|c| matches!(current_route(c), Route::Draft(_)), Duration::from_secs(30), what)?;
    check.wait(Duration::from_millis(500));
    let Route::Draft(draft) = current_route(check) else { bail!("not a draft") };
    let root = check.root.clone();
    let project = check.read(|root, cx| root.sidebar().read(cx).snapshot().drafts.iter().find(|d| d.id == draft.0).map(|d| d.project.clone())).context("draft project")?;
    let key = check.cx.update(|cx| {
        let env = StateBridge::environment(&t3code_contracts::EnvironmentId::new(project.environment_id.0.as_ref()), cx)?;
        let env = env.read(cx);
        let p = env.shell.snapshot.as_ref()?.projects.iter().find(|p| p.id.0.as_ref() == project.project_id.0.as_ref())?.clone();
        Some(p.repository_identity.as_ref().map(|i| i.grouping_key().to_owned()).unwrap_or_else(|| p.workspace_root.clone()))
    });
    let (text, model) = check.cx.update(|cx| {
        let composer = root.read(cx).composer(cx);
        let composer = composer.read(cx);
        (composer.editor().read(cx).text().to_string(), format!("{}:{}", composer.state().selection.instance_id.0, composer.state().selection.model))
    });
    Ok((key.unwrap_or_default(), text, model))
}

fn toast(check: &mut Check, title: &str) -> bool {
    check.cx.update(|cx| Toaster::titles(cx).iter().any(|t| t == title))
}

/// Makes the GitHub-origin project (a local git repository with that remote) unless the server has it.
fn ensure_project(check: &mut Check, state: &Seeded) -> anyhow::Result<()> {
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    let has = |c: &mut Check| {
        let id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
        c.cx.update(|cx| {
            StateBridge::environment(&id, cx).is_some_and(|env| {
                env.read(cx).shell.snapshot.as_ref().is_some_and(|s| {
                    s.projects.iter().any(|p| p.repository_identity.as_ref().is_some_and(|i| i.grouping_key() == format!("github.com/{REPOSITORY}")))
                })
            })
        })
    };
    if has(check) {
        return Ok(());
    }
    let root = check.out.join(format!("workspaces/quokka-notes-{}", now_ms() % 100_000));
    std::fs::create_dir_all(&root)?;
    std::fs::write(root.join("README.md"), "# quokka-notes\n")?;
    let git = |args: &[&str]| std::process::Command::new("git").args(["-c", "user.name=Scenario", "-c", "user.email=scenario@example.invalid"]).args(args).current_dir(&root).status();
    git(&["init", "-q", "--initial-branch=main"])?;
    git(&["add", "-A"])?;
    git(&["commit", "-qm", "chore: initial commit"])?;
    git(&["remote", "add", "origin", &format!("https://github.com/{REPOSITORY}.git")])?;
    let commands = commands_of(check, &environment_id)?;
    let mutation = ProjectMutation::ProjectCreate(ProjectMutationProjectCreate {
        command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
        project_id: ProjectId::new(uuid::Uuid::new_v4().to_string()),
        title: "quokka-notes".into(),
        workspace_root: root.display().to_string(),
        create_workspace_root_if_missing: Some(false),
        default_model_selection: None,
        scripts: None,
    });
    check.block(async move { commands.mutate_project(mutation).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    check.pump(move |c| has(c), Duration::from_secs(30), "the server lists the GitHub project")
}

/// The first ready provider's driver, its default model and another model, from the server.
fn provider(check: &mut Check, state: &Seeded) -> Option<(String, String, String, Option<String>)> {
    let id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    check.cx.update(|cx| {
        let config = StateBridge::environment(&id, cx)?.read(cx).config.clone()?;
        let p = config.providers.iter().find(|p| p.enabled && p.status == t3code_contracts::ServerProviderState::Ready && !p.models.is_empty())?;
        let default = p.models.iter().find(|m| m.is_default == Some(true)).unwrap_or(&p.models[0]).slug.clone();
        let other = p.models.iter().map(|m| m.slug.clone()).find(|s| *s != default);
        Some((p.instance_id.clone(), p.driver.clone(), default, other))
    })
}

pub(super) fn run_prompt_links(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    ensure_project(check, &state)?;
    // Shots compare against the fork capture at the owner's settings (dark Graphite).
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    let (instance, driver, default_model, other_model) = provider(check, &state).context("a ready provider")?;
    check.note(format!("provider {instance} ({driver}), default {default_model}, other {other_model:?}"));

    // A repository that is not here.
    let missing = open_link(check, &format!("{SCHEME}://new?prompt=Fix+the+flaky+test&repo=example-org/absent-repo"), |s| !matches!(s, LinkState::Opening), "the missing-repository link settles")?;
    check.check("a link to a missing repository offers a clone", matches!(&missing, LinkState::MissingRepository(r) if r.path() == "example-org/absent-repo"), format!("{missing:?}"));
    check.shot("links-00-missing-repo.png")?;

    // No prompt: the route fails to read it.
    let route = route_from_app_link(&format!("https://app.t3.codes/new?repo={REPOSITORY}"), SCHEME);
    check.check("the hosted web link opens /new", matches!(&route, Some(Route::New { query }) if query.as_ref() == format!("repo={REPOSITORY}")), format!("{route:?}"));
    let root = check.root.clone();
    if let Some(route) = route {
        check.cx.update(|cx| root.read(cx).router().clone().update(cx, |router, cx| router.navigate(route, cx)));
    }
    check.wait(Duration::from_millis(300));
    let error = match current_route(check) {
        Route::New { query } => parse_new_thread_link(&query).err(),
        _ => None,
    };
    check.check("a link with no prompt is refused", error == Some(t3code_shared::prompt_links::PromptLinkError::MissingPrompt), format!("{error:?}"));
    check.shot("links-01-no-prompt.png")?;

    // A pull request link asks before checking out.
    let pr = open_link(check, &format!("{SCHEME}://new?pr={REPOSITORY}%2342&prompt=Review+it"), |s| !matches!(s, LinkState::Opening), "the pull request link settles")?;
    check.check("a pull request link asks before checking out", matches!(pr, LinkState::PullRequestCheckout { checking_out: false, .. }), format!("{pr:?}"));
    check.shot("links-02-pr-checkout.png")?;

    // `/welcome`.
    let root = check.root.clone();
    check.cx.update(|cx| root.read(cx).router().clone().update(cx, |router, cx| router.navigate(Route::Welcome, cx)));
    check.wait(Duration::from_millis(400));
    check.shot("links-05-welcome.png")?;

    // The repository's project, with the link's model.
    let link = format!("{SCHEME}://new?prompt=Draft+the+release+notes&repo={REPOSITORY}&provider={driver}&model={}", other_model.as_deref().unwrap_or(&default_model));
    open_link(check, &link, |s| !matches!(s, LinkState::Opening), "the draft link settles")?;
    let (key, text, model) = landed_draft(check, "the link opens a draft")?;
    check.check("the draft is in the linked repository's project", key == format!("github.com/{REPOSITORY}"), &key);
    check.check("the composer holds the linked prompt", text == "Draft the release notes", &text);
    let want = format!("{instance}:{}", other_model.as_deref().unwrap_or(&default_model));
    check.check("the composer uses the link's model", model == want, format!("{model} (want {want})"));
    let shown = toast(check, "Prompt added from a link");
    check.check("the prompt toast shows", shown, "");
    check.shot("links-03-draft-landed.png")?;

    // An unknown model: the provider's default stands in.
    open_link(check, &format!("{SCHEME}://new?prompt=Second+link&repo={REPOSITORY}&provider={driver}&model=no-such-model"), |s| !matches!(s, LinkState::Opening), "the fallback link settles")?;
    let (_, text, model) = landed_draft(check, "the fallback link opens a draft")?;
    check.check("an unknown model falls back to the provider's default", model == format!("{instance}:{default_model}") && text == "Second link", format!("{model} {text:?}"));

    // Copy prompt link with repo and model, from that draft's composer.
    let shortcuts = check.shortcuts.clone();
    let palette = check.cx.update(|cx| shortcuts.read(cx).palette().clone());
    check.cx.update_window(check.window, |_, w, cx| palette.update(cx, |p, cx| p.open(OpenIntent::Search("Copy prompt link".into()), w, cx))).ok();
    check.wait(Duration::from_millis(400));
    let titles: Vec<String> = check.cx.update(|cx| palette.read(cx).shown_titles().iter().map(|t| t.to_string()).collect());
    check.check("the palette offers both prompt link rows", titles.iter().any(|t| t == "Copy prompt link") && titles.iter().any(|t| t == "Copy prompt link with repo and model"), format!("{titles:?}"));
    // Arrow to the row (the highlight is `target_value`), then run it.
    for _ in 0..titles.len() + 1 {
        if check.cx.update(|cx| palette.read(cx).target_value()).as_deref() == Some("action:copy-prompt-link-with-context") {
            break;
        }
        check.press("down");
    }
    check.press("enter");
    check.wait(Duration::from_millis(300));
    let copied = check.cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default());
    let want = format!("https://app.t3.codes/new?prompt=Second+link&repo={}&provider={driver}&model={default_model}", REPOSITORY.replace('/', "%2F"));
    check.check("the copied link names the prompt, repository and model", copied == want, format!("{copied} (want {want})"));
    std::fs::write(check.out.join("copied-prompt-link.txt"), &copied)?;
    open_link(check, &copied, |s| !matches!(s, LinkState::Opening), "the copied link settles")?;
    let (key, text, model) = landed_draft(check, "the copied link opens a draft")?;
    check.check("the copied link opens the same draft", key == format!("github.com/{REPOSITORY}") && text == "Second link" && model == format!("{instance}:{default_model}"), format!("{key} {text:?} {model}"));

    // Clone a missing repository from the link, then land in its project.
    let clone_root = std::env::var("T3_CLONE_ROOT").ok();
    let missing = open_link(check, &format!("{SCHEME}://new?prompt=Look+around&repo=example-org/widgets"), |s| !matches!(s, LinkState::Opening), "the widgets link settles")?;
    let destination = match &missing {
        LinkState::MissingRepository(repository) => check.cx.update(|cx| t3code_web::state::prompt_links::clone_target(repository, cx)).map(|(_, _, d)| d),
        _ => None,
    };
    let fresh = destination.as_deref().is_some_and(|d| !Path::new(d).exists());
    if let (LinkState::MissingRepository(repository), Some(_), true) = (missing, clone_root, fresh) {
        let view = check.read(|root, cx| root.new_thread_link_view(cx)).context("link view")?;
        check.cx.update(|cx| view.update(cx, |v, cx| v.clone_repository(repository, cx)));
        let (key, text, _) = landed_draft(check, "the clone opens a draft")?;
        // The server's git serves this URL from a local repository, so its identity is the folder.
        check.check("the clone lands in the cloned project", key == "github.com/example-org/widgets" || key.ends_with("/widgets"), &key);
        check.check("the cloned project's draft holds the prompt", text == "Look around", &text);
        check.shot("links-04-cloned.png")?;
    } else {
        check.note(format!("skipped the clone: set T3_CLONE_ROOT on a fresh server (destination {destination:?})"));
    }

    Ok(())
}
