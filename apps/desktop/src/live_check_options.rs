//! `live-check options`: the model picker's options beside reasoning, on the server's own
//! provider data. Claude's bundled catalog gives Sonnet 4.6 a context window select and Claude
//! Opus a Fast mode switch, and Haiku 4.5 a two-state thinking control. Through real clicks and
//! keys: the picker search finds the model, the effort column lists its levels with a "Context
//! window" row under them, the row opens its side menu by click and by ArrowRight, a choice there
//! is written, ArrowLeft and Escape close the side menu before the picker, the footer's Standard /
//! Fast segments flip Fast mode, the trigger names the context window, and a sent turn carries
//! every chosen option (read back from the server's thread). In Settings → Threads the default
//! model's traits chip names its values, and a pick in its menu reaches the server's settings.
//! Run `seed` first.

use std::time::Duration;

use t3code_web::hooks::use_settings::ServerSettingsStore;
use t3code_web::model_selection::{ModelSelection, OptionSelection, OptionValue};
use t3code_web::router::SettingsPanel;

use super::*;

fn element_center(check: &mut Check, id: &str) -> Option<Point<Pixels>> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    check.cx.update_window(check.window, |_, w, _| w.try_find(element).filter(|e| e.visible()).map(|e| e.bounds().center())).ok().flatten()
}

fn click(check: &mut Check, id: &str) -> anyhow::Result<()> {
    let at = element_center(check, id).ok_or_else(|| anyhow::anyhow!("no visible element {id}"))?;
    click_at(check, at)
}

fn selection(check: &mut Check) -> ModelSelection {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).state().selection.clone())
}

fn option(selection: &ModelSelection, id: &str) -> Option<OptionValue> {
    selection.options.iter().find(|o| o.id.as_ref() == id).map(|o| o.value.clone())
}

fn picker_menu(check: &mut Check) -> Option<Option<String>> {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).picker().map(|p| p.read(cx).open_option_menu_id().map(|id| id.to_string())))
}

/// The column row ids in order, up to the first that is missing.
fn column_rows(check: &mut Check) -> usize {
    (0..32).take_while(|i| element_center(check, &format!("effort-{i}")).is_some()).count()
}

/// Opens the picker with a click on the trigger and finds `query` through its search.
fn pick_by_search(check: &mut Check, query: &str) -> anyhow::Result<()> {
    if picker_menu(check).is_none() {
        click(check, "composer-model-trigger")?;
    }
    check.type_text(query);
    check.frame();
    click(check, "picker-row-0")?;
    check.frame();
    Ok(())
}

fn text(value: &str) -> Option<OptionValue> {
    Some(OptionValue::Text(value.into()))
}

pub(super) fn run_options(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
    });
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    // After the profile's client settings load, so the owner's overrides win.
    check.cx.update(t3code_web::theme_palette::ThemeSettings::apply_env_overrides);
    let rem = check.cx.update(|cx| t3code_web::components::chat::composer_theme::ComposerColors::new(cx).r(px(16.)));
    check.note(format!("composer rem {rem:?}"));
    let key = thread_with_turn(check, &state, "Model options")?;
    check.go(&key);
    check.pump(|c| c.shown_input().entries.len() >= 2, Duration::from_secs(20), "thread shown")?;
    let ready = {
        let composer = composer_of(check);
        check.cx.update(|cx| {
            let catalog = composer.read(cx).state().catalog.clone();
            catalog.instances.iter().map(|i| format!("{}:{}", i.instance_id.0, i.ready)).collect::<Vec<_>>().join(", ")
        })
    };
    check.note(format!("instances: {ready}"));
    check.focus_composer();

    // Sonnet 4.6: levels, then the context window as one row with a side menu.
    pick_by_search(check, "Sonnet 4.6")?;
    let picked = selection(check);
    check.check("the search and a click pick Claude Sonnet 4.6", picked.model.as_ref() == "claude-sonnet-4-6", &picked.model);
    let rows = column_rows(check);
    let levels = {
        let composer = composer_of(check);
        check.cx.update(|cx| {
            let state = composer.read(cx).state();
            state.selection.traits(&state.catalog).and_then(|t| t.effort().map(|e| e.selectable().len())).unwrap_or(0)
        })
    };
    check.check("the column lists the levels and one Context window row", levels >= 2 && rows == levels + 1, format!("{levels} levels, {rows} rows"));
    check.shot("o0-column.png")?;
    let menu_row = format!("effort-{levels}");
    click(check, &menu_row)?;
    let open = picker_menu(check).flatten();
    check.check("a click on the Context window row opens its side menu", open.as_deref() == Some("contextWindow"), format!("{open:?}"));
    let items = (0..8).take_while(|i| element_center(check, &format!("option-menu-item-{i}")).is_some()).count();
    check.check("the side menu offers both context windows", items == 2, items);
    let (row, item) = (element_center(check, &menu_row), element_center(check, "option-menu-item-0"));
    // Right of the column, or left of it when the viewport leaves no room (Base UI's flip).
    let room = check.cx.update_window(check.window, |_, w, _| w.viewport_size().width).unwrap_or_default();
    let side = row.zip(item).map(|(r, i)| if i.x > r.x { "right" } else { "left" });
    check.check("the side menu opens beside the column, flipping left when the window is narrow", side.is_some(), format!("{side:?} in a {room:?} window: {row:?} {item:?}"));
    check.shot("o1-side-menu.png")?;
    let ids = ["model-picker", "model-picker-option-menu", "option-menu-item-0", "option-menu-item-1", "effort-0", "effort-3", menu_row.as_str()];
    for id in ids {
        let element = ElementId::from(SharedString::from(id.to_owned()));
        let bounds = check.cx.update_window(check.window, |_, w, _| w.try_find(element).map(|e| e.bounds())).ok().flatten();
        check.note(format!("bounds {id}: {bounds:?}"));
    }
    click(check, "option-menu-item-1")?;
    let after = selection(check);
    check.check("picking 1M writes contextWindow=1m", option(&after, "contextWindow") == text("1m"), format!("{:?}", after.options));
    let ok = picker_menu(check) == Some(None);
    check.check("the pick closes the side menu and keeps the picker open", ok, "");
    check.check(
        "the write carries every option the model has (buildProviderOptionSelectionsFromDescriptors)",
        option(&after, "effort").is_some(),
        format!("{:?}", after.options),
    );

    // The keyboard: Down to the row, ArrowRight opens, Up and Enter pick, ArrowLeft closes.
    check.press("escape");
    check.focus_composer();
    check.press("mod-shift-e");
    for _ in 0..rows {
        check.press("down");
    }
    check.press("right");
    let open = picker_menu(check).flatten();
    check.check("ArrowRight on the option row opens its side menu", open.as_deref() == Some("contextWindow"), format!("{open:?}"));
    check.press("left");
    let ok = picker_menu(check) == Some(None);
    check.check("ArrowLeft closes the side menu, not the picker", ok, "");
    check.press("right");
    check.press("up");
    check.press("enter");
    let after = selection(check);
    check.check("Up then Enter in the side menu picks 200K", option(&after, "contextWindow") == text("200k"), format!("{:?}", after.options));
    check.press("right");
    check.press("escape");
    let ok = picker_menu(check) == Some(None);
    check.check("Escape closes the side menu first", ok, "");
    check.press("escape");
    let ok = picker_menu(check).is_none();
    check.check("a second Escape closes the picker", ok, "");

    // Opus 4.6: Fast mode as footer segments, and the context window back at 1M by click.
    pick_by_search(check, "Opus 4.6")?;
    let picked = selection(check);
    // The draft keeps the implicit Fast mode off (`withImplicitFastModeDefault`); nothing else carries over.
    check.check(
        "a different model starts without the last one's options (pickModelSelection)",
        picked.model.as_ref() == "claude-opus-4-6" && option(&picked, "contextWindow").is_none() && option(&picked, "effort").is_none(),
        format!("{} {:?}", picked.model, picked.options),
    );
    click(check, "speed-1")?;
    let after = selection(check);
    check.check("the footer's Fast segment turns Fast mode on", option(&after, "fastMode") == Some(OptionValue::Bool(true)), format!("{:?}", after.options));
    let rows = column_rows(check);
    click(check, &format!("effort-{}", rows - 1))?;
    click(check, "option-menu-item-1")?;
    // A level by click: the first one the column offers.
    click(check, "effort-0")?;
    let chosen = selection(check);
    check.check(
        "a level, Fast and 1M are all in the selection",
        option(&chosen, "effort") == text("low") && option(&chosen, "fastMode") == Some(OptionValue::Bool(true)) && option(&chosen, "contextWindow") == text("1m"),
        format!("{:?}", chosen.options),
    );
    check.shot("o2-fast-and-window.png")?;
    check.press("escape");
    check.shot("o3-trigger.png")?;

    // Send: the turn carries the options, and the server's thread holds them.
    check.focus_composer();
    check.type_text("Which options did this turn carry?");
    check.press("enter");
    let k = key.clone();
    check.pump(
        move |c| shell_thread(c, &k).is_some_and(|t| t.model_selection.model == "claude-opus-4-6"),
        Duration::from_secs(30),
        "the server's thread takes the sent model",
    )?;
    let sent = shell_thread(check, &key).map(|t| t.model_selection.clone());
    let sent = sent.as_ref().map(t3code_web::provider_models_live::from_wire);
    let carried = |id: &str| sent.as_ref().and_then(|s| option(s, id));
    check.check(
        "the server's thread carries effort=low, fastMode=true and contextWindow=1m",
        carried("effort") == text("low") && carried("fastMode") == Some(OptionValue::Bool(true)) && carried("contextWindow") == text("1m"),
        format!("{:?}", sent.as_ref().map(|s| &s.options)),
    );

    // Haiku 4.5: a two-state reasoning control lists On and Off as its levels.
    pick_by_search(check, "Haiku 4.5")?;
    let rows = column_rows(check);
    check.check("Haiku 4.5's thinking switch shows On and Off rows", rows == 2, rows);
    click(check, "effort-1")?;
    let after = selection(check);
    check.check("Off writes thinking=false", option(&after, "thinking") == Some(OptionValue::Bool(false)), format!("{:?}", after.options));
    check.shot("o4-thinking.png")?;
    check.press("escape");
    let selection = selection(check);
    let record = serde_json::json!({
        "sent": sent.map(|s| s.options.iter().map(|OptionSelection { id, value }| (id.to_string(), format!("{value:?}"))).collect::<Vec<_>>()),
        "haiku": selection.options.iter().map(|o| (o.id.to_string(), format!("{:?}", o.value))).collect::<Vec<_>>(),
    });
    std::fs::write(check.out.join("options-record.json"), serde_json::to_string_pretty(&record)?)?;
    // The thread ends on Sonnet 4.6 so a fork capture of the same thread shows its column.
    check.focus_composer();
    pick_by_search(check, "Sonnet 4.6")?;
    check.press("escape");
    check.wait(Duration::from_millis(500));
    settings_chip(check)
}

fn saved_default(check: &mut Check, id: &t3code_contracts::EnvironmentId) -> Option<ModelSelection> {
    check.cx.update(|cx| ServerSettingsStore::config(cx, id).and_then(|c| c.settings.default_model_selection.as_ref().map(t3code_web::provider_models_live::from_wire)))
}

/// Settings → Threads: the default model's traits chip (`TraitsPicker` beside the settings picker).
fn settings_chip(check: &mut Check) -> anyhow::Result<()> {
    let id = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.to_string());
    let environment_id = t3code_contracts::EnvironmentId::new(id.as_str());
    let router = check.read(|root, _| root.router().clone());
    let route = Route::Settings { panel: SettingsPanel::Threads, query: SharedString::default() };
    check.cx.update(|cx| router.update(cx, |router, cx| router.navigate(route, cx)));
    check.frame();
    check.wait(Duration::from_millis(500));
    click(check, "model-trigger-defaultModelSelection")?;
    check.type_text("Sonnet 4.6");
    check.frame();
    click(check, "picker-row-0")?;
    let env = environment_id.clone();
    check.pump(move |c| saved_default(c, &env).is_some_and(|s| s.model.as_ref() == "claude-sonnet-4-6"), Duration::from_secs(15), "the default model reaches the server")?;
    check.wait(Duration::from_millis(300));
    let chip = element_center(check, "traits-trigger-defaultModelSelection");
    check.check("a traits chip sits beside the default model picker", chip.is_some(), format!("{chip:?}"));
    check.shot("o5-settings-chip.png")?;
    click(check, "traits-trigger-defaultModelSelection")?;
    let items = element_center(check, "traits-contextWindow-1").is_some() && element_center(check, "traits-effort-0").is_some();
    check.check("its menu lists the effort levels and the context windows", items, items);
    check.shot("o6-settings-chip-menu.png")?;
    click(check, "traits-contextWindow-1")?;
    let env = environment_id.clone();
    check.pump(
        move |c| saved_default(c, &env).is_some_and(|s| s.options.iter().any(|o| o.id.as_ref() == "contextWindow" && o.value == OptionValue::Text("1m".into()))),
        Duration::from_secs(15),
        "the chip's pick reaches the server's settings",
    )?;
    let saved = saved_default(check, &environment_id);
    check.check("the server saves the default model with contextWindow=1m", true, format!("{:?}", saved.map(|s| s.options)));
    let closed = element_center(check, "traits-contextWindow-1").is_none();
    check.check("a pick closes the chip's menu", closed, "");
    check.shot("o7-settings-chip-picked.png")?;
    click(check, "defaultModelSelection-reset")?;
    let env = environment_id.clone();
    check.pump(move |c| saved_default(c, &env).is_none_or(|s| s.model.as_ref() != "claude-sonnet-4-6"), Duration::from_secs(15), "reset restores the default model")?;
    Ok(())
}
