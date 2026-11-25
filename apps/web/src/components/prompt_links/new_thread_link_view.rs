//! The `/new` page (`_chat.new.tsx`, `NewThreadLinkView.tsx`): reads a prompt link, waits for
//! every reachable shell, then opens the pull request's thread, a draft in the repository's
//! most recent project or a projectless draft, or asks to clone or check out first. The app
//! root does the opening through [`NewThreadLinkEvent`].

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use t3code_contracts::rpc::methods::GitPreparePullRequestThread;
use t3code_contracts::{GitPreparePullRequestThreadInput, ModelSelection, ThreadEnvMode};
use t3code_shared::prompt_links::{LinkedPrompt, NewThreadLink, PROMPT_LINK_MAX_PROMPT_LENGTH, PromptLinkError, PromptLinkRepository, parse_new_thread_link};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::command_palette_project_flow::ProjectFlowBackend;
use crate::components::sidebar::model::{ProjectRef, ThreadKey};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::state::StateBridge;
use crate::state::prompt_links::{self as live, LinkDecision, LinkProject};

/// `LinkState`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    Opening,
    Opened,
    Failed { title: Option<SharedString>, message: SharedString },
    MissingRepository(PromptLinkRepository),
    PullRequestCheckout { project: LinkProject, checking_out: bool },
}

/// What the app root opens for the link.
#[derive(Clone, Debug)]
pub enum NewThreadLinkEvent {
    /// A new draft in `project` with the prompt and model placed (`openDraft`, `placeLinkedPrompt`).
    OpenDraft { project: ProjectRef, prompt: Option<LinkedPrompt>, model: Option<ModelSelection>, notes: Vec<SharedString> },
    /// A draft on a checked-out pull request branch.
    OpenDraftOnBranch { project: ProjectRef, branch: String, worktree_path: Option<String>, prompt: Option<LinkedPrompt> },
    /// The pull request's existing thread, with the prompt placed in its composer.
    OpenThread { key: ThreadKey, prompt: Option<LinkedPrompt> },
}

/// The `/new` page for one link (one history entry).
pub struct NewThreadLinkView {
    link: Result<NewThreadLink, PromptLinkError>,
    state: LinkState,
    flows: Option<Rc<dyn ProjectFlowBackend>>,
    topbar: Pixels,
    _task: Option<Task<()>>,
}

impl EventEmitter<NewThreadLinkEvent> for NewThreadLinkView {}

impl NewThreadLinkView {
    /// Reads `query` (the raw search string) and starts opening it on live data.
    pub fn new(query: &str, flows: Option<Rc<dyn ProjectFlowBackend>>, cx: &mut Context<Self>) -> Self {
        let mut this = Self { link: parse_new_thread_link(query), state: LinkState::Opening, flows, topbar: px(40.), _task: None };
        this.start(cx);
        this
    }

    /// A page in a fixed state, for scenes.
    pub fn with_state(query: &str, state: LinkState) -> Self {
        Self { link: parse_new_thread_link(query), state, flows: None, topbar: px(40.), _task: None }
    }

    pub fn state(&self) -> &LinkState {
        &self.state
    }

    /// The window's titlebar height, which the page's header strip keeps.
    pub fn set_topbar(&mut self, topbar: Pixels) {
        self.topbar = topbar;
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let Ok(link) = self.link.clone() else { return };
        if !StateBridge::is_running(cx) {
            return;
        }
        self.state = LinkState::Opening;
        let ready = live::wait_until_ready(cx);
        self._task = Some(cx.spawn(async move |this, cx| {
            ready.await;
            let decision = cx.update(|cx| live::decide(&link, cx)).await;
            this.update(cx, |this, cx| this.launch(link, decision, cx)).ok();
        }));
        cx.notify();
    }

    fn launch(&mut self, link: NewThreadLink, decision: LinkDecision, cx: &mut Context<Self>) {
        match decision {
            LinkDecision::Thread(key) => {
                self.state = LinkState::Opened;
                cx.emit(NewThreadLinkEvent::OpenThread { key, prompt: link.prompt().cloned() });
            }
            LinkDecision::Scratch => self.open_scratch(link, cx),
            LinkDecision::Draft(project) => self.open_draft(&link, project.project, None, cx),
            LinkDecision::MissingRepository(repository) => self.state = LinkState::MissingRepository(repository),
            LinkDecision::PullRequestCheckout(project) => {
                if let Some(prompt) = link.prompt() {
                    crate::components::prompt_links::warn_about_hidden_characters(prompt, cx);
                }
                self.state = LinkState::PullRequestCheckout { project, checking_out: false };
            }
        }
        cx.notify();
    }

    /// `openDraft`: the prompt (and `extra`, the pull request URL for a projectless draft) and
    /// the link's model in a new draft.
    fn open_draft(&mut self, link: &NewThreadLink, project: ProjectRef, extra: Option<String>, cx: &mut Context<Self>) {
        self.state = LinkState::Opened;
        let (model, note) = live::link_model(link, &project, cx);
        let text = [link.prompt().map(|p| p.text.clone()), extra].into_iter().flatten().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
        let prompt = (!text.is_empty()).then(|| LinkedPrompt { text, hidden_characters: link.prompt().map_or(0, |p| p.hidden_characters) });
        cx.emit(NewThreadLinkEvent::OpenDraft { project, prompt, model, notes: note.into_iter().collect() });
        cx.notify();
    }

    /// `openScratchDraft`: a No project draft on the last active machine that offers one.
    fn open_scratch(&mut self, link: NewThreadLink, cx: &mut Context<Self>) {
        let preferred = live::last_active_environment(cx);
        let Some(environment) = crate::state::palette::scratch_environment_for(preferred, cx) else {
            self.state = LinkState::Failed { title: None, message: t!("None of your machines can start a thread without a project right now.") };
            return;
        };
        self.state = LinkState::Opening;
        let task = crate::state::palette::start_scratch_thread(environment, cx);
        self._task = Some(cx.spawn(async move |this, cx| {
            let project = task.await;
            this.update(cx, |this, cx| match project {
                Some(project) => {
                    let extra = match &link {
                        NewThreadLink::PullRequest { pull_request, .. } => Some(pull_request.url()),
                        NewThreadLink::New { .. } => None,
                    };
                    this.open_draft(&link, project, extra, cx);
                }
                None => {
                    this.state = LinkState::Failed { title: None, message: t!("None of your machines can start a thread without a project right now.") };
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// "Open it again": decides again, as a fresh visit would.
    fn reopen(&mut self, cx: &mut Context<Self>) {
        self.start(cx);
    }

    /// "Use without project".
    pub fn use_without_project(&mut self, cx: &mut Context<Self>) {
        let Ok(link) = self.link.clone() else { return };
        self.open_scratch(link, cx);
        cx.notify();
    }

    /// `cloneRepository`: clones into the last active machine's projects folder, then opens a
    /// draft there (a pull request link asks to be opened again once the clone finishes).
    pub fn clone_repository(&mut self, repository: PromptLinkRepository, cx: &mut Context<Self>) {
        let Ok(link) = self.link.clone() else { return };
        let (Some(flows), Some((environment, remote_url, destination))) = (self.flows.clone(), live::clone_target(&repository, cx)) else {
            self.state = LinkState::Failed {
                title: None,
                message: t!("This machine cannot clone from a link. Update T3 Code on it, or clone the repository from the command palette."),
            };
            cx.notify();
            return;
        };
        self.state = LinkState::Opening;
        let started = flows.start_clone(&environment, remote_url, destination, cx);
        self._task = Some(cx.spawn(async move |this, cx| {
            let Some((project, listed)) = started.await else {
                this.update(cx, |this, cx| {
                    this.state = LinkState::MissingRepository(repository);
                    cx.notify();
                })
                .ok();
                return;
            };
            listed.await;
            this.update(cx, |this, cx| {
                if matches!(link, NewThreadLink::PullRequest { .. }) {
                    this.state = LinkState::Failed {
                        title: Some(t!("Cloning {repository}", repository = repository.path().as_str())),
                        message: t!("Open this link again when the clone finishes."),
                    };
                    cx.notify();
                    return;
                }
                this.open_draft(&link, project, None, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// `PullRequestLinkCheckout`'s "Check out": the pull request in a new worktree, then a draft
    /// on its branch with the link's prompt.
    pub fn check_out(&mut self, cx: &mut Context<Self>) {
        let (Ok(NewThreadLink::PullRequest { pull_request, prompt }), LinkState::PullRequestCheckout { project, checking_out }) = (self.link.clone(), &mut self.state) else { return };
        if *checking_out {
            return;
        }
        *checking_out = true;
        let project = project.clone();
        let environment_id = t3code_contracts::EnvironmentId::new(project.project.environment_id.0.as_ref());
        let Some(link) = crate::live::rpc::RpcLink::new(&environment_id, cx) else { return };
        let request = link.call::<GitPreparePullRequestThread>(GitPreparePullRequestThreadInput {
            cwd: project.workspace_root.clone(),
            reference: pull_request.url(),
            mode: ThreadEnvMode::Worktree,
            thread_id: None,
        });
        let toast = Toaster::push(ToastData::new(ToastKind::Loading, t!("Preparing the {term, select, mr {merge request} cr {change request} other {pull request}} checkout...", term = "pr")).persistent(), cx);
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                Toaster::dismiss(toast, cx);
                match result {
                    Ok(prepared) => {
                        this.state = LinkState::Opened;
                        cx.emit(NewThreadLinkEvent::OpenDraftOnBranch {
                            project: project.project,
                            branch: prepared.branch,
                            worktree_path: prepared.worktree_path,
                            prompt,
                        });
                    }
                    Err(error) => {
                        if let LinkState::PullRequestCheckout { checking_out, .. } = &mut this.state {
                            *checking_out = false;
                        }
                        Toaster::push(ToastData::new(ToastKind::Error, t!("Could not prepare the {term, select, mr {merge request} cr {change request} other {pull request}} checkout", term = "pr")).description(error), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// `LinkPage`: the desktop header strip over a centred title, description and buttons.
    fn link_page(&self, title: SharedString, description: Option<SharedString>, buttons: Vec<AnyElement>, header: bool, window: &Window, cx: &App) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let max = theme.rem(28.);
        let balanced = |text: &str, size: TextSize, weight| {
            let rem = theme.fonts.interface;
            crate::components::prompt_links::balanced_width(text, crate::appearance_fonts::ui_font(weight), px(size.size(rem)), max, window)
        };
        let title_width = balanced(&title, TextSize::Xl, weight::SEMIBOLD);
        let description_width = description.as_deref().map(|d| balanced(d, TextSize::Sm, weight::NORMAL));
        div()
            .id("new-thread-link")
            .size_full()
            .flex()
            .flex_col()
            .when(header, |page| page.child(div().flex_none().h(self.topbar)))
            .child(
                // `Empty`: `flex-1 gap-6 p-6 md:p-12`, then `EmptyHeader className="max-w-md"`.
                div().flex_1().min_h_0().flex().flex_col().items_center().justify_center().p(theme.sp(12.)).child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .text_center()
                        .max_w(max)
                        .child(div().max_w(title_width).ui_text(theme, TextSize::Xl, weight::SEMIBOLD).text_color(c.foreground()).child(title))
                        .children(description.map(|text| {
                            div()
                                .mt(theme.sp(1.))
                                .max_w(description_width.unwrap_or(max))
                                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                                .text_color(c.muted_foreground)
                                .child(text)
                        }))
                        .when(!buttons.is_empty(), |header| {
                            header.child(div().mt(theme.sp(5.)).flex().justify_center().gap(theme.sp(2.)).children(buttons))
                        }),
                ),
            )
            .into_any_element()
    }
}

/// The message for a link that cannot be read (`promptLinksCopy.ts`).
pub fn link_error_message(error: PromptLinkError) -> SharedString {
    match error {
        PromptLinkError::PromptTooLong => t!(
            "This prompt is longer than {maxLength} characters, so T3 Code did not open it.",
            maxLength = format_thousands(PROMPT_LINK_MAX_PROMPT_LENGTH).as_str()
        ),
        PromptLinkError::PromptOnlyHidden => t!("This prompt had only hidden characters, so it may be malicious. T3 Code did not open it."),
        PromptLinkError::UnreadablePullRequest => t!("This link names a pull request T3 Code cannot read."),
        PromptLinkError::MissingPrompt => t!("This link has no prompt."),
        PromptLinkError::UnreadableRepository => t!("This link names a repository T3 Code cannot read."),
    }
}

/// `toLocaleString("en-US")` of a count.
fn format_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Render for NewThreadLinkView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let link = match &self.link {
            Ok(link) => link.clone(),
            Err(error) => return self.link_page(t!("Could not open the link"), Some(link_error_message(*error)), Vec::new(), false, window, cx),
        };
        match self.state.clone() {
            LinkState::Opening => self.link_page(t!("Opening the link..."), None, Vec::new(), true, window, cx),
            LinkState::Opened => {
                let again = Button::new("link-open-again")
                    .size(ButtonSize::Sm)
                    .variant(ButtonVariant::Outline)
                    .label(t!("Open it again"))
                    .on_click(cx.listener(|this, _, _, cx| this.reopen(cx)));
                self.link_page(t!("This link is open"), None, vec![again.into_any_element()], true, window, cx)
            }
            LinkState::Failed { title, message } => self.link_page(title.unwrap_or_else(|| t!("Could not open the link")), Some(message), Vec::new(), true, window, cx),
            LinkState::MissingRepository(repository) => {
                let name = repository.path();
                let clone = Button::new("link-clone").size(ButtonSize::Sm).label(t!("Clone {repository}", repository = name.as_str())).on_click(cx.listener(move |this, _, _, cx| this.clone_repository(repository.clone(), cx)));
                let without = Button::new("link-without-project")
                    .size(ButtonSize::Sm)
                    .variant(ButtonVariant::Outline)
                    .label(t!("Use without project"))
                    .on_click(cx.listener(|this, _, _, cx| this.use_without_project(cx)));
                self.link_page(
                    t!("You don't have {repository}", repository = name.as_str()),
                    Some(t!("Clone it to work on it here, or start without a project.")),
                    vec![clone.into_any_element(), without.into_any_element()],
                    true,
                    window,
                    cx,
                )
            }
            LinkState::PullRequestCheckout { checking_out, .. } => {
                let NewThreadLink::PullRequest { pull_request, .. } = &link else { return self.link_page(t!("Opening the link..."), None, Vec::new(), true, window, cx) };
                let name = format!("{}#{}", pull_request.repository.path(), pull_request.number);
                let button = Button::new("link-check-out")
                    .size(ButtonSize::Sm)
                    .label(if checking_out { t!("Checking out...") } else { t!("Check out") })
                    .disabled(checking_out)
                    .on_click(cx.listener(|this, _, _, cx| this.check_out(cx)));
                self.link_page(
                    t!("Check out {pullRequest}?", pullRequest = name.as_str()),
                    Some(t!("This puts the pull request's code in a new worktree and runs your project's setup script on it.")),
                    vec![button.into_any_element()],
                    true,
                    window,
                    cx,
                )
            }
        }
    }
}
