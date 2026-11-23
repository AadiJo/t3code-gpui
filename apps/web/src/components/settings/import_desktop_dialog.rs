//! Import from the Electron desktop app. Says what it will read, reads and
//! decrypts it only when asked, shows what carries over and what needs
//! pairing again, then saves. The T3 Connect sign-in is a separate, opt-in
//! choice because both apps would then share one rotating Clerk session.
//!
//! The web app has no such screen, so this copy is not in the translation
//! catalogs.

use gpui_kit::{Context, EventEmitter, IntoElement, Render, SharedString, Window, div, prelude::*, px};
use t3code_client_runtime::platform::electron_import::SkipReason;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::checkbox::{CheckState, Checkbox};
use crate::components::ui::dialog::DialogSurface;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::connection::{ConnectionRuntime, ImportPreview};

#[derive(Clone, Debug)]
pub enum ImportDesktopEvent {
    Close,
}

enum Step {
    Explain,
    Reading,
    Preview(ImportPreview),
    Saving,
    Failed(SharedString),
}

/// The import dialog.
pub struct ImportDesktopDialog {
    step: Step,
    with_clerk_session: bool,
}

impl EventEmitter<ImportDesktopEvent> for ImportDesktopDialog {}

impl ImportDesktopDialog {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { step: Step::Explain, with_clerk_session: false }
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let Some(runtime) = cx.try_global::<ConnectionRuntime>() else { return };
        let task = runtime.preview_electron_import(cx);
        self.step = Step::Reading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.step = match result {
                    Ok(preview) => Step::Preview(preview),
                    Err(error) => Step::Failed(error.0),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Step::Preview(preview) = std::mem::replace(&mut self.step, Step::Saving) else { return };
        let Some(runtime) = cx.try_global::<ConnectionRuntime>() else { return };
        let task = runtime.apply_electron_import(preview, self.with_clerk_session, cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(added) => {
                    Toaster::push(
                        ToastData::new(ToastKind::Success, "Imported from the desktop app")
                            .description(format!("Added {added} route{}.", if added == 1 { "" } else { "s" })),
                        cx,
                    );
                    cx.emit(ImportDesktopEvent::Close);
                }
                Err(error) => {
                    this.step = Step::Failed(error.0);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

fn skip_reason(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::DeviceBoundCredential => "paired to the desktop app's own key; pair it again",
        SkipReason::MissingCredential => "no saved sign-in; pair it again",
        SkipReason::Ssh => "SSH routes are not supported in this app",
    }
}

impl Render for ImportDesktopDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let text = |s: SharedString| div().ui_text(theme, TextSize::Sm, weight::NORMAL).child(s);
        let muted = |s: SharedString| div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(s);
        let close = Button::new("import-cancel")
            .label("Cancel")
            .variant(ButtonVariant::Outline)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(ImportDesktopEvent::Close)));
        let surface = DialogSurface::new("Import from the desktop app")
            .description("Copies saved environments from the T3 Code desktop app on this computer. Its files are read once and never changed.");
        let surface = match &self.step {
            Step::Explain | Step::Reading => surface
                .child(text("This reads:".into()))
                .child(muted("• connection-catalog.json in the desktop app's data folder: environments, routes, and pairing tokens.".into()))
                .child(muted("• The desktop app's encryption key, from Windows data protection or the macOS Keychain (macOS asks you first).".into()))
                .child(muted("Routes paired to the desktop app's own key cannot move and are listed for pairing again.".into()))
                .footer(close)
                .footer(
                    Button::new("import-read")
                        .label(if matches!(self.step, Step::Reading) { "Reading…" } else { "Read desktop app data" })
                        .disabled(matches!(self.step, Step::Reading))
                        .on_click(cx.listener(|this, _, _, cx| this.read(cx))),
                ),
            Step::Preview(preview) => {
                let routes: usize = preview.import.environments.iter().map(|env| env.routes.len()).sum();
                let mut surface = surface.child(text(
                    format!(
                        "{} environment{} with {routes} route{} can be imported.",
                        preview.import.environments.len(),
                        if preview.import.environments.len() == 1 { "" } else { "s" },
                        if routes == 1 { "" } else { "s" }
                    )
                    .into(),
                ));
                for env in &preview.import.environments {
                    surface = surface.child(muted(format!("• {}", env.label).into()));
                }
                if !preview.import.skipped.is_empty() {
                    surface = surface.child(text("Needs pairing again:".into()));
                    for skipped in &preview.import.skipped {
                        surface = surface.child(muted(
                            format!("• {} ({}): {}", skipped.environment_label, skipped.address, skip_reason(skipped.reason)).into(),
                        ));
                    }
                }
                if preview.has_clerk_session {
                    let checked = self.with_clerk_session;
                    surface = surface.child(
                        div()
                            .id("import-clerk-choice")
                            .flex()
                            .items_start()
                            .gap(sp(2.))
                            .mt(sp(1.))
                            .child(
                                Checkbox::new("import-clerk", if checked { CheckState::Checked } else { CheckState::Unchecked })
                                    .on_change({
                                        let this = cx.entity().downgrade();
                                        move |value, _, cx| {
                                            this.update(cx, |this, cx| {
                                                this.with_clerk_session = value;
                                                cx.notify();
                                            })
                                            .ok();
                                        }
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(text("Also use the desktop app's T3 Connect sign-in".into()))
                                    .child(muted("Both apps then share one session. Clerk rotates it on every use, so this can sign the desktop app out. Signing in here separately is safer.".into())),
                            ),
                    );
                }
                surface.footer(close).footer(
                    Button::new("import-save")
                        .label("Import")
                        .disabled(preview.import.environments.is_empty() && !self.with_clerk_session)
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
            }
            Step::Saving => surface.child(text("Importing…".into())),
            Step::Failed(error) => surface.child(div().text_color(c.destructive()).child(error.clone())).footer(close),
        };
        div().id("import-desktop-dialog").w_full().flex().justify_center().child(surface)
    }
}
