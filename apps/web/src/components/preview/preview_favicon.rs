//! A browser tab's 12px icon (`PreviewFavicon` in `RightPanelTabs.tsx`). A streamed tab has
//! no captured favicon, so it is Google's icon for a public host (`faviconUrlForOrigin`,
//! fetched once per host) and `Globe2` for anything else or when the fetch fails. While the
//! icon loads the slot stays empty, as an unloaded `<img>` does.

use gpui_kit::{AnyElement, App, ImageSource, IntoElement, ObjectFit, div, img, prelude::*, px};

use crate::ActiveAppTheme as _;
use crate::components::chat::timeline_link_favicons::{LinkFavicon, external_link_host, page_favicon, request};
use crate::components::icons::{Icon, IconName, IconSize};

/// The icon for a tab at `url` (none while the tab is idle); starts the fetch it needs.
pub fn preview_tab_icon(url: Option<&str>, cx: &mut App) -> AnyElement {
    let size = cx.app_theme().sp(3.);
    let globe = || Icon::new(IconName::Earth).size(IconSize::Xs).into_any_element();
    let Some(host) = url.and_then(external_link_host) else { return globe() };
    let mut wanted = Vec::new();
    let favicon = page_favicon(&host, &mut wanted, cx);
    request(wanted, cx);
    match favicon {
        LinkFavicon::Image(image) => img(ImageSource::Render(image)).size(size).rounded(px(2.)).object_fit(ObjectFit::Contain).flex_none().into_any_element(),
        LinkFavicon::Loading => div().size(size).flex_none().into_any_element(),
        LinkFavicon::Mark(_) => globe(),
    }
}
