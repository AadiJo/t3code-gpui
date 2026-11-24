//! The composer's prompt stash (`useComposerStash.ts`, `ComposerStashBadge.tsx`,
//! `ComposerStashMenu.tsx`): Mod+S sets the draft aside, an empty composer restores the only entry
//! or opens the stash, the badge beside the banners counts the entries, and the stash drawer lists
//! them where the command menu opens, with Up/Down, Enter to restore, Mod+Backspace to delete and
//! Escape to close.

use std::sync::Arc;

use gpui_kit::{Focusable as _, StyledImage as _,
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, img, prelude::FluentBuilder as _, px,
};

use super::composer::{ChatComposer, ComposerEvent};
use super::composer_icons::composer_icon_stroke;
use super::composer_theme::{ATTACHMENT_OVERLAP, BORDER, ComposerColors, DRAWER_INSET, SURFACE_RADIUS};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::GlassExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::composer_draft_store::{ComposerAttachment, ComposerAttachmentKind, UploadState};
use crate::i18n::t;
use crate::prompt_stash_store::{
    MAX_STASH_ENTRIES, PromptStash, PromptStashEntry, StashedFile, StashedImage, partition_stash_images, stash_entry_snippet,
};

impl ChatComposer {
    /// `composer.stash` (Mod+S): stashes the draft, or with an empty composer restores the only
    /// entry or toggles the stash. With a question waiting it only toggles the stash, and with no
    /// project chosen it does nothing; the key is always taken.
    pub fn stash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        PromptStash::entries(cx);
        if self.state().placeholder.is_some() {
            return self.toggle_stash_menu(cx);
        }
        if self.state().project_required {
            return;
        }
        let text = self.editor().read(cx).text().to_owned();
        let trimmed = text.trim();
        // A trailing chip only parses with whitespace after it, so keep one space to restore it.
        let ends_with_chip = crate::composer_editor_mentions::split_prompt_into_segments(&text)
            .last()
            .is_some_and(|s| matches!(s, crate::composer_editor_mentions::ComposerPromptSegment::Chip(c) if c.range.end == text.trim_end().len()));
        let prompt = if ends_with_chip { format!("{trimmed} ") } else { trimmed.to_owned() };
        if prompt.is_empty() && self.attachments.is_empty() {
            let entries = PromptStash::entries(cx);
            match entries.as_slice() {
                [only] if only.pending_image_count.unwrap_or(0) == 0 => {
                    let id = only.id.clone();
                    self.restore_stash_entry(&id, window, cx);
                }
                _ => self.toggle_stash_menu(cx),
            }
            return;
        }
        if self.converting.contains(&self.state().draft_key) {
            Toaster::push(
                ToastData::new(ToastKind::Info, t!("Still compressing a pasted image.")).description(t!("Stash again once it finishes compressing.")),
                cx,
            );
            return;
        }
        let mut images = Vec::new();
        let mut files = Vec::new();
        for attachment in &self.attachments {
            match &attachment.kind {
                ComposerAttachmentKind::Image { bytes, mime } => {
                    use base64::Engine as _;
                    images.push(StashedImage {
                        id: attachment.context_id.to_string(),
                        name: attachment.name.to_string(),
                        mime_type: mime.to_string(),
                        size_bytes: attachment.size_bytes,
                        data_url: format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)),
                    });
                }
                kind => {
                    let upload = crate::state::attachments::AttachmentUploads::persisted(&attachment.context_id);
                    let Some(upload) = upload.filter(|_| attachment.upload == UploadState::Uploaded) else {
                        Toaster::push(ToastData::new(ToastKind::Error, t!("Wait for file uploads before stashing this prompt")), cx);
                        return;
                    };
                    let (path, text, mime) = match kind {
                        ComposerAttachmentKind::File { path } => (
                            Some(path.to_string()),
                            None,
                            crate::state::attachments::mime_from_name(&attachment.name).unwrap_or("application/octet-stream").to_owned(),
                        ),
                        ComposerAttachmentKind::PastedText { text } => {
                            (None, Some(text.to_string()), crate::state::attachments::PASTED_TEXT_MIME.to_owned())
                        }
                        ComposerAttachmentKind::Image { .. } => unreachable!(),
                    };
                    files.push(StashedFile {
                        id: attachment.context_id.to_string(),
                        name: attachment.name.to_string(),
                        mime_type: mime,
                        size_bytes: attachment.size_bytes,
                        attachment_id: upload.attachment_id,
                        environment_id: upload.environment_id.to_string(),
                        path,
                        text,
                    });
                }
            }
        }
        let image_ids: Vec<SharedString> = images.iter().map(|i| SharedString::from(i.id.clone())).collect();
        let (kept, dropped) = partition_stash_images(images);
        let entry = PromptStashEntry {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: t3code_client_runtime::state::models::format_ms(t3code_client_runtime::state::models::now_ms()),
            prompt,
            attachments: kept,
            files: (!files.is_empty()).then_some(files),
            dropped_image_names: dropped,
            unreadable_image_names: None,
            pending_image_count: None,
            records: None,
        };
        let evicted = PromptStash::stash(entry, cx);
        // The stash keeps file uploads; images are kept as bytes, so their uploads go.
        self.attachments.clear();
        for id in image_ids {
            cx.emit(ComposerEvent::AttachmentRemoved(id));
        }
        self.editor().clone().update(cx, |e, cx| e.set_text(String::new(), cx));
        self.save_stashed_draft(cx);
        if let Some(evicted) = evicted {
            if let Some(files) = evicted.files.filter(|f| !f.is_empty()) {
                cx.emit(ComposerEvent::ReleaseStashedFiles(files));
            }
            Toaster::push(
                ToastData::new(ToastKind::Warning, t!("Oldest stashed prompt discarded"))
                    .description(t!("The stash holds {max} prompts; the oldest was removed to make room.", max = MAX_STASH_ENTRIES)),
                cx,
            );
        }
        cx.notify();
    }

    /// Opens or closes the stash drawer, on the newest entry.
    pub fn toggle_stash_menu(&mut self, cx: &mut Context<Self>) {
        self.stash_menu = if self.stash_menu.is_some() { None } else { Some(0) };
        cx.notify();
    }

    /// Whether the stash drawer is open, and the highlighted row.
    pub fn stash_menu(&self) -> Option<usize> {
        self.stash_menu
    }

    /// Takes an entry back into the composer: its prompt after the current text (a blank line
    /// between), its files with their uploads and its images re-uploaded, up to the attachment limit.
    pub fn restore_stash_entry(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let pending_question = self.state().placeholder.is_some();
        let Some(entry) = PromptStash::peek(cx).iter().find(|e| e.id == id).cloned() else { return };
        if pending_question && entry.records.as_ref().is_some_and(|r| r.iter().any(|r| r.get("kind").and_then(|k| k.as_str()) == Some("environment"))) {
            Toaster::push(
                ToastData::new(ToastKind::Error, t!("Stashed machines can't go in an answer"))
                    .description(t!("Restore this prompt after the agent's question is answered.")),
                cx,
            );
            return;
        }
        self.stash_menu = None;
        let Some(entry) = PromptStash::take(id, cx) else { return };
        let current = self.editor().read(cx).text().to_owned();
        let restored = entry.prompt.clone();
        let next = if restored.is_empty() {
            current.clone()
        } else if !current.trim().is_empty() {
            format!("{}\n\n{restored}", current.trim_end())
        } else {
            restored
        };
        let held_ids: Vec<SharedString> = self.attachments.iter().map(|a| a.context_id.clone()).collect();
        let mut capacity = crate::state::attachments::MAX_ATTACHMENTS.saturating_sub(self.attachments.len());
        let mut skipped = Vec::new();
        let mut files = Vec::new();
        for file in entry.files.iter().flatten() {
            if held_ids.iter().any(|id| id.as_ref() == file.id) {
                continue;
            }
            if capacity == 0 {
                skipped.push(file.name.clone());
                continue;
            }
            capacity -= 1;
            let upload = t3code_client_runtime::state::drafts::DraftUpload {
                attachment_id: file.attachment_id.clone(),
                environment_id: t3code_contracts::EnvironmentId::new(file.environment_id.as_str()),
            };
            let state = crate::state::attachments::AttachmentUploads::restore(&file.id, Some(&upload), &file.mime_type, file.size_bytes);
            let kind = match (&file.text, &file.path) {
                (Some(text), _) => ComposerAttachmentKind::PastedText { text: Arc::from(text.as_str()) },
                (None, path) => ComposerAttachmentKind::File { path: path.clone().unwrap_or_default().into() },
            };
            files.push(ComposerAttachment { context_id: file.id.clone().into(), name: file.name.clone().into(), size_bytes: file.size_bytes, kind, upload: state });
        }
        let mut images = Vec::new();
        for image in &entry.attachments {
            if held_ids.iter().any(|id| id.as_ref() == image.id) {
                continue;
            }
            if capacity == 0 {
                skipped.push(image.name.clone());
                continue;
            }
            let Some(bytes) = decode_data_url(&image.data_url) else { continue };
            capacity -= 1;
            images.push(ComposerAttachment {
                context_id: image.id.clone().into(),
                name: image.name.clone().into(),
                size_bytes: image.size_bytes,
                kind: ComposerAttachmentKind::Image { bytes: Arc::from(bytes), mime: image.mime_type.clone().into() },
                upload: UploadState::Uploading,
            });
        }
        // A restored file without its chip in the text gets one at the end (`appendReference`).
        let mut text = next;
        for file in &files {
            if !text.contains(file.context_id.as_ref()) {
                let chip = crate::composer_editor_mentions::format_context_reference("file", &file.context_id, &file.name);
                text = if text.is_empty() { format!("{chip} ") } else { format!("{} {chip} ", text.trim_end()) };
            }
        }
        let end = text.len();
        self.editor().clone().update(cx, |e, cx| {
            e.set_text(text, cx);
            e.set_cursor(end, cx);
        });
        self.attachments.extend(files);
        if !images.is_empty() {
            self.attachments.extend(images.iter().cloned());
            cx.emit(ComposerEvent::AttachmentsAdded(images));
        }
        let mut reasons = Vec::new();
        if !entry.dropped_image_names.is_empty() {
            reasons.push(t!("{names} exceeded the stash size limit when this prompt was saved.", names = entry.dropped_image_names.join(", ")));
        }
        if let Some(names) = entry.unreadable_image_names.as_ref().filter(|n| !n.is_empty()) {
            reasons.push(t!("{names} could not be read when this prompt was saved.", names = names.join(", ")));
        }
        if !skipped.is_empty() {
            reasons.push(t!(
                "{names} could not be restored: the composer is at its {limit}-attachment limit.",
                names = skipped.join(", "),
                limit = crate::state::attachments::MAX_ATTACHMENTS
            ));
        }
        if !reasons.is_empty() {
            let description = reasons.iter().map(|r| r.as_ref()).collect::<Vec<_>>().join(" ");
            Toaster::push(ToastData::new(ToastKind::Warning, t!("Some attachments were not restored")).description(description), cx);
        }
        self.save_stashed_draft(cx);
        window.focus(&self.editor().focus_handle(cx), cx);
        cx.notify();
    }

    /// Deletes an entry and the file uploads it kept; the drawer closes when it was the last.
    pub fn delete_stash_entry(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(entry) = PromptStash::take(id, cx) else { return };
        let left = PromptStash::peek(cx).len();
        if left == 0 {
            self.stash_menu = None;
        } else if let Some(at) = self.stash_menu.as_mut() {
            *at = (*at).min(left - 1);
        }
        if let Some(files) = entry.files.filter(|f| !f.is_empty()) {
            cx.emit(ComposerEvent::ReleaseStashedFiles(files));
        }
        cx.notify();
    }

    /// Steps the drawer's highlight, wrapping.
    pub(super) fn step_stash_menu(&mut self, down: bool, cx: &mut Context<Self>) {
        let count = PromptStash::peek(cx).len();
        if let Some(at) = self.stash_menu.as_mut()
            && count > 0
        {
            *at = if down { (*at + 1) % count } else { (*at + count - 1) % count };
            cx.notify();
        }
    }

    /// Restores (`restore`) or deletes the highlighted entry.
    pub(super) fn act_on_highlighted(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.stash_menu else { return };
        let Some(id) = PromptStash::peek(cx).get(at).map(|e| e.id.clone()) else { return };
        if restore { self.restore_stash_entry(&id, window, cx) } else { self.delete_stash_entry(&id, cx) }
    }

    /// The badge beside the banners (`ComposerStashBadge`): a content-width attached card,
    /// `density="comfortable"`, shown while the stash holds anything.
    pub(super) fn render_stash_badge(&self, c: &ComposerColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let count = PromptStash::peek(cx).len();
        if count == 0 {
            return None;
        }
        let open = self.stash_menu.is_some();
        let ink = if open { c.foreground } else { c.muted_foreground };
        let (hover_bg, fg) = (c.foreground.opacity(0.05), c.foreground);
        let row = div()
            .id("stash-badge")
            .h(c.r(px(24.)))
            .flex()
            .items_center()
            .gap(c.r(px(4.)))
            .rounded(c.r(px(12.)))
            .text_color(ink)
            .when(!open, |el| el.cursor_pointer().hover(move |s| s.bg(hover_bg).text_color(fg)))
            .child(banner_icon(IconName::Bookmark, c.muted_foreground, c))
            .child(div().child(t!("Stash")))
            .child(count_label(count, c.muted_foreground, c))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_stash_menu(cx)));
        Some(
            div()
                .flex_none()
                .px(c.r(px(4.)) - BORDER)
                .pt(c.r(px(5.)) - BORDER)
                .pb(c.r(px(5.)) + c.r(ATTACHMENT_OVERLAP) - BORDER)
                .rounded_t(c.r(SURFACE_RADIUS))
                .glass(c.glass_surface)
                .border_1()
                .border_color(c.outline)
                .text_size(c.r(px(12.)))
                .line_height(c.r(px(16.)))
                .child(row)
                .into_any_element(),
        )
    }

    /// The stash drawer (`ComposerStashMenu`), in the command menu's place above the card.
    pub(super) fn render_stash_menu(&self, c: &ComposerColors, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let highlighted = self.stash_menu?;
        let entries = PromptStash::peek(cx);
        let header = div()
            .id("stash-close")
            .h(c.r(px(24.)))
            .flex()
            .items_center()
            .gap(c.r(px(4.)))
            .rounded(c.r(px(12.)))
            .cursor_pointer()
            .hover({
                let h = c.foreground.opacity(0.05);
                move |s| s.bg(h)
            })
            .child(banner_icon(IconName::Bookmark, c.muted_foreground, c))
            .child(div().flex_1().min_w_0().text_color(c.muted_foreground).child(t!("Stash")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(c.r(px(4.)))
                    .child(count_label(entries.len(), c.muted_foreground, c))
                    .child(div().size(c.r(px(24.))).flex().items_center().justify_center().child(
                        Icon::new(IconName::ChevronDown).size(IconSize::Custom(c.r(px(14.)))).color(c.muted_foreground),
                    )),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.stash_menu = None;
                cx.notify();
            }));
        let rows: Vec<AnyElement> = if entries.is_empty() {
            let shortcut = cx
                .try_global::<crate::shortcuts::keymap::ShortcutState>()
                .and_then(|s| s.label(&t3code_shared::keybindings::KeybindingCommand::Static(t3code_shared::keybindings::StaticCommand::ComposerStash)));
            let text = match shortcut {
                Some(label) => format!("{} {}", t!("Nothing stashed yet."), t!("Press {shortcut} with a prompt in the composer to stash it.", shortcut = label)),
                None => t!("Nothing stashed yet.").to_string(),
            };
            vec![div().min_h(c.r(px(20.))).flex().items_center().pl(c.r(px(28.))).text_color(c.muted_foreground).child(text).into_any_element()]
        } else {
            entries.iter().enumerate().map(|(i, entry)| self.render_stash_row(i, entry, i == highlighted, c, cx)).collect()
        };
        let max = c.r(px(384.)).min(window.viewport_size().height * 0.4);
        Some(
            div()
                .absolute()
                .left(c.r(DRAWER_INSET))
                .right(c.r(DRAWER_INSET))
                .bottom(gpui_kit::relative(1.))
                .mb(-c.r(ATTACHMENT_OVERLAP))
                .pb(c.r(ATTACHMENT_OVERLAP) + c.r(px(4.)) - BORDER)
                .pt(c.r(px(4.)) - BORDER)
                .px(c.r(px(4.)) - BORDER)
                .rounded_t(c.r(SURFACE_RADIUS))
                .glass(c.glass_surface)
                .border_1()
                .border_color(c.outline)
                .text_size(c.r(px(12.)))
                .line_height(c.r(px(16.)))
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.stash_menu = None;
                    cx.notify();
                }))
                .child(header)
                .child(div().id("stash-list").max_h(max).overflow_y_scroll().flex().flex_col().gap(px(1.)).children(rows))
                .into_any_element(),
        )
    }

    fn render_stash_row(&self, index: usize, entry: &PromptStashEntry, highlighted: bool, c: &ComposerColors, cx: &mut Context<Self>) -> AnyElement {
        let id = SharedString::from(entry.id.clone());
        let missing = entry.missing_image_count();
        let status: Option<AnyElement> = match entry.pending_image_count.unwrap_or(0) {
            0 if missing > 0 => Some(
                div()
                    .flex_none()
                    .text_color(c.warning)
                    .child(t!("{count, plural, one {{count} image dropped} other {{count} images dropped}}", count = missing))
                    .into_any_element(),
            ),
            0 => None,
            pending => Some(
                div()
                    .flex_none()
                    .text_color(c.muted_foreground)
                    .child(t!("{count, plural, one {saving {count} image…} other {saving {count} images…}}", count = pending))
                    .into_any_element(),
            ),
        };
        let thumbs = (!entry.attachments.is_empty()).then(|| {
            div().flex().flex_none().items_center().children(entry.attachments.iter().take(3).enumerate().filter_map(|(i, image)| {
                let bytes = decode_data_url(&image.data_url)?;
                let format = match image.mime_type.as_str() {
                    "image/jpeg" => gpui_kit::ImageFormat::Jpeg,
                    "image/gif" => gpui_kit::ImageFormat::Gif,
                    "image/webp" => gpui_kit::ImageFormat::Webp,
                    _ => gpui_kit::ImageFormat::Png,
                };
                Some(
                    img(Arc::new(gpui_kit::Image::from_bytes(format, bytes)))
                        .size(c.r(px(16.)))
                        .when(i > 0, |el| el.ml(-c.r(px(6.))))
                        .rounded(c.r(px(4.)))
                        .border_1()
                        .border_color(c.border.opacity(0.7))
                        .object_fit(gpui_kit::ObjectFit::Cover),
                )
            }))
        });
        let files = (entry.file_count() > 0).then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(c.r(px(4.)))
                .text_color(c.muted_foreground)
                .child(Icon::new(IconName::File).size(IconSize::Custom(c.r(px(12.)))).color(c.muted_foreground))
                .child(entry.file_count().to_string())
        });
        let time = match crate::components::relative_time::relative_time_state(Some(&entry.created_at), crate::timestamp_format::now_ms()) {
            crate::timestamp_format::RelativeTimeState::Relative(parts) => parts.label,
            _ => SharedString::default(),
        };
        let (restore_id, delete_id) = (id.clone(), id.clone());
        div()
            .id(SharedString::from(format!("stash-row-{index}")))
            .h(c.r(px(24.)))
            .flex()
            .items_center()
            .gap(c.r(px(4.)))
            .rounded(c.r(px(4.)))
            .cursor_pointer()
            .when(highlighted, |el| el.bg(c.accent).text_color(c.accent_foreground))
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.stash_menu.is_some_and(|at| at != index) {
                    this.stash_menu = Some(index);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.restore_stash_entry(&restore_id, window, cx)))
            .child(banner_icon(IconName::FileText, c.muted_foreground, c))
            .child(div().flex_1().min_w_0().truncate().text_color(c.foreground.opacity(0.8)).child(stash_entry_snippet(entry)))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(c.r(px(4.)))
                    .children(status)
                    .children(thumbs)
                    .children(files)
                    .child(div().flex_none().text_color(c.muted_foreground).child(time))
                    .child(
                        Button::new(SharedString::from(format!("stash-delete-{index}")))
                            .icon(IconName::X)
                            .icon_size(IconSize::Custom(c.r(px(14.))))
                            .size(ButtonSize::IconXs)
                            .variant(ButtonVariant::Ghost)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.delete_stash_entry(&delete_id, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// `ComposerBanner.Icon`: the icon column, a 12px glyph centred in it.
fn banner_icon(icon: IconName, color: gpui_kit::Hsla, c: &ComposerColors) -> impl IntoElement {
    div()
        .w(c.r(px(24.)))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).size(IconSize::Custom(c.r(px(12.)))).color(color).stroke(composer_icon_stroke(icon)))
}

/// `ComposerBanner.Count`: medium, at least the icon column wide, centred.
fn count_label(count: usize, color: gpui_kit::Hsla, c: &ComposerColors) -> impl IntoElement {
    div().min_w(c.r(px(24.))).flex().justify_center().font_weight(FontWeight::MEDIUM).text_color(color).child(count.to_string())
}

fn decode_data_url(data_url: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let (_, data) = data_url.split_once(";base64,")?;
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}
