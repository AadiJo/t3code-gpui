//! The prompt stash: drafts set aside with Mod+S and restored later. Ports `promptStashStore.ts`:
//! up to 20 entries, newest first, kept under the web's `t3code:prompt-stash:v2` key, so an
//! imported install brings its stash along. Image data URLs share a 2.7M-character budget per
//! entry; images over it are recorded by name instead.

use std::sync::Arc;

use gpui_kit::{App, Global, SharedString};
use serde::{Deserialize, Serialize};

use crate::client_persistence_storage::LocalStorage;

/// The `localStorage` key, the same as the web app's.
pub const PROMPT_STASH_STORAGE_KEY: &str = "t3code:prompt-stash:v2";
const PROMPT_STASH_STORAGE_VERSION: u32 = 2;
/// The stash keeps this many prompts; a new one past it drops the oldest.
pub const MAX_STASH_ENTRIES: usize = 20;
/// Image data URL characters one entry may hold (`MAX_STASH_ENTRY_ATTACHMENT_CHARS`).
pub const MAX_STASH_ENTRY_ATTACHMENT_CHARS: usize = 2_700_000;

/// A stashed image (`PersistedComposerImageAttachment`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashedImage {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub data_url: String,
}

/// A stashed file: its finished upload, which the stash keeps (`PersistedComposerFileAttachment`).
/// `path` and `text` are this client's: where the file was read, or a long paste's text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashedFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub attachment_id: String,
    pub environment_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// One stashed prompt (`PromptStashEntry`). `records` (reply, thread and terminal contexts) are
/// kept as written so entries from the web app survive a round trip here; this client writes none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptStashEntry {
    pub id: String,
    pub created_at: String,
    pub prompt: String,
    pub attachments: Vec<StashedImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<StashedFile>>,
    pub dropped_image_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreadable_image_names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_image_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<Vec<serde_json::Value>>,
}

impl PromptStashEntry {
    /// Images the stash could not keep: over the budget, or unreadable.
    pub fn missing_image_count(&self) -> usize {
        self.dropped_image_names.len() + self.unreadable_image_names.as_ref().map_or(0, Vec::len)
    }

    pub fn file_count(&self) -> usize {
        self.files.as_ref().map_or(0, Vec::len)
    }
}

#[derive(Serialize, Deserialize, Default)]
struct StoredState {
    entries: Vec<PromptStashEntry>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    state: StoredState,
}

/// The stash in memory, loaded once from storage; every change writes it back.
#[derive(Default)]
pub struct PromptStash {
    entries: Arc<Vec<PromptStashEntry>>,
    loaded: bool,
}

impl Global for PromptStash {}

impl PromptStash {
    fn ensure(cx: &mut App) {
        if cx.try_global::<Self>().is_some_and(|s| s.loaded) {
            return;
        }
        let entries = LocalStorage::get::<Stored>(cx, PROMPT_STASH_STORAGE_KEY).map(|s| s.state.entries).unwrap_or_default();
        // Images still encoding when the app closed never finished (`clearOrphanedPendingImages`).
        let entries = entries.into_iter().map(clear_orphaned_pending_images).collect();
        cx.set_global(Self { entries: Arc::new(entries), loaded: true });
    }

    /// The stashed prompts, newest first.
    pub fn entries(cx: &mut App) -> Arc<Vec<PromptStashEntry>> {
        Self::ensure(cx);
        cx.global::<Self>().entries.clone()
    }

    /// The stash as last loaded, without loading it (render paths).
    pub fn peek(cx: &App) -> Arc<Vec<PromptStashEntry>> {
        cx.try_global::<Self>().map(|s| s.entries.clone()).unwrap_or_default()
    }

    fn write(entries: Vec<PromptStashEntry>, cx: &mut App) {
        let stored = Stored { version: PROMPT_STASH_STORAGE_VERSION, state: StoredState { entries: entries.clone() } };
        LocalStorage::set(cx, PROMPT_STASH_STORAGE_KEY, Some(&stored));
        cx.set_global(Self { entries: Arc::new(entries), loaded: true });
    }

    /// Puts `entry` first; returns the oldest entry it pushed out past [`MAX_STASH_ENTRIES`].
    pub fn stash(entry: PromptStashEntry, cx: &mut App) -> Option<PromptStashEntry> {
        Self::ensure(cx);
        let mut entries = vec![entry];
        entries.extend(cx.global::<Self>().entries.iter().cloned());
        let evicted = (entries.len() > MAX_STASH_ENTRIES).then(|| entries.pop()).flatten();
        Self::write(entries, cx);
        evicted
    }

    /// Removes and returns the entry with `id`.
    pub fn take(id: &str, cx: &mut App) -> Option<PromptStashEntry> {
        Self::ensure(cx);
        let mut entries: Vec<PromptStashEntry> = cx.global::<Self>().entries.iter().cloned().collect();
        let at = entries.iter().position(|e| e.id == id)?;
        let entry = entries.remove(at);
        Self::write(entries, cx);
        Some(entry)
    }
}

fn clear_orphaned_pending_images(mut entry: PromptStashEntry) -> PromptStashEntry {
    let pending = entry.pending_image_count.take().unwrap_or(0);
    if pending > 0 {
        let names = entry.unreadable_image_names.get_or_insert_with(Vec::new);
        names.extend((1..=pending).map(|i| format!("image {i} (not saved before reload)")));
        entry.pending_image_count = Some(0);
    }
    entry
}

/// Keeps images in order while they fit the entry's data URL budget (`partitionStashAttachments`).
pub fn partition_stash_images(images: Vec<StashedImage>) -> (Vec<StashedImage>, Vec<String>) {
    let mut used = 0;
    let (mut kept, mut dropped) = (Vec::new(), Vec::new());
    for image in images {
        if used + image.data_url.len() > MAX_STASH_ENTRY_ATTACHMENT_CHARS {
            dropped.push(image.name);
        } else {
            used += image.data_url.len();
            kept.push(image);
        }
    }
    (kept, dropped)
}

/// The row text (`stashEntrySnippet`): the prompt with chips as their labels, whitespace
/// collapsed, cut at 90 characters with an ellipsis; an empty prompt names its attachments.
pub fn stash_entry_snippet(entry: &PromptStashEntry) -> SharedString {
    use crate::composer_editor_mentions::{ChipKind, ComposerPromptSegment, split_prompt_into_segments};
    let plain: String = split_prompt_into_segments(&entry.prompt)
        .into_iter()
        .map(|segment| match segment {
            ComposerPromptSegment::Text(text) => text.to_owned(),
            ComposerPromptSegment::Chip(chip) => match chip.kind {
                ChipKind::Context { .. } | ChipKind::Citation => chip.label.to_string(),
                _ => entry.prompt[chip.range].to_owned(),
            },
        })
        .collect();
    let collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() {
        // The web cuts at 90 UTF-16 units and never on half an emoji; a char boundary does both.
        let mut units = 0;
        let cut = collapsed.char_indices().find(|(_, c)| {
            units += c.len_utf16();
            units > 90
        });
        return match cut {
            None => collapsed.into(),
            Some((at, _)) => format!("{}…", &collapsed[..at]).into(),
        };
    }
    let images = entry.attachments.len() + entry.dropped_image_names.len();
    let files = entry.file_count();
    let count = images + files;
    if count == 0 {
        return crate::i18n::t!("(empty)");
    }
    if images > 0 && files > 0 {
        crate::i18n::t!("{count, plural, one {({count} attachment)} other {({count} attachments)}}", count = count)
    } else if files > 0 {
        crate::i18n::t!("{count, plural, one {({count} file)} other {({count} files)}}", count = count)
    } else {
        crate::i18n::t!("{count, plural, one {({count} image)} other {({count} images)}}", count = count)
    }
}
