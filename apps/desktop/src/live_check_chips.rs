//! `live-check chips`: the composer's inline chips case by case against the fork's
//! `ComposerPromptEditorTiptap` rules, through real keys and the real clipboard. What becomes a
//! chip (an `@` path that is not a scoped package like `@scope/pkg`, a known `$skill`, a known `model:<instance>/<slug>`; an unknown `$HOME`, a
//! `$20` or an unknown model stays text); one Backspace or Delete removes a chip; Shift+arrow
//! selects a chip whole; copy writes the chip's source; pasting chip text makes a chip and pads it
//! with spaces; the sent prompt is the source; a draft restored after leaving the thread has its
//! chips back; Undo brings a deleted chip back in one step. Needs the stand-in Claude provider
//! (its `quokka-notes` skill and bundled models). Run `seed` first. Writes `chips-record.json`.

use std::time::Duration;

use gpui_kit::ClipboardItem;
use t3code_web::composer_editor_mentions::ChipKind;

use super::*;

const MODEL: &str = "model:claudeAgent/claude-sonnet-4-6";

fn editor(check: &mut Check) -> Entity<t3code_web::components::composer_prompt_editor::ComposerPromptEditor> {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).editor().clone())
}

fn text(check: &mut Check) -> String {
    let e = editor(check);
    check.cx.update(|cx| e.read(cx).text().to_owned())
}

fn chips(check: &mut Check) -> Vec<(std::ops::Range<usize>, ChipKind)> {
    let e = editor(check);
    check.cx.update(|cx| e.read(cx).chips())
}

fn selection(check: &mut Check) -> std::ops::Range<usize> {
    let e = editor(check);
    check.cx.update(|cx| e.read(cx).selection())
}

fn clear(check: &mut Check) {
    let e = editor(check);
    check.cx.update(|cx| e.update(cx, |e, cx| e.set_text(String::new(), cx)));
    check.frame();
}

fn kinds(chips: &[(std::ops::Range<usize>, ChipKind)]) -> Vec<&'static str> {
    chips
        .iter()
        .map(|(_, k)| match k {
            ChipKind::Mention { .. } => "mention",
            ChipKind::Skill { .. } => "skill",
            ChipKind::Model { .. } => "model",
            ChipKind::Citation => "citation",
            ChipKind::Context { .. } => "context",
        })
        .collect()
}

pub(super) fn run_chips(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = thread_with_turn(check, &state, "Chip cases")?;
    let other = thread_with_turn(check, &state, "Chip elsewhere")?;
    check.go(&key);
    check.pump(|c| c.shown_input().entries.len() >= 2, Duration::from_secs(20), "thread shown")?;
    let picked = pick_instance(check, "claudeAgent");
    check.check("the composer sends with the stand-in Claude provider", picked, picked);
    check.wait(Duration::from_millis(300));
    check.focus_composer();
    let mut record = serde_json::Map::new();

    // 1. What becomes a chip.
    clear(check);
    check.type_text("read @README.md and @src/math.rs then $quokka-notes and $HOME or $20 with ");
    check.type_text(&format!("{MODEL} not model:claudeAgent/no-such-model "));
    let typed = chips(check);
    check.check(
        "@README.md, a known $skill and a known model become chips; @src/math.rs (a scoped-package shape), $HOME, $20 and an unknown model stay text",
        kinds(&typed) == ["mention", "skill", "model"],
        format!("{:?}", typed),
    );
    record.insert("formed".into(), format!("{typed:?}").into());
    check.shot("c0-formed.png")?;

    // 2. One Backspace after a chip removes it whole; Undo restores it in one step.
    clear(check);
    check.type_text("see $quokka-notes");
    check.type_text(" ");
    check.press("backspace");
    let before = text(check);
    check.press("backspace");
    let after = text(check);
    check.check("one Backspace after a chip deletes the whole chip", before == "see $quokka-notes" && after == "see ", format!("{before:?} -> {after:?}"));
    check.press(if cfg!(target_os = "macos") { "cmd-z" } else { "ctrl-z" });
    let undone = text(check);
    let back = chips(check);
    // `groupUndoByChangeKind`: the two Backspaces are one delete group, so the space comes back too.
    check.check("one Undo brings the chip back with the space deleted just before it", undone == "see $quokka-notes " && kinds(&back) == ["skill"], format!("{undone:?} {back:?}"));
    clear(check);
    check.type_text("see $quokka-notes now");
    let e = editor(check);
    check.cx.update(|cx| e.update(cx, |e, cx| e.set_cursor("see $quokka-notes".len(), cx)));
    check.press("backspace");
    let gone = text(check);
    check.press(if cfg!(target_os = "macos") { "cmd-z" } else { "ctrl-z" });
    let undone = text(check);
    let back = chips(check);
    check.check("Undo of a lone chip delete restores the chip in one step", gone == "see  now" && undone == "see $quokka-notes now" && kinds(&back) == ["skill"], format!("{gone:?} -> {undone:?} {back:?}"));

    // 3. Delete before a chip removes it whole.
    clear(check);
    check.type_text("see @README.md done");
    let e = editor(check);
    check.cx.update(|cx| e.update(cx, |e, cx| e.set_cursor("see ".len(), cx)));
    check.press("delete");
    let after = text(check);
    check.check("one Delete before a chip deletes the whole chip", after == "see  done", format!("{after:?}"));

    // 4. Shift+arrows take a chip as one unit.
    clear(check);
    check.type_text("a @README.md b");
    let e = editor(check);
    check.cx.update(|cx| e.update(cx, |e, cx| e.set_cursor("a @README.md".len(), cx)));
    check.press("shift-left");
    let one = selection(check);
    check.check("Shift+Left from after a chip selects the whole chip", one == (2.."a @README.md".len()), format!("{one:?}"));
    check.press("shift-left");
    let two = selection(check);
    check.check("the next Shift+Left extends over the space before it", two == (1.."a @README.md".len()), format!("{two:?}"));
    check.cx.update(|cx| e.update(cx, |e, cx| e.set_cursor(0, cx)));
    for _ in 0..3 {
        check.press("shift-right");
    }
    let across = selection(check);
    check.check("Shift+Right crosses the chip in one step", across == (0.."a @README.md".len()), format!("{across:?}"));

    // 5. Copy writes the chips' source.
    clear(check);
    let source = format!("ask {MODEL} about @README.md ");
    check.type_text(&source);
    check.press(if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" });
    check.press(if cfg!(target_os = "macos") { "cmd-c" } else { "ctrl-c" });
    let copied = check.cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    check.check("copy writes the chips' Markdown source", copied.as_deref() == Some(source.as_str()), format!("{copied:?}"));

    // 6. Paste of chip text makes the chip, with a space on either side when it needs one.
    clear(check);
    check.type_text("x");
    check.cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(MODEL.to_owned())));
    check.press(if cfg!(target_os = "macos") { "cmd-v" } else { "ctrl-v" });
    let pasted = text(check);
    let made = chips(check);
    check.check(
        "pasting model text after a word adds a space before and after it and makes a chip",
        pasted == format!("x {MODEL} ") && kinds(&made) == ["model"],
        format!("{pasted:?} {made:?}"),
    );
    clear(check);
    check.cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo $HOME $quokka-notes".to_owned())));
    check.press(if cfg!(target_os = "macos") { "cmd-v" } else { "ctrl-v" });
    let pasted = text(check);
    let made = chips(check);
    check.check(
        "pasting keeps unknown $names as text and makes the known skill a chip",
        pasted == "echo $HOME $quokka-notes " && kinds(&made) == ["skill"],
        format!("{pasted:?} {made:?}"),
    );

    // 7. A draft restored after leaving the thread has its chips back.
    clear(check);
    let draft = format!("restore @README.md and {MODEL} and $quokka-notes ");
    check.type_text(&draft);
    let before = chips(check);
    check.go(&other);
    check.wait(Duration::from_millis(500));
    check.go(&key);
    check.wait(Duration::from_millis(500));
    let restored = text(check);
    let after = chips(check);
    check.check("a restored draft keeps its text and its chips", restored == draft && kinds(&after) == kinds(&before) && after.len() == 3, format!("{restored:?} {after:?}"));

    // 8. The sent prompt is the source, chips included.
    check.focus_composer();
    let runs = projection_of(check, &key).map(|p| p.runs.len()).unwrap_or(0);
    check.press("enter");
    let k = key.clone();
    check.pump(move |c| projection_of(c, &k).is_some_and(|p| p.runs.len() > runs), Duration::from_secs(30), "the server has the sent turn")?;
    let sent = projection_of(check, &key)
        .and_then(|p| p.messages.iter().filter(|m| m.role == t3code_contracts::OrchestrationV2ConversationMessageRole::User).last().cloned())
        .map(|m| m.text.clone());
    check.check("the server's message is the chips' source", sent.as_deref().map(str::trim_end) == Some(draft.trim_end()), format!("{sent:?}"));
    record.insert("sent".into(), sent.unwrap_or_default().into());
    std::fs::write(check.out.join("chips-record.json"), serde_json::to_string_pretty(&record)?)?;
    Ok(())
}
