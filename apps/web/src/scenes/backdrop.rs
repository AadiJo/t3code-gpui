//! Glass panels over a grid of coloured squares, for comparing the renderer's
//! backdrop blur with Chromium's `backdrop-filter`. The same page in HTML is
//! `scripts/parity/backdrop.html`; keep the two in step.

use gpui_kit::{AnyView, App, AppContext as _, Context, IntoElement, Render, Rgba, Window, div, prelude::*, px, rgb, rgba};

/// The grid's colours, picked by `(column * 7 + row * 3) % 6`.
const PALETTE: [u32; 6] = [0xe04040, 0x40c040, 0x4060e0, 0xe0c040, 0xf0f0f0, 0x202020];
const CELL: f32 = 40.;

/// A glass panel: position, size, corner radius, blur, saturation and fill.
struct Panel {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    blur: f32,
    saturation: f32,
    fill: Rgba,
}

fn panels() -> [Panel; 5] {
    [
        Panel { x: 60., y: 60., width: 300., height: 200., radius: 12., blur: 16., saturation: 1.08, fill: rgba(0x00000000) },
        Panel { x: 420., y: 60., width: 300., height: 200., radius: 8., blur: 12., saturation: 1.14, fill: rgba(0x18181b66) },
        Panel { x: 60., y: 300., width: 300., height: 150., radius: 16., blur: 16., saturation: 1.08, fill: rgba(0x00000066) },
        Panel { x: 420., y: 300., width: 300., height: 150., radius: 0., blur: 4., saturation: 1., fill: rgba(0x0a0a0aa3) },
        Panel { x: 740., y: 430., width: 120., height: 120., radius: 10., blur: 16., saturation: 1.08, fill: rgba(0xffffff1a) },
    ]
}

struct BackdropScene;

impl Render for BackdropScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let cells = (0..20).flat_map(|column| {
            (0..13).map(move |row| {
                div()
                    .absolute()
                    .left(px(column as f32 * CELL))
                    .top(px(row as f32 * CELL))
                    .size(px(CELL))
                    .bg(rgb(PALETTE[(column * 7 + row * 3) % 6]))
            })
        });
        let glass = panels().into_iter().map(|panel| {
            div()
                .absolute()
                .left(px(panel.x))
                .top(px(panel.y))
                .w(px(panel.width))
                .h(px(panel.height))
                .rounded(px(panel.radius))
                .backdrop_blur(px(panel.blur), panel.saturation)
                .bg(panel.fill)
        });
        div()
            .size_full()
            .relative()
            .bg(rgb(0x202020))
            .children(cells)
            .children(glass)
            // Painted after the glass and above it: it must stay sharp and
            // must not show in the glass's blur.
            .child(div().absolute().left(px(320.)).top(px(280.)).size(px(80.)).bg(rgb(0xff00ff)))
    }
}

/// The comparison scene, 800x500.
pub fn build(_: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|_| BackdropScene).into()
}
