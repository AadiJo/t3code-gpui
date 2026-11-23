use gpui_kit::{AnyView, App, AppContext as _, Window};

use crate::AppRoot;

pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| AppRoot::new(window, cx)).into()
}
