//! "Open in editor" (`OpenInPicker.tsx`, panel layout, and the favourite
//! editor shortcut of `OpenInPickerShortcut.ts`). The primary half opens the
//! preferred editor; the chevron picks another, which becomes preferred. A
//! local environment launches the editor through `shell.openInEditor`; a
//! remote one opens an SSH editor URL on this machine.

use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
    prelude::*, px,
};
use t3code_contracts::{EditorId, LaunchEditorInput};

use super::thread_details_panel_styles::{row_base, row_icon, split_row, split_secondary};
use crate::components::git_actions_control_source::GitActionsSource;
use crate::components::icons::{BrandIcon, Icon, IconName, IconSize, IconSource};
use crate::components::ui::popover::{below, glass_surface};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::tooltip;
use crate::components::ui::{DISABLED_OPACITY, TextStyleExt as _};
use crate::appearance_fonts::{TextSize, weight};
use crate::editor_preferences::{EDITORS, LastEditor, Platform, editor_label, effective_editor};
use crate::i18n::t;
use crate::remote_open::{RemoteOpenState, build_remote_open_url};
use crate::{ActiveAppTheme as _, AppTheme};

/// The editor's mark: a brand icon where one is bundled, else a folder or the
/// generic open glyph (`resolveOpenInOptions`).
pub fn editor_icon(editor: EditorId, platform: Platform) -> (IconSource, bool) {
    let brand = |b: BrandIcon| (IconSource::from(b), true);
    match editor {
        EditorId::Cursor => brand(BrandIcon::Cursor),
        EditorId::Trae => brand(BrandIcon::Trae),
        EditorId::Kiro => brand(BrandIcon::Kiro),
        EditorId::Vscode => brand(BrandIcon::VisualStudioCode),
        EditorId::VscodeInsiders => brand(BrandIcon::VisualStudioCodeInsiders),
        EditorId::Vscodium => brand(BrandIcon::VsCodium),
        EditorId::Zed => brand(BrandIcon::Zed),
        EditorId::Antigravity => brand(BrandIcon::Antigravity),
        EditorId::FileManager => match platform {
            Platform::Mac => brand(BrandIcon::Finder),
            Platform::Windows => brand(BrandIcon::FileExplorer),
            Platform::Linux => (IconName::FolderClosed.into(), false),
        },
        // JetBrains marks are not bundled.
        _ => (IconName::SquareArrowOutUpRight.into(), false),
    }
}

/// The open-in-editor row.
pub struct OpenInPicker {
    source: Arc<dyn GitActionsSource>,
    /// Editors the environment's server found (`availableEditors`).
    available: Vec<EditorId>,
    /// Remote-capable editors on this machine, for a remote environment.
    remote_capable: Vec<EditorId>,
    remote: RemoteOpenState,
    environment_label: Option<SharedString>,
    cwd: Option<String>,
    /// The session may launch processes on the host.
    can_operate_host: bool,
    platform: Platform,
    menu_open: bool,
}

impl OpenInPicker {
    pub fn new(
        source: Arc<dyn GitActionsSource>,
        available: Vec<EditorId>,
        remote: RemoteOpenState,
        cwd: Option<String>,
    ) -> Self {
        Self {
            source,
            available,
            remote_capable: crate::remote_open::REMOTE_FALLBACK_EDITORS.to_vec(),
            remote,
            environment_label: None,
            cwd,
            can_operate_host: true,
            platform: Platform::current(),
            menu_open: false,
        }
    }

    /// Names the environment in the remote hints.
    pub fn with_environment_label(mut self, label: impl Into<SharedString>) -> Self {
        self.environment_label = Some(label.into());
        self
    }

    /// The desktop's probe of remote-capable editors on this machine.
    pub fn set_remote_capable_editors(&mut self, editors: Vec<EditorId>, cx: &mut Context<Self>) {
        self.remote_capable = editors;
        cx.notify();
    }

    pub fn set_menu_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.menu_open = open;
        cx.notify();
    }

    /// Remote editors run on this machine, so they come from its probe.
    fn effective_editors(&self) -> &[EditorId] {
        if self.remote == RemoteOpenState::LocalExec { &self.available } else { &self.remote_capable }
    }

    fn preferred(&self, cx: &App) -> Option<EditorId> {
        effective_editor(LastEditor::get(cx), self.effective_editors())
    }

    fn can_open(&self) -> bool {
        match self.remote {
            RemoteOpenState::LocalExec => self.can_operate_host,
            RemoteOpenState::RemoteLinks(_) => true,
            RemoteOpenState::RemoteUnavailable => false,
        }
    }

    /// Opens the checkout in `editor`, or the preferred editor. This is also
    /// what `editor.openFavorite` (mod+o) runs.
    pub fn open(&mut self, editor: Option<EditorId>, cx: &mut Context<Self>) {
        let Some(cwd) = self.cwd.clone() else { return };
        let Some(editor) = editor.or_else(|| self.preferred(cx)) else { return };
        match &self.remote {
            RemoteOpenState::RemoteUnavailable => {}
            RemoteOpenState::RemoteLinks(host) => {
                if let Some(url) = build_remote_open_url(editor, &host.host, host.username.as_deref(), &cwd, false) {
                    cx.open_url(&url);
                    LastEditor::set(editor, cx);
                }
            }
            RemoteOpenState::LocalExec => {
                if !self.can_operate_host {
                    return;
                }
                LastEditor::set(editor, cx);
                let request =
                    self.source.open_in_editor(LaunchEditorInput { cwd, editor, position: None, project_root: None, reveal: None });
                cx.spawn(async move |_, cx| {
                    if let Err(error) = request.await {
                        cx.update(|cx| {
                            Toaster::push(ToastData::new(ToastKind::Error, t!("Unable to open file")).description(error.0), cx);
                        });
                    }
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn unavailable_reason(&self) -> Option<SharedString> {
        let env = self.environment_label.clone();
        match (&self.remote, env) {
            (RemoteOpenState::RemoteUnavailable, Some(env)) => Some(t!("No SSH route to {environment}", environment = env)),
            (RemoteOpenState::RemoteUnavailable, None) => Some(t!("No SSH route to this machine")),
            (RemoteOpenState::RemoteLinks(_), Some(env)) if self.remote_capable.is_empty() => {
                Some(t!("Open from a computer to edit on {environment}", environment = env))
            }
            (RemoteOpenState::RemoteLinks(_), None) if self.remote_capable.is_empty() => {
                Some(t!("Open from a computer to edit on this machine"))
            }
            _ => None,
        }
    }

    fn render_menu(&self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let highlight = c.highlight();
        let preferred = self.preferred(cx);
        let disabled_row = |text: SharedString| {
            div()
                .min_h(theme.sp(7.))
                .flex()
                .items_center()
                .px(theme.sp(2.))
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .opacity(DISABLED_OPACITY)
                .child(text)
                .into_any_element()
        };
        let mut rows = Vec::new();
        if let Some(reason) = self.unavailable_reason() {
            rows.push(disabled_row(reason));
        } else {
            let editors: Vec<EditorId> =
                EDITORS.iter().map(|(id, ..)| *id).filter(|id| self.effective_editors().contains(id)).collect();
            if editors.is_empty() {
                rows.push(disabled_row(t!("No installed editors found")));
            }
            let enabled = self.cwd.is_some() && self.can_open();
            for (index, editor) in editors.into_iter().enumerate() {
                let (icon, brand) = editor_icon(editor, self.platform);
                rows.push(
                    div()
                        .id(("editor-option", index))
                        .flex()
                        .items_center()
                        .gap(theme.sp(2.))
                        .min_h(theme.sp(7.))
                        .px(theme.sp(2.))
                        .rounded(theme.radii.sm)
                        .ui_text(theme, TextSize::Sm, weight::NORMAL)
                        .child(Icon::new(icon).size(IconSize::Md).color(if brand { c.foreground() } else { c.muted_foreground }))
                        .child(div().flex_1().min_w_0().truncate().child(editor_label(editor, self.platform)))
                        .when(Some(editor) == preferred, |r| {
                            r.child(
                                div()
                                    .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                                    .text_color(c.secondary_label)
                                    .child(if cfg!(target_os = "macos") { "⌘O" } else { "Ctrl+O" }),
                            )
                        })
                        .when(!enabled, |r| r.opacity(DISABLED_OPACITY))
                        .when(enabled, |r| {
                            r.cursor_pointer().hover(move |s| s.bg(highlight)).on_click(cx.listener(move |this, _, _, cx| {
                                // Picking from the panel's menu only changes the preferred editor.
                                LastEditor::set(editor, cx);
                                this.menu_open = false;
                                cx.notify();
                            }))
                        })
                        .into_any_element(),
                );
            }
            if matches!(self.remote, RemoteOpenState::RemoteLinks(_)) {
                rows.push(disabled_row(match &self.environment_label {
                    Some(env) => t!("Opens over SSH. Needs your key on {environment}", environment = env.clone()),
                    None => t!("Opens over SSH. Needs your key on this machine"),
                }));
            }
        }
        below(
            glass_surface(theme)
                .id("open-in-menu")
                .occlude()
                .w(theme.spacing.thread_details_panel_width - px(16.))
                .p(theme.sp(1.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_menu_open(false, cx)))
                .children(rows),
        )
        .into_any_element()
    }
}

impl Render for OpenInPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let preferred = self.preferred(cx);
        let label = match preferred {
            Some(editor) => t!("Open in {editor}", editor = editor_label(editor, self.platform)),
            None => t!("Open in editor"),
        };
        let icon = match preferred {
            Some(editor) => {
                let (icon, brand) = editor_icon(editor, self.platform);
                Icon::new(icon).size(IconSize::Md).color(if brand { theme.colors.foreground() } else { theme.colors.muted_foreground })
            }
            None => row_icon(&theme, IconName::SquareArrowOutUpRight),
        };
        let disabled = preferred.is_none() || self.cwd.is_none() || !self.can_open();
        let host_denied = self.remote == RemoteOpenState::LocalExec && !self.can_operate_host;
        let primary = row_base("open-in-primary", &theme, disabled, false)
            .child(icon)
            .child(div().flex_1().min_w_0().truncate().child(label.clone()))
            .tooltip(tooltip(label))
            .when(!disabled, |r| r.on_click(cx.listener(|this, _, _, cx| this.open(None, cx))));
        let chevron = split_secondary("open-in-menu-trigger", &theme, host_denied, self.menu_open)
            .tooltip(tooltip(t!("Choose editor")))
            .when(!host_denied, |r| {
                r.on_click(cx.listener(|this, _, _, cx| {
                    let open = !this.menu_open;
                    this.set_menu_open(open, cx);
                }))
            });
        let menu = self.menu_open.then(|| self.render_menu(&theme, cx));
        div().flex().flex_col().w_full().child(split_row("open-in-split", &theme, primary, chevron, self.menu_open, false)).children(menu)
    }
}
