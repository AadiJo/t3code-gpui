//! Arrow-key recall of sent prompts. Ports `stepComposerPromptHistory` from
//! `composerPromptHistory.ts`. The chat view builds the entries from the
//! thread's user messages (see `buildComposerPromptHistoryEntries`).

use gpui_kit::SharedString;

/// A sent prompt that can be recalled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptHistoryEntry {
    pub id: SharedString,
    pub prompt: String,
}

/// Which entry the composer is showing, and the text it recalled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptHistoryPosition {
    pub entry_id: SharedString,
    pub recalled: String,
}

/// The result of one arrow press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptHistoryStep {
    pub position: Option<PromptHistoryPosition>,
    pub prompt: String,
}

fn find_active(entries: &[PromptHistoryEntry], position: &PromptHistoryPosition) -> Option<usize> {
    entries
        .iter()
        .position(|e| e.id == position.entry_id)
        .or_else(|| entries.iter().rposition(|e| e.prompt == position.recalled))
}

/// Steps backward (ArrowUp) or forward (ArrowDown). `None` means the arrow
/// moves the caret instead: the draft has text that is not an unedited recall.
pub fn step_prompt_history(
    backward: bool,
    entries: &[PromptHistoryEntry],
    position: Option<&PromptHistoryPosition>,
    current_prompt: &str,
) -> Option<PromptHistoryStep> {
    let active = position.filter(|p| p.recalled == current_prompt).and_then(|p| find_active(entries, p));
    let recall = |entry: &PromptHistoryEntry| PromptHistoryStep {
        position: Some(PromptHistoryPosition { entry_id: entry.id.clone(), recalled: entry.prompt.clone() }),
        prompt: entry.prompt.clone(),
    };
    if backward {
        if active.is_none() && !current_prompt.is_empty() {
            return None;
        }
        let index = match active {
            None => entries.len().checked_sub(1)?,
            Some(i) => i.checked_sub(1)?,
        };
        return entries.get(index).map(recall);
    }
    let active = active?;
    Some(match entries.get(active + 1) {
        Some(entry) => recall(entry),
        None => PromptHistoryStep { position: None, prompt: String::new() },
    })
}
