//! Per-thread composer drafts. Ports the parts of `composerDraftStore.ts` the
//! composer owns: a draft per thread key (text, attachments, model choice),
//! kept behind a small store trait and written after a 300 ms pause in typing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::SharedString;

use crate::model_selection::ModelSelection;

/// How long the composer waits after the last change before writing the draft.
pub const DRAFT_PERSIST_DEBOUNCE: Duration = Duration::from_millis(300);

/// Identifies whose draft this is: an environment and a thread (or a new-thread route).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComposerDraftKey(pub SharedString);

/// An attachment's payload kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAttachmentKind {
    /// An image with its encoded bytes, shown as a tile in the strip.
    Image { bytes: Arc<[u8]>, mime: SharedString },
    /// A file from disk.
    File { path: SharedString },
    /// A large paste kept as a text file.
    PastedText { text: Arc<str> },
}

/// Upload state of an attachment; sends wait for every upload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadState {
    Uploading,
    Uploaded,
    Failed,
}

/// One attachment in a draft. Its chip in the text is a context reference with
/// the same `context_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerAttachment {
    pub context_id: SharedString,
    pub name: SharedString,
    pub size_bytes: u64,
    pub kind: ComposerAttachmentKind,
    pub upload: UploadState,
}

/// A saved draft.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComposerDraft {
    pub prompt: String,
    pub attachments: Vec<ComposerAttachment>,
    pub model: Option<ModelSelection>,
}

impl ComposerDraft {
    /// An empty draft needs no storage.
    pub fn is_empty(&self) -> bool {
        self.prompt.is_empty() && self.attachments.is_empty() && self.model.is_none()
    }
}

/// Where drafts live. The app supplies a persistent store; scenes use [`InMemoryDraftStore`].
pub trait ComposerDraftStore {
    fn load(&self, key: &ComposerDraftKey) -> Option<ComposerDraft>;
    /// Writes a draft; an empty draft removes the entry.
    fn save(&mut self, key: &ComposerDraftKey, draft: &ComposerDraft);
}

/// Drafts kept in memory, with a write counter so tests can see the debounce work.
#[derive(Debug, Default)]
pub struct InMemoryDraftStore {
    drafts: HashMap<ComposerDraftKey, ComposerDraft>,
    pub writes: usize,
}

impl ComposerDraftStore for InMemoryDraftStore {
    fn load(&self, key: &ComposerDraftKey) -> Option<ComposerDraft> {
        self.drafts.get(key).cloned()
    }

    fn save(&mut self, key: &ComposerDraftKey, draft: &ComposerDraft) {
        self.writes += 1;
        if draft.is_empty() {
            self.drafts.remove(key);
        } else {
            self.drafts.insert(key.clone(), draft.clone());
        }
    }
}
