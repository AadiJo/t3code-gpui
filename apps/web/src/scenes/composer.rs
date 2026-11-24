//! The composer in its main states, side by side: empty, a multi-line draft
//! with chips and a code block, running with Stop, disabled with an error
//! banner, the model picker open, and the `@` menu open. `composer-light`
//! renders the same in the light appearance.

use crate::components::icons::IconName;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use crate::ActiveAppTheme as _;
use crate::components::chat::composer::{
    BannerTone, ChatComposer, ComposerBanner, ComposerContextStrip, ComposerState, FollowUpBehavior,
};
use crate::components::chat::composer_command_menu::{
    ComposerCommandAction, ComposerCommandIcon, ComposerCommandItem, ComposerMenuSource,
};
use crate::components::chat::composer_permissions::RuntimeMode;
use crate::components::chat::composer_prompt_history::PromptHistoryEntry;
use crate::composer_draft_store::{ComposerDraftKey, ComposerDraftStore, InMemoryDraftStore};
use crate::composer_editor_mentions::{format_context_reference, serialize_file_link};
use crate::composer_logic::{ComposerTrigger, ComposerTriggerKind, SendShortcut};
use crate::model_selection::{
    ModelCatalog, ModelOption, ModelSelection, OptionChoice, ProviderDriverKind, ProviderIcon, ProviderInstanceEntry,
    ProviderInstanceId, SelectDescriptor, provider_model_key,
};
use crate::theme_palette::Appearance;

/// Cell size for one composer state.
const CELL_WIDTH: f32 = 840.;
const CELL_HEIGHT: f32 = 540.;

fn effort(levels: &[(&str, &str)], default: &str) -> SelectDescriptor {
    SelectDescriptor {
        label: "Reasoning".into(),
        options: levels
            .iter()
            .map(|(id, label)| OptionChoice { id: (*id).into(), label: (*label).into(), is_default: *id == default })
            .collect(),
    }
}

fn model(slug: &str, name: &str, effort_levels: Option<SelectDescriptor>) -> ModelOption {
    ModelOption {
        slug: slug.into(),
        name: name.into(),
        short_name: None,
        sub_provider: None,
        is_legacy: false,
        is_unavailable: false,
        effort: effort_levels,
        speed: None,
        ultracode: false,
    }
}

/// A model list in server order (OpenAI before Claude, so the fork's reorder shows).
pub fn fixture_catalog() -> ModelCatalog {
    let codex = ProviderInstanceId("codex".into());
    let claude = ProviderInstanceId("claudeAgent".into());
    let cursor = ProviderInstanceId("cursor".into());
    let opencode = ProviderInstanceId("opencode".into());
    let gpt_effort = effort(&[("low", "Low"), ("medium", "Medium"), ("high", "High"), ("xhigh", "Extra high")], "medium");
    let claude_effort = effort(&[("low", "Low"), ("medium", "Medium"), ("high", "High"), ("max", "Max")], "high");
    let speed = SelectDescriptor {
        label: "Speed".into(),
        options: vec![
            OptionChoice { id: "standard".into(), label: "Standard".into(), is_default: true },
            OptionChoice { id: "fast".into(), label: "Fast".into(), is_default: false },
        ],
    };
    let entry = |id: &ProviderInstanceId, driver, name: &str| ProviderInstanceEntry {
        instance_id: id.clone(),
        driver_kind: driver,
        display_name: name.into(),
        ready: true,
        unavailable_reason: None,
    };
    let mut models = HashMap::new();
    models.insert(
        codex.clone(),
        vec![
            ModelOption { speed: Some(speed.clone()), ..model("gpt-5.5", "GPT-5.5", Some(gpt_effort.clone())) },
            model("gpt-5.5-codex", "GPT-5.5 Codex", Some(gpt_effort.clone())),
            model("gpt-5.4-mini", "GPT-5.4 Mini", Some(gpt_effort.clone())),
            ModelOption { is_legacy: true, ..model("gpt-5", "GPT-5", Some(gpt_effort)) },
        ],
    );
    models.insert(
        claude.clone(),
        vec![
            ModelOption { ultracode: true, ..model("claude-opus-5-5", "Claude Opus 5.5", Some(claude_effort.clone())) },
            model("claude-sonnet-5-5", "Claude Sonnet 5.5", Some(claude_effort.clone())),
            model("claude-haiku-5-5", "Claude Haiku 5.5", None),
            model("claude-fable-5-2", "Claude Fable 5.2", Some(claude_effort)),
        ],
    );
    models.insert(
        cursor.clone(),
        vec![model("composer-2", "Composer 2", None), model("grok-4", "Grok 4", None), model("gemini-3-pro", "Gemini 3 Pro", None)],
    );
    models.insert(
        opencode.clone(),
        vec![model("opencode/muse-spark-1", "Muse Spark 1", None), model("opencode/kimi-k2", "Kimi K2", None)],
    );
    let mut hidden = HashMap::new();
    hidden.insert(codex.clone(), HashSet::from([SharedString::from("gpt-5.4-mini")]));
    ModelCatalog {
        instances: vec![
            entry(&codex, ProviderDriverKind::Codex, "Codex"),
            entry(&cursor, ProviderDriverKind::Cursor, "Cursor"),
            entry(&claude, ProviderDriverKind::ClaudeAgent, "Claude"),
            entry(&opencode, ProviderDriverKind::OpenCode, "OpenCode"),
        ],
        models,
        favorites: HashSet::from([provider_model_key(&claude, "claude-opus-5-5"), provider_model_key(&codex, "gpt-5.5")]),
        hidden,
    }
}

/// Menu items from fixtures.
pub struct FixtureMenuSource;

impl ComposerMenuSource for FixtureMenuSource {
    fn items(&self, trigger: &ComposerTrigger) -> (Vec<ComposerCommandItem>, bool) {
        let q = trigger.query.to_lowercase();
        let items = match trigger.kind {
            ComposerTriggerKind::Path => {
                let files = [
                    ("apps/web/src/components/chat/composer.rs", IconName::File),
                    ("apps/web/src/components/composer_prompt_editor.rs", IconName::File),
                    ("apps/web/src/composer_logic.rs", IconName::File),
                    ("apps/web/src/components/chat/", IconName::Folder),
                    ("packages/contracts/src/rpc.rs", IconName::File),
                ];
                let mut items: Vec<_> = files
                    .iter()
                    .filter(|(p, _)| p.to_lowercase().contains(&q))
                    .map(|(path, glyph)| {
                        let trimmed = path.trim_end_matches('/');
                        let (dir, name) = trimmed.rsplit_once('/').unwrap_or(("", trimmed));
                        ComposerCommandItem {
                            id: format!("path:{path}").into(),
                            icon: ComposerCommandIcon::Icon(*glyph),
                            label: name.to_owned().into(),
                            label_prefix: None,
                            description: dir.to_owned().into(),
                            badge: None,
                            action: ComposerCommandAction::Insert(serialize_file_link(path).into()),
                        }
                    })
                    .collect();
                if "claude".contains(&q) || q.starts_with("cl") {
                    items.push(ComposerCommandItem {
                        id: "model:claude".into(),
                        icon: ComposerCommandIcon::Provider(ProviderIcon::Claude),
                        label: "Claude Opus 5.5".into(),
                        label_prefix: None,
                        description: "Claude".into(),
                        badge: None,
                        action: ComposerCommandAction::Model {
                            instance_id: ProviderInstanceId("claudeAgent".into()),
                            model: "claude-opus-5-5".into(),
                            insert: "model:claudeAgent/claude-opus-5-5".into(),
                        },
                    });
                }
                items
            }
            ComposerTriggerKind::Skill => ["review", "release-notes", "frontend-design"]
                .iter()
                .filter(|s| s.contains(&q))
                .map(|s| ComposerCommandItem {
                    id: format!("skill:{s}").into(),
                    icon: ComposerCommandIcon::Icon(IconName::Sparkles),
                    label: (*s).into(),
                    label_prefix: None,
                    description: "Project skill".into(),
                    badge: Some("Project".into()),
                    action: ComposerCommandAction::Insert(format!("${s}").into()),
                })
                .collect(),
            ComposerTriggerKind::PullRequest => vec![ComposerCommandItem {
                id: "pr:41596".into(),
                icon: ComposerCommandIcon::Icon(IconName::GitPullRequest),
                label: "#41596".into(),
                label_prefix: None,
                description: "Markdown previews keep a long table in one piece".into(),
                badge: None,
                action: ComposerCommandAction::Insert("#41596".into()),
            }],
            ComposerTriggerKind::SlashCommand => Vec::new(),
        };
        (items, false)
    }
}

/// Composer state from fixtures.
pub fn fixture_state(key: &str) -> ComposerState {
    let catalog = Arc::new(fixture_catalog());
    ComposerState {
        draft_key: ComposerDraftKey(key.to_owned().into()),
        is_draft_thread: false,
        catalog,
        selection: ModelSelection {
            instance_id: ProviderInstanceId("claudeAgent".into()),
            model: "claude-opus-5-5".into(),
            effort: Some("high".into()),
            speed: None,
            ultracode: false,
        },
        runtime_mode: RuntimeMode::FullAccess,
        default_runtime_mode: RuntimeMode::FullAccess,
        running: false,
        can_interrupt: false,
        follow_up: FollowUpBehavior::Steer,
        send_shortcut: SendShortcut::Enter,
        rich_text: true,
        send_disabled_reason: None,
        project_required: false,
        is_send_busy: false,
        history: Arc::new(vec![PromptHistoryEntry { id: "m1".into(), prompt: "Run the composer scene again".into() }]),
        banners: Vec::new(),
        context_strip: Some(ComposerContextStrip {
            environment: "devbox".into(),
            workspace: "Worktree".into(),
            branch: "composer".into(),
        }),
        voice_available: true,
        validation: None,
        file_attachment_limit: Some(crate::state::attachments::MAX_FILE_BYTES),
        placeholder: None,
    }
}

/// The multi-line draft: chips, inline Markdown and a fenced code block.
pub fn fixture_draft() -> String {
    let file = serialize_file_link("apps/web/src/components/chat/composer.rs");
    let image = format_context_reference("image", "img_1", "screenshot.png");
    let pasted = format_context_reference("text", "txt_1", "Pasted text · 34 KB");
    format!(
        "Port the **composer** from {file} and keep the *typing feel* tight.\n\
         Use $review on the result, compare with {image} and {pasted} for context.\n\
         - Match `CornerDownLeft` for send\n\
         - Keep ~~pills~~ quiet icon buttons\n\
         ```rust\n\
         fn main() {{\n    let draft = \"hello\"; // send on Enter\n    println!(\"{{draft}}\");\n}}\n\
         ```\n\
         Then run the scene."
    )
}

/// One composer cell and how to set it up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneState {
    Empty,
    MultiLine,
    Running,
    Disabled,
    PickerOpen,
    MenuOpen,
}

impl SceneState {
    pub const ALL: [Self; 6] = [Self::Empty, Self::MultiLine, Self::Running, Self::Disabled, Self::PickerOpen, Self::MenuOpen];

    fn label(self) -> &'static str {
        match self {
            Self::Empty => "Empty",
            Self::MultiLine => "Multi-line, chips, code block",
            Self::Running => "Running: Stop",
            Self::Disabled => "Disabled with error banner",
            Self::PickerOpen => "Model picker open",
            Self::MenuOpen => "@ menu open",
        }
    }
}

/// Builds a composer in one state.
pub fn build_composer(state: SceneState, store: Rc<RefCell<dyn ComposerDraftStore>>, window: &mut Window, cx: &mut App) -> Entity<ChatComposer> {
    let mut composer_state = fixture_state(&format!("scene-{state:?}"));
    match state {
        SceneState::Running => {
            composer_state.running = true;
            composer_state.can_interrupt = true;
        }
        SceneState::Disabled => {
            composer_state.project_required = true;
            composer_state.send_disabled_reason = Some("Environment disconnected".into());
            composer_state.runtime_mode = RuntimeMode::ApprovalRequired;
            composer_state.banners = vec![ComposerBanner {
                id: "disconnected".into(),
                tone: BannerTone::Error,
                icon: IconName::CircleAlert,
                title: "Environment disconnected. Messages send when it reconnects.".into(),
                action: Some("Retry".into()),
            }];
        }
        _ => {}
    }
    let composer = cx.new(|cx| ChatComposer::new(composer_state, Rc::new(FixtureMenuSource), store, cx));
    composer.update(cx, |composer, cx| {
        let editor = composer.editor().clone();
        match state {
            SceneState::MultiLine => editor.update(cx, |e, cx| e.set_text(fixture_draft(), cx)),
            SceneState::Running => {}
            SceneState::MenuOpen => editor.update(cx, |e, cx| e.set_text("Look at @comp", cx)),
            SceneState::PickerOpen => {
                editor.update(cx, |e, cx| e.set_text("Which model should review this?", cx));
                composer.open_picker(window, cx);
            }
            _ => {}
        }
    });
    composer
}

/// The scene view: a grid of composer states.
pub struct ComposerScene {
    cells: Vec<(SceneState, Entity<ChatComposer>)>,
}

impl ComposerScene {
    pub fn new(states: &[SceneState], window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::components::chat::composer::init(cx);
        let store: Rc<RefCell<dyn ComposerDraftStore>> = Rc::new(RefCell::new(InMemoryDraftStore::default()));
        let cells: Vec<_> = states.iter().map(|s| (*s, build_composer(*s, store.clone(), window, cx))).collect();
        // The multi-line draft takes focus so its caret and active markers show,
        // unless the picker (which focuses its search) is open.
        if let Some((_, multi)) = cells.iter().find(|(s, _)| *s == SceneState::MultiLine) {
            let focus = multi.read(cx).editor().read(cx).focus_handle.clone();
            if cells.iter().all(|(s, c)| *s != SceneState::PickerOpen || c.read(cx).picker().is_none()) {
                window.focus(&focus, cx);
            }
        }
        Self { cells }
    }

    /// The composer for a state.
    pub fn composer(&self, state: SceneState) -> Option<&Entity<ChatComposer>> {
        self.cells.iter().find(|(s, _)| *s == state).map(|(_, c)| c)
    }
}

impl Render for ComposerScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        div()
            .size_full()
            .bg(colors.canvas)
            .text_color(colors.text)
            .flex()
            .flex_wrap()
            .children(self.cells.iter().map(|(state, composer)| {
                div()
                    .w(px(CELL_WIDTH))
                    .h(px(CELL_HEIGHT))
                    .p(px(24.))
                    .flex()
                    .flex_col()
                    .justify_between()
                    .child(div().text_size(px(12.)).text_color(colors.text_muted).child(state.label()))
                    .child(composer.clone())
            }))
    }
}

/// The resting states. The headless display is 1920x1080, so the popover
/// states are a second scene.
const RESTING: [SceneState; 4] = [SceneState::Empty, SceneState::MultiLine, SceneState::Running, SceneState::Disabled];
const POPOVERS: [SceneState; 2] = [SceneState::PickerOpen, SceneState::MenuOpen];

fn light(cx: &mut App) {
    crate::theme_palette::ThemeSettings::set_appearance(Appearance::Light, cx);
}

pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| ComposerScene::new(&RESTING, window, cx)).into()
}

pub fn build_light(window: &mut Window, cx: &mut App) -> AnyView {
    light(cx);
    cx.new(|cx| ComposerScene::new(&RESTING, window, cx)).into()
}

pub fn build_popovers(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| ComposerScene::new(&POPOVERS, window, cx)).into()
}

pub fn build_popovers_light(window: &mut Window, cx: &mut App) -> AnyView {
    light(cx);
    cx.new(|cx| ComposerScene::new(&POPOVERS, window, cx)).into()
}
