//! The Usage modal over the app shell, with fixture data: limits, cost,
//! tokens, the past 24 hours, the model dialog, loading, in dark and light.

use std::rc::Rc;

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use t3code_contracts::UsageProviderKind;

use crate::AppRoot;
use crate::AppTheme;
use crate::components::ui::dialog::Modal;
use crate::components::usage::usage_shortcuts::{UsageMetric, UsageWindowDays};
use crate::components::usage::usage_source::{FixtureUsageSource, UsageSource};
use crate::routes::usage::UsageRoute;
use crate::theme_palette::{Appearance, ThemeDocument, set_app_theme};

struct UsageScene {
    app: Entity<AppRoot>,
    route: Entity<UsageRoute>,
}

impl Render for UsageScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.app.clone()).child(Modal::dialog("usage-route", self.route.clone(), |_, _| {}))
    }
}

/// What the scene opens.
#[derive(Clone, Copy)]
enum Variant {
    Limits,
    Cost,
    Tokens,
    Day,
    Model,
    TimeBreakdown,
    Loading,
    /// Limits, with no provider reporting any (the reference capture).
    Empty,
    /// "Custom model prices" from the environment menu.
    Prices,
    /// "Custom model prices" from a model's "Set price", with a new row.
    PricesNew,
}

fn build_variant(variant: Variant, light: bool, window: &mut Window, cx: &mut App) -> AnyView {
    if light {
        set_app_theme(AppTheme::new(&ThemeDocument::graphite(), Appearance::Light), cx);
    }
    let source: Rc<dyn UsageSource> = match variant {
        Variant::Loading => FixtureUsageSource::loading(),
        Variant::Empty => FixtureUsageSource::without_limits(),
        _ => FixtureUsageSource::new(),
    };
    let app = cx.new(|cx| AppRoot::new(window, cx));
    let route = cx.new(|cx| UsageRoute::new(source, None, window, cx));
    let page = route.read(cx).page().clone();
    page.update(cx, |page, cx| {
        let (metric, days) = match variant {
            Variant::Limits | Variant::Empty => (UsageMetric::Limits, UsageWindowDays::Month),
            Variant::Cost | Variant::Model | Variant::Loading | Variant::Prices | Variant::PricesNew => (UsageMetric::Cost, UsageWindowDays::Month),
            Variant::Tokens => (UsageMetric::Tokens, UsageWindowDays::Week),
            Variant::Day | Variant::TimeBreakdown => (UsageMetric::Cost, UsageWindowDays::Day),
        };
        page.show(metric, days, cx);
        if matches!(variant, Variant::Model) {
            page.show_model(UsageProviderKind::Claude, "claude-opus-4-5", cx);
        }
        match variant {
            Variant::Prices => page.show_prices(None, cx),
            Variant::PricesNew => page.show_prices(Some("grok-code-fast-1".into()), cx),
            _ => {}
        }
        if matches!(variant, Variant::TimeBreakdown) {
            page.show_breakdown_by_time(true, cx);
        }
    });
    cx.new(|_| UsageScene { app, route }).into()
}

pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Limits, false, window, cx)
}
pub fn build_light(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Limits, true, window, cx)
}
pub fn build_cost(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Cost, false, window, cx)
}
pub fn build_cost_light(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Cost, true, window, cx)
}
pub fn build_tokens(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Tokens, false, window, cx)
}
pub fn build_day(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Day, false, window, cx)
}
pub fn build_time_breakdown(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::TimeBreakdown, false, window, cx)
}
pub fn build_model(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Model, false, window, cx)
}
pub fn build_prices(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Prices, false, window, cx)
}
pub fn build_prices_new(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::PricesNew, false, window, cx)
}
pub fn build_loading(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Loading, false, window, cx)
}
pub fn build_empty(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(Variant::Empty, false, window, cx)
}
