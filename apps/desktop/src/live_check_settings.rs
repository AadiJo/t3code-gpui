// live-check `settings`: the Threads settings page against a real server. The Fallback model
// trigger opens the model picker and Enter picks its highlighted model, which the server then
// reports as `fallbackModelSelection`; the reset arrow clears it again. The first snooze option's
// "+" steps its amount, which the server reports in `snoozePresets`; "−" steps it back. Providers
// names its one environment ("on <label>"). Usage's "Model prices" adds a custom price, saves it
// and resets it again. Every value is read back from the server's config.

use super::*;
use t3code_contracts::{SnoozePresetSetting, SnoozePresetSettingAfter};
use t3code_web::hooks::use_settings::ServerSettingsStore;
use t3code_web::router::SettingsPanel;

/// Clicks the centre of the visible element `id`, scrolling the settings page down with the
/// wheel until it is laid out in view.
fn click_id(check: &mut Check, id: &str) -> anyhow::Result<()> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    let mut found = None;
    for _ in 0..12 {
        let size = check.cx.update_window(check.window, |_, w, _| w.viewport_size()).unwrap_or_default();
        found = check
            .cx
            .update_window(check.window, |_, w, _| w.try_find(element.clone()).filter(|e| e.visible()).map(|e| e.bounds().center()))
            .ok()
            .flatten()
            .filter(|at: &Point<Pixels>| at.y > px(80.) && at.y < size.height - px(40.));
        if found.is_some() {
            break;
        }
        let wheel_at = point(size.width * 0.6, size.height * 0.5);
        check.cx.update_window(check.window, |_, w, cx| {
            w.dispatch_event(
                ScrollEvent { position: wheel_at, delta: ScrollDelta::Pixels(point(px(0.), px(-240.))), modifiers: Modifiers::default(), touch_phase: TouchPhase::Moved, source: ScrollSource::Pointer }
                    .to_platform_input(),
                cx,
            );
        })?;
        check.frame();
    }
    let at = found.with_context(|| format!("no visible element {id}"))?;
    check.cx.update_window(check.window, |_, w, cx| {
        w.dispatch_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
        w.dispatch_event(
            MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }.to_platform_input(),
            cx,
        );
        w.dispatch_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
    })?;
    check.frame();
    Ok(())
}

/// Clicks the centre of the visible element `id` where it is, without scrolling.
fn click_visible(check: &mut Check, id: &str) -> anyhow::Result<()> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    let at = check
        .cx
        .update_window(check.window, |_, w, _| w.try_find(element.clone()).filter(|e| e.visible()).map(|e| e.bounds().center()))
        .ok()
        .flatten()
        .with_context(|| format!("no visible element {id}"))?;
    check.cx.update_window(check.window, |_, w, cx| {
        w.dispatch_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
        w.dispatch_event(
            MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }.to_platform_input(),
            cx,
        );
        w.dispatch_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
    })?;
    check.frame();
    Ok(())
}

/// The custom input and output price the server holds for `model`.
fn custom_price(check: &mut Check, id: &t3code_contracts::EnvironmentId, model: &str) -> Option<(f64, f64)> {
    let price = server_settings(check, id)?.usage_price_overrides?.get(model)?.clone();
    Some((price.input_cost_per_million_tokens, price.output_cost_per_million_tokens))
}

/// The environment's server settings as its config last reported them.
fn server_settings(check: &mut Check, id: &t3code_contracts::EnvironmentId) -> Option<t3code_contracts::ServerSettings> {
    check.cx.update(|cx| ServerSettingsStore::config(cx, id).map(|config| config.settings.clone()))
}

fn first_snooze_minutes(settings: &t3code_contracts::ServerSettings) -> Option<u64> {
    match settings.snooze_presets.as_ref()?.first()? {
        SnoozePresetSetting::After(SnoozePresetSettingAfter { minutes }) => Some(*minutes),
        _ => None,
    }
}

pub(super) fn run_settings(check: &mut Check) -> anyhow::Result<()> {
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let id = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.to_string());
    let environment_id = t3code_contracts::EnvironmentId::new(id.as_str());
    let env = environment_id.clone();
    check.pump(move |c| server_settings(c, &env).is_some(), Duration::from_secs(30), "the server config arrived")?;
    let router = check.read(|root, _| root.router().clone());
    let route = Route::Settings { panel: SettingsPanel::Threads, query: SharedString::default() };
    check.cx.update(|cx| router.update(cx, |router, cx| router.navigate(route, cx)));
    check.frame();
    check.wait(Duration::from_millis(500));
    check.shot("s0-threads.png")?;

    // Fallback model: the trigger opens the picker; Enter picks the highlighted model.
    let before = server_settings(check, &environment_id).and_then(|s| s.fallback_model_selection);
    check.note(format!("fallback before: {before:?}"));
    click_id(check, "model-trigger-fallbackModelSelection")?;
    check.wait(Duration::from_millis(300));
    check.shot("s1-fallback-picker.png")?;
    check.press("enter");
    let env = environment_id.clone();
    check.pump(
        move |c| server_settings(c, &env).is_some_and(|s| s.fallback_model_selection.is_some()),
        Duration::from_secs(15),
        "the picked fallback model reaches the server",
    )?;
    let picked = server_settings(check, &environment_id).and_then(|s| s.fallback_model_selection);
    check.check("the fallback model is saved", picked.is_some(), format!("{picked:?}"));
    check.shot("s2-fallback-picked.png")?;
    click_id(check, "fallbackModelSelection-reset")?;
    let env = environment_id.clone();
    check.pump(
        move |c| server_settings(c, &env).is_some_and(|s| s.fallback_model_selection.is_none()),
        Duration::from_secs(15),
        "reset clears the fallback model on the server",
    )?;
    check.check("reset clears the fallback model", true, "");

    // Snooze: "+" on the first option's number field steps its amount by one unit.
    let minutes = server_settings(check, &environment_id).as_ref().and_then(first_snooze_minutes).unwrap_or(60);
    click_id(check, "snooze-amount-0-inc")?;
    let env = environment_id.clone();
    check.pump(
        move |c| server_settings(c, &env).as_ref().and_then(first_snooze_minutes).is_some_and(|m| m != minutes),
        Duration::from_secs(15),
        "the stepped snooze amount reaches the server",
    )?;
    let stepped = server_settings(check, &environment_id).as_ref().and_then(first_snooze_minutes);
    check.check("+ adds one unit to the first snooze option", stepped == Some(minutes + 60) || stepped == Some(minutes + 1), format!("{minutes} -> {stepped:?}"));
    check.shot("s3-snooze-stepped.png")?;
    click_id(check, "snooze-amount-0-dec")?;
    let env = environment_id.clone();
    check.pump(
        move |c| server_settings(c, &env).as_ref().and_then(first_snooze_minutes) == Some(minutes),
        Duration::from_secs(15),
        "− steps the snooze amount back",
    )?;
    check.check("− restores the first snooze option", true, format!("{minutes}"));

    // Providers edits one environment and names it.
    let route = Route::Settings { panel: SettingsPanel::Providers, query: SharedString::default() };
    check.cx.update(|cx| router.update(cx, |router, cx| router.navigate(route, cx)));
    check.frame();
    check.wait(Duration::from_millis(500));
    let named = check.cx.update(|cx| cx.try_global::<t3code_web::components::settings::settings_controls::PagesEnvironment>().and_then(|e| e.0.clone()));
    check.check("Providers names the environment its page shows", named.as_ref() == Some(&environment_id), format!("{named:?}"));
    check.shot("s4-providers.png")?;

    // Usage > environments menu > Model prices: a new row with input and output prices saves to
    // the server's `usagePriceOverrides`; its reset arrow and Save remove it again.
    const MODEL: &str = "live-check-model";
    check.cx.update(|cx| router.update(cx, |router, cx| router.navigate(Route::Usage, cx)));
    check.frame();
    check.wait(Duration::from_millis(800));
    click_visible(check, "usage-environments-trigger")?;
    check.wait(Duration::from_millis(300));
    check.shot("s5-usage-menu.png")?;
    click_visible(check, "usage-model-prices")?;
    check.wait(Duration::from_millis(300));
    click_visible(check, "usage-price-add")?;
    check.type_text(MODEL);
    click_visible(check, "usage-price-new:0-input")?;
    check.type_text("0.5");
    click_visible(check, "usage-price-new:0-output")?;
    check.type_text("2");
    check.type_text("x");
    check.frame();
    check.shot("s6-prices-typed.png")?;
    click_visible(check, "usage-prices-save")?;
    let env = environment_id.clone();
    check.pump(move |c| custom_price(c, &env, MODEL).is_some(), Duration::from_secs(15), "the new model price reaches the server")?;
    let saved = custom_price(check, &environment_id, MODEL);
    // "x" is not a price edit, so the output cell keeps "2".
    check.check("Save writes input 0.5 and output 2", saved == Some((0.5, 2.0)), format!("{saved:?}"));
    check.wait(Duration::from_millis(300));
    check.shot("s7-prices-saved.png")?;
    click_visible(check, &format!("usage-price-reset-model:{MODEL}"))?;
    check.shot("s8-prices-reset.png")?;
    click_visible(check, "usage-prices-save")?;
    let env = environment_id.clone();
    check.pump(move |c| custom_price(c, &env, MODEL).is_none(), Duration::from_secs(15), "reset removes the model price on the server")?;
    check.check("reset and Save remove the custom price", true, "");
    check.shot("s9-prices-removed.png")?;
    Ok(())
}
