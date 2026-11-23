//! Scenes `files*`: the Files tab at a right panel's width over a real
//! directory written to the temp dir, read through [`DirectoryFiles`].
//! `files` opens a highlighted Rust file with folders expanded;
//! `files-markdown`, `files-image`, `files-csv` open those kinds;
//! `files-search` shows content search with results; `files-goto` shows go to
//! file. Each has a `-light` twin.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window, div};

use crate::{
    ActiveAppTheme as _,
    components::files::file_preview_panel::FilesPanel,
    file_source::DirectoryFiles,
    theme_palette::{AppTheme, Appearance, ThemeDocument, set_app_theme},
};

/// Writes the fixture workspace (overwriting) and returns its root:
/// `$T3_FILES_FIXTURE`, or `<temp>/t3code-files-fixture`.
pub fn fixture_workspace() -> PathBuf {
    let root = std::env::var_os("T3_FILES_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("t3code-files-fixture"));
    write_fixture(&root).expect("write files fixture");
    root
}

fn write(root: &Path, path: &str, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(full, contents)
}

/// The fixture's files: source in several languages, markdown, CSV, images,
/// a video, a PDF stand-in, a nested folder chain and an ignored folder.
pub fn write_fixture(root: &Path) -> std::io::Result<()> {
    write(root, "README.md", README)?;
    write(root, "AGENTS.md", "# Agents\n\nRun `cargo test` before committing.\n")?;
    write(root, "package.json", "{\n  \"name\": \"orbit\",\n  \"private\": true,\n  \"scripts\": { \"dev\": \"vite\" }\n}\n")?;
    write(root, "pnpm-lock.yaml", "lockfileVersion: '9.0'\n")?;
    write(root, "Cargo.toml", "[package]\nname = \"orbit\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nserde = \"1\"\n")?;
    write(root, ".gitignore", "target/\nnode_modules/\n")?;
    write(root, "src/main.rs", MAIN_RS)?;
    write(root, "src/orbit.rs", ORBIT_RS)?;
    write(root, "src/components/App.tsx", APP_TSX)?;
    write(root, "src/components/Button.tsx", "export function Button(props: { label: string }) {\n  return <button className=\"btn\">{props.label}</button>;\n}\n")?;
    write(root, "src/lib/utils.ts", "export const clamp = (value: number, min: number, max: number) =>\n  Math.min(max, Math.max(min, value));\n")?;
    write(root, "src/styles/app.css", ".btn {\n  border-radius: 8px;\n  padding: 4px 12px;\n}\n")?;
    write(root, "docs/guide/getting-started.md", "# Getting started\n\nInstall with `pnpm i`, then run `pnpm dev`.\n")?;
    write(root, "scripts/deploy.sh", "#!/usr/bin/env bash\nset -euo pipefail\necho \"deploying orbit\"\n")?;
    write(root, "data/planets.csv", PLANETS_CSV)?;
    write(root, "data/moons.tsv", "planet\tmoon\tradius_km\nEarth\tMoon\t1737\nMars\tPhobos\t11\nMars\tDeimos\t6\nJupiter\tIo\t1821\n")?;
    write(root, "assets/orbit.png", gradient_png(640, 400, [36, 99, 235], [168, 85, 247])?)?;
    write(root, "assets/sunset.png", gradient_png(480, 320, [249, 115, 22], [236, 72, 153])?)?;
    write(root, "assets/demo.mp4", [0u8, 0, 0, 24, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm'])?;
    write(root, "assets/manual.pdf", "%PDF-1.4\n% fixture\n")?;
    write(root, "node_modules/left-pad/index.js", "module.exports = (s, n) => s.padStart(n);\n")?;
    Ok(())
}

/// A two-colour diagonal gradient with a light ring, as PNG bytes.
fn gradient_png(width: u32, height: u32, from: [u8; 3], to: [u8; 3]) -> std::io::Result<Vec<u8>> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        let t = (x + y) as f32 / (width + height) as f32;
        let (cx, cy) = (x as f32 - width as f32 / 2., y as f32 - height as f32 / 2.);
        let ring = ((cx * cx + cy * cy).sqrt() - height as f32 * 0.3).abs() < 6.;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        if ring { image::Rgba([255, 255, 255, 230]) } else { image::Rgba([mix(from[0], to[0]), mix(from[1], to[1]), mix(from[2], to[2]), 255]) }
    });
    let mut bytes = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(std::io::Error::other)?;
    Ok(bytes)
}

const README: &str = "# Orbit\n\nA tiny **orbital mechanics** toolkit with a web front end.\n\n## Features\n\n- Two-body propagation with `propagate()`\n- Planet data in [`data/planets.csv`](data/planets.csv)\n- A React viewer in `src/components`\n\n## Quick start\n\n```bash\npnpm i\npnpm dev\n```\n\n| Planet | Moons |\n| --- | ---: |\n| Earth | 1 |\n| Mars | 2 |\n\n> Distances are in kilometres unless a name says otherwise.\n";

const MAIN_RS: &str = "//! Orbit: propagate a body around a central mass.\n\nmod orbit;\n\nuse orbit::{Body, propagate};\n\n/// Standard gravitational parameter of Earth, km^3/s^2.\nconst MU_EARTH: f64 = 398_600.4418;\n\nfn main() {\n    let mut body = Body::circular(7_000.0, MU_EARTH);\n    for step in 0..10 {\n        body = propagate(&body, 60.0, MU_EARTH);\n        let radius = body.radius();\n        println!(\"step {step:>2}: r = {radius:.3} km\");\n    }\n    let period = 2.0 * std::f64::consts::PI * (7_000.0f64.powi(3) / MU_EARTH).sqrt();\n    println!(\"period: {:.1} min\", period / 60.0);\n}\n";

const ORBIT_RS: &str = "/// A body's state: position and velocity in km and km/s.\n#[derive(Clone, Copy, Debug, PartialEq)]\npub struct Body {\n    pub position: [f64; 3],\n    pub velocity: [f64; 3],\n}\n\nimpl Body {\n    /// A circular orbit of `radius` km.\n    pub fn circular(radius: f64, mu: f64) -> Self {\n        let speed = (mu / radius).sqrt();\n        Self { position: [radius, 0.0, 0.0], velocity: [0.0, speed, 0.0] }\n    }\n\n    pub fn radius(&self) -> f64 {\n        self.position.iter().map(|v| v * v).sum::<f64>().sqrt()\n    }\n}\n\n/// One semi-implicit Euler step of `dt` seconds.\npub fn propagate(body: &Body, dt: f64, mu: f64) -> Body {\n    let r = body.radius();\n    let scale = -mu / (r * r * r);\n    let mut next = *body;\n    for i in 0..3 {\n        next.velocity[i] += scale * body.position[i] * dt;\n        next.position[i] += next.velocity[i] * dt;\n    }\n    next\n}\n";

const APP_TSX: &str = "import { useState } from \"react\";\nimport { Button } from \"./Button\";\n\nexport function App() {\n  const [steps, setSteps] = useState(10);\n  return (\n    <main className=\"app\">\n      <h1>Orbit</h1>\n      <Button label={`Propagate ${steps} steps`} />\n      <input type=\"range\" value={steps} onChange={(e) => setSteps(Number(e.target.value))} />\n    </main>\n  );\n}\n";

const PLANETS_CSV: &str = "name,type,radius_km,moons,distance_au\nMercury,terrestrial,2439.7,0,0.39\nVenus,terrestrial,6051.8,0,0.72\nEarth,terrestrial,6371.0,1,1.00\nMars,terrestrial,3389.5,2,1.52\nJupiter,gas giant,69911,95,5.20\nSaturn,gas giant,58232,146,9.58\nUranus,ice giant,25362,28,19.2\nNeptune,ice giant,24622,16,30.05\n\"Pluto, dwarf\",dwarf,1188.3,5,39.48\n";

/// Which scene to build.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    Code,
    Markdown,
    Image,
    Csv,
    Search,
    GoTo,
}

struct FilesScene {
    panel: Entity<FilesPanel>,
}

impl Render for FilesScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        div().size_full().bg(colors.canvas).child(self.panel.clone())
    }
}

fn build_variant(variant: Variant, appearance: Appearance, window: &mut Window, cx: &mut App) -> AnyView {
    set_app_theme(AppTheme::new(&ThemeDocument::graphite(), appearance), cx);
    gpui_kit::component::Theme::change(
        if appearance == Appearance::Light { gpui_kit::component::ThemeMode::Light } else { gpui_kit::component::ThemeMode::Dark },
        Some(window),
        cx,
    );
    let files = DirectoryFiles::new(fixture_workspace());
    let panel = cx.new(|cx| FilesPanel::new(files as Arc<_>, "orbit".into(), window, cx));
    panel.update(cx, |panel, cx| {
        let path = match variant {
            Variant::Code | Variant::Search | Variant::GoTo => "src/main.rs",
            Variant::Markdown => "README.md",
            Variant::Image => "assets/orbit.png",
            Variant::Csv => "data/planets.csv",
        };
        panel.open_file(path.into(), (variant == Variant::Code).then_some(9), window, cx);
        if variant == Variant::Markdown {
            panel.set_rendered(true, cx);
        }
        match variant {
            Variant::Search => {
                panel.open_content_search(window, cx);
                if let Some(search) = panel.content_search().cloned() {
                    search.update(cx, |search, cx| search.set_query_text("radius", window, cx));
                }
            }
            Variant::GoTo => {
                panel.open_file_picker(window, cx);
                if let Some(picker) = panel.file_picker().cloned() {
                    picker.update(cx, |picker, cx| picker.set_query_text("orb", window, cx));
                }
            }
            _ => {}
        }
    });
    // Open `src/components` too, so the tree shows nesting.
    let tree = panel.read(cx).tree().clone();
    tree.update(cx, |tree, cx| tree.expand_folder("src/components", cx));
    cx.new(|_| FilesScene { panel }).into()
}

macro_rules! scene {
    ($dark:ident, $light:ident, $variant:expr) => {
        pub fn $dark(window: &mut Window, cx: &mut App) -> AnyView {
            build_variant($variant, Appearance::Dark, window, cx)
        }
        pub fn $light(window: &mut Window, cx: &mut App) -> AnyView {
            build_variant($variant, Appearance::Light, window, cx)
        }
    };
}

scene!(build, build_light, Variant::Code);
scene!(build_markdown, build_markdown_light, Variant::Markdown);
scene!(build_image, build_image_light, Variant::Image);
scene!(build_csv, build_csv_light, Variant::Csv);
scene!(build_search, build_search_light, Variant::Search);
scene!(build_goto, build_goto_light, Variant::GoTo);
