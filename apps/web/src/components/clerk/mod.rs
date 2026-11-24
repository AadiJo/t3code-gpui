//! T3 Connect sign-in. Ports the entry points of `T3ConnectSidebarSignIn.tsx`
//! and replaces Clerk's hosted sign-in UI, which cannot run natively, with
//! an email-code form driven through Clerk's Frontend API (see
//! `client-runtime/src/relay/clerk.rs` for why not a browser redirect).
//!
//! Clerk's own form copy is not in T3 Code's catalogs, so the form text here
//! is plain English; the entry points use the web strings.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, div,
    prelude::*,
};
use t3code_client_runtime::relay::{CloudAccountState, EmailCodeSignIn};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::settings::connections_settings::ConnectionsSettings;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::dialog::DialogSurface;
use crate::components::ui::input::TextInput;
use crate::connection::ConnectionRuntime;
use crate::i18n::t;

#[derive(Clone, Debug)]
pub enum T3ConnectSignInEvent {
    Close,
    SignedIn,
}

enum Step {
    Email,
    Code(EmailCodeSignIn),
}

/// The email-code sign-in form.
pub struct T3ConnectSignIn {
    step: Step,
    email: Entity<InputState>,
    code: Entity<InputState>,
    busy: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<T3ConnectSignInEvent> for T3ConnectSignIn {}

impl T3ConnectSignIn {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("123456"));
        let on_email = cx.subscribe(&email, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(cx);
            }
        });
        let on_code = cx.subscribe(&code, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(cx);
            }
        });
        email.update(cx, |state, cx| state.focus(window, cx));
        Self { step: Step::Email, email, code, busy: false, error: None, _subscriptions: vec![on_email, on_code] }
    }

    /// The email field, for scripted checks.
    pub fn email(&self) -> &Entity<InputState> {
        &self.email
    }

    /// The code field, for scripted checks.
    pub fn code(&self) -> &Entity<InputState> {
        &self.code
    }

    /// Shows the code step as if Clerk had sent a code (scenes).
    pub fn show_code_step(&mut self, email: &str, cx: &mut Context<Self>) {
        self.step = Step::Code(EmailCodeSignIn { sign_in_id: String::new(), email: email.to_owned() });
        cx.notify();
    }

    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if !ConnectionRuntime::is_running(cx) {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        match &self.step {
            Step::Email => {
                let email = self.email.read(cx).value().to_string();
                let task = ConnectionRuntime::global(cx).start_email_code(email, cx);
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, cx| {
                        this.busy = false;
                        match result {
                            Ok(sign_in) => this.step = Step::Code(sign_in),
                            Err(error) => this.error = Some(error.0),
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            Step::Code(sign_in) => {
                let code = self.code.read(cx).value().to_string();
                let task = ConnectionRuntime::global(cx).verify_email_code(sign_in.clone(), code, cx);
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, cx| {
                        this.busy = false;
                        match result {
                            Ok(()) => cx.emit(T3ConnectSignInEvent::SignedIn),
                            Err(error) => this.error = Some(error.0),
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
    }
}

impl Render for T3ConnectSignIn {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let (description, field, submit): (SharedString, AnyElement, &str) = match &self.step {
            Step::Email => (
                "Enter the email address of your T3 Connect account. You will get a code to sign in with.".into(),
                div().id("t3-connect-email").child(TextInput::new(&self.email).disabled(self.busy)).into_any_element(),
                if self.busy { "Sending…" } else { "Continue" },
            ),
            Step::Code(sign_in) => (
                format!("Enter the code sent to {}.", sign_in.email).into(),
                div().id("t3-connect-code").child(TextInput::new(&self.code).mono().disabled(self.busy)).into_any_element(),
                if self.busy { "Checking…" } else { "Sign in" },
            ),
        };
        let surface = DialogSurface::new(t!("Sign in to T3 Connect"))
            .description(description)
            .child(field)
            .children(
                self.error
                    .clone()
                    .map(|error| div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive()).child(error)),
            )
            .footer(
                Button::new("t3-connect-cancel")
                    .label(t!("Cancel"))
                    .variant(ButtonVariant::Outline)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(T3ConnectSignInEvent::Close))),
            )
            .footer(
                Button::new("t3-connect-submit")
                    .label(submit)
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
            );
        div().id("t3-connect-sign-in").w_full().flex().justify_center().child(surface)
    }
}

/// The settings nav footer: "Sign in to T3 Connect" while signed out
/// (`ConfiguredT3ConnectSidebarSignIn`), the account avatar while signed in
/// (`ConfiguredT3ConnectSidebarAvatar`), nothing while Clerk loads.
pub fn t3_connect_sidebar_sign_in(environments: Entity<ConnectionsSettings>, cx: &App) -> Option<AnyElement> {
    let state = environments.read(cx).state().clone();
    let account = state.read(cx).cloud.as_ref()?.account.clone();
    let signed_out = match account {
        CloudAccountState::SignedOut => true,
        CloudAccountState::SignedIn(account) => return Some(t3_connect_sidebar_avatar(account, cx)),
        CloudAccountState::Loading => false,
    };
    if !signed_out {
        return None;
    }
    let theme = cx.app_theme();
    let c = &theme.colors;
    Some(
        div()
            .p(theme.sp(2.))
            .child(
                div()
                    .id("t3-connect-sidebar-sign-in")
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .h(theme.sp(8.))
                    .px(theme.sp(2.5))
                    .rounded(theme.radii.control)
                    .cursor_pointer()
                    .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                    .text_color(theme.derived.sidebar_menu_button_text)
                    .hover(|s| s.bg(c.sidebar_row_hover).text_color(c.sidebar_foreground))
                    .on_click(move |_, window, cx| {
                        environments.update(cx, |page, cx| page.open_sign_in(window, cx));
                    })
                    .child(Icon::new(IconName::LogIn).size(IconSize::Md).color(theme.derived.sidebar_icon))
                    .child(t!("Sign in to T3 Connect")),
            )
            .into_any_element(),
    )
}

/// `UserButton` (`avatarBox: size-7`, trigger `mb-1 rounded-lg p-1`): the
/// account's initial in a circle; its menu names the account and signs out.
/// Clerk's "Manage account" profile pages are web-only and not offered.
fn t3_connect_sidebar_avatar(account: t3code_client_runtime::relay::CloudAccount, cx: &App) -> AnyElement {
    use crate::components::ui::menu::{MenuItem, MenuPopup, dropdown_menu};
    let theme = cx.app_theme();
    let c = &theme.colors;
    let label = account.name.clone().or(account.email.clone()).unwrap_or_default();
    let initial: SharedString = label.chars().next().map(|ch| ch.to_uppercase().collect::<String>()).unwrap_or_default().into();
    let trigger = div()
        .id("t3-connect-avatar")
        .mb(theme.sp(1.))
        .p(theme.sp(1.))
        .rounded(theme.radii.lg)
        .cursor_pointer()
        .hover(|s| s.bg(c.sidebar_row_hover))
        .child(
            div()
                .size(theme.sp(7.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(c.muted)
                .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                .text_color(c.foreground())
                .child(initial),
        );
    let (name, email) = (account.name.clone(), account.email.clone());
    div()
        .p(theme.sp(2.))
        .child(dropdown_menu("t3-connect-account", trigger, move |_, _| {
            let mut menu = MenuPopup::new();
            if let Some(name) = &name {
                menu = menu.item(MenuItem::new(name.clone()).disabled(true));
            }
            if let Some(email) = &email {
                menu = menu.item(MenuItem::new(email.clone()).disabled(true));
            }
            menu.separator().item(MenuItem::new(t!("Sign out")).on_select(|_, cx| {
                let Some(runtime) = cx.try_global::<ConnectionRuntime>() else { return };
                let task = runtime.sign_out(cx);
                cx.spawn(async move |cx| {
                    if let Err(error) = task.await {
                        cx.update(|cx| {
                            crate::components::ui::toast::Toaster::push(
                                crate::components::ui::toast::ToastData::new(crate::components::ui::toast::ToastKind::Error, SharedString::from(error.0.to_string())),
                                cx,
                            )
                        });
                    }
                })
                .detach();
            }))
        }))
        .into_any_element()
}
