//! Per-type file icons. Ports `pierre-icons.ts`: `@pierre/trees`' complete,
//! coloured built-in set plus T3's own sprite, resolved by file name and then
//! by each extension suffix. The SVGs live in `assets/icons/files`; GPUI draws
//! an SVG as a mask, so each icon is one or two layers tinted here. The file
//! tree, the diff file tree, search results and chat file chips share this.

use gpui_kit::{App, Hsla, IntoElement, ParentElement as _, Pixels, Styled as _, div, svg};

use crate::{ActiveAppTheme as _, file_preview::VIDEO_FILE_EXTENSIONS, syntax::hex, theme_palette::Appearance};

/// T3's icons from `T3_FILE_ICON_SPRITE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum T3Icon {
    Agents,
    Pnpm,
    Video,
    Diff,
    Xml,
    Pdf,
    Word,
    Spreadsheet,
    Slides,
    Epub,
    Csharp,
    Dart,
    Elixir,
    Haskell,
    Java,
    Kotlin,
    Lua,
    Php,
    Powershell,
    R,
    Scala,
    Toml,
}

/// A resolved file icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PierreIcon {
    BuiltIn(BuiltInToken),
    T3(T3Icon),
}

/// How one SVG layer is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconTint {
    /// `--trees-fg-muted`: `currentColor` inside the icon section.
    Muted,
    Palette(TreeIconColor),
    /// A literal colour in the sprite.
    Fixed(&'static str),
}

/// One tinted SVG.
#[derive(Clone, Copy, Debug)]
pub struct IconLayer {
    pub asset: &'static str,
    pub tint: IconTint,
}

impl T3Icon {
    fn layers(self) -> &'static [IconLayer] {
        use IconTint::*;
        use TreeIconColor as C;
        macro_rules! one {
            ($asset:literal, $tint:expr) => {
                &[IconLayer { asset: $asset, tint: $tint }]
            };
        }
        match self {
            Self::Agents => one!("icons/files/t3-agents.svg", Muted),
            Self::Pnpm => &[
                IconLayer { asset: "icons/files/t3-pnpm-accent.svg", tint: Fixed("#f9ad00") },
                IconLayer { asset: "icons/files/t3-pnpm.svg", tint: Muted },
            ],
            Self::Video => one!("icons/files/t3-video.svg", Fixed("#a631be")),
            Self::Diff => one!("icons/files/t3-diff.svg", Palette(C::Green)),
            Self::Xml => one!("icons/files/t3-xml.svg", Palette(C::Orange)),
            Self::Pdf => one!("icons/files/t3-pdf.svg", Palette(C::Red)),
            Self::Word => one!("icons/files/t3-word.svg", Palette(C::Blue)),
            Self::Spreadsheet => one!("icons/files/t3-spreadsheet.svg", Palette(C::Green)),
            Self::Slides => one!("icons/files/t3-slides.svg", Palette(C::Orange)),
            Self::Epub => one!("icons/files/t3-epub.svg", Palette(C::Indigo)),
            Self::Csharp => one!("icons/files/t3-csharp.svg", Palette(C::Indigo)),
            Self::Dart => one!("icons/files/t3-dart.svg", Palette(C::Cyan)),
            Self::Elixir => one!("icons/files/t3-elixir.svg", Palette(C::Purple)),
            Self::Haskell => one!("icons/files/t3-haskell.svg", Palette(C::Indigo)),
            Self::Java => one!("icons/files/t3-java.svg", Palette(C::Orange)),
            Self::Kotlin => one!("icons/files/t3-kotlin.svg", Palette(C::Indigo)),
            Self::Lua => one!("icons/files/t3-lua.svg", Palette(C::Blue)),
            Self::Php => one!("icons/files/t3-php.svg", Palette(C::Indigo)),
            Self::Powershell => one!("icons/files/t3-powershell.svg", Palette(C::Blue)),
            Self::R => one!("icons/files/t3-r.svg", Palette(C::Blue)),
            Self::Scala => one!("icons/files/t3-scala.svg", Palette(C::Red)),
            Self::Toml => one!("icons/files/t3-toml.svg", Palette(C::Gray)),
        }
    }
}

/// `T3_PIERRE_ICONS.byFileName`.
fn by_file_name(lower_name: &str) -> Option<PierreIcon> {
    Some(match lower_name {
        "package.json" => PierreIcon::BuiltIn(BuiltInToken::Npm),
        "tsconfig.json" => PierreIcon::BuiltIn(BuiltInToken::Typescript),
        "agents.md" => PierreIcon::T3(T3Icon::Agents),
        "pnpm-lock.yaml" | "pnpm-workspace.yaml" => PierreIcon::T3(T3Icon::Pnpm),
        _ => return None,
    })
}

/// `T3_PIERRE_ICONS.byFileExtension`.
fn by_extension(extension: &str) -> Option<PierreIcon> {
    use T3Icon::*;
    if VIDEO_FILE_EXTENSIONS.contains(&extension) {
        return Some(PierreIcon::T3(Video));
    }
    let icon = match extension {
        "pdf" => Pdf,
        "doc" | "docx" | "odt" | "rtf" => Word,
        "xls" | "xlsx" | "ods" | "csv" | "tsv" => Spreadsheet,
        "ppt" | "pptx" | "odp" => Slides,
        "epub" => Epub,
        "cs" | "csx" => Csharp,
        "dart" => Dart,
        "diff" | "patch" => Diff,
        "ex" | "exs" => Elixir,
        "hs" | "lhs" => Haskell,
        "heic" | "heif" => return Some(PierreIcon::BuiltIn(BuiltInToken::Image)),
        "java" => Java,
        "kt" | "kts" => Kotlin,
        "lua" | "luau" => Lua,
        "php" => Php,
        "ps1" | "psm1" | "psd1" => Powershell,
        "r" => R,
        "scala" => Scala,
        "toml" => Toml,
        "xml" | "xsd" | "xsl" | "xslt" => Xml,
        _ => return None,
    };
    Some(PierreIcon::T3(icon))
}

fn lookup(table: &[(&str, BuiltInToken)], key: &str) -> Option<BuiltInToken> {
    table.binary_search_by(|(k, _)| (*k).cmp(key)).ok().map(|i| table[i].1)
}

/// `resolvePierreIconForEntry(path, "file")`: T3's names, then T3's
/// extensions, then Pierre's names, complete-set overrides and extensions.
pub fn resolve_file_icon(path: &str) -> PierreIcon {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    if let Some(icon) = by_file_name(&lower) {
        return icon;
    }
    // `getExtensionCandidates`: every suffix after a dot, longest first.
    let candidates: Vec<&str> = lower.match_indices('.').map(|(i, _)| &lower[i + 1..]).collect();
    if let Some(icon) = candidates.iter().find_map(|ext| by_extension(ext)) {
        return icon;
    }
    if let Some(token) = lookup(FILE_NAME_TOKENS, &lower) {
        return PierreIcon::BuiltIn(token);
    }
    for ext in &candidates {
        if let Some(token) = lookup(COMPLETE_EXTENSION_OVERRIDES, ext).or_else(|| lookup(EXTENSION_TOKENS, ext)) {
            return PierreIcon::BuiltIn(token);
        }
    }
    PierreIcon::BuiltIn(BuiltInToken::Default)
}

impl PierreIcon {
    /// The SVG layers, bottom first.
    pub fn layers(self) -> &'static [IconLayer] {
        match self {
            Self::T3(icon) => icon.layers(),
            Self::BuiltIn(token) => token.layer(),
        }
    }

    /// True when the name matched nothing more specific than the generic file.
    pub fn is_default(self) -> bool {
        self == Self::BuiltIn(BuiltInToken::Default)
    }
}

impl IconTint {
    /// The colour for an appearance; `muted` is the surface's `--trees-fg-muted`.
    pub fn color(self, appearance: Appearance, muted: Hsla) -> Hsla {
        match self {
            Self::Muted => muted,
            Self::Fixed(value) => hex(value).unwrap_or(muted),
            Self::Palette(color) => {
                let (light, dark) = color.hex();
                hex(if appearance == Appearance::Dark { dark } else { light }).unwrap_or(muted)
            }
        }
    }
}

/// Draws a file's icon at `size` (16px in the tree, 14px in search results).
pub fn file_icon(path: &str, size: Pixels, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.app_theme();
    pierre_icon(resolve_file_icon(path), size, theme.appearance, theme.colors.muted_foreground)
}

/// Draws a resolved icon with explicit colours.
pub fn pierre_icon(icon: PierreIcon, size: Pixels, appearance: Appearance, muted: Hsla) -> impl IntoElement + use<> {
    let layers = icon.layers();
    div().relative().size(size).flex_none().children(layers.iter().map(move |layer| {
        svg().absolute().top_0().left_0().size(size).path(layer.asset).text_color(layer.tint.color(appearance, muted))
    }))
}

impl BuiltInToken {
    fn layer(self) -> &'static [IconLayer] {
        // One static layer per token, built once.
        static LAYERS: std::sync::LazyLock<Vec<(BuiltInToken, [IconLayer; 1])>> = std::sync::LazyLock::new(|| {
            ALL_TOKENS
                .iter()
                .map(|&token| {
                    let tint = token.color().map_or(IconTint::Muted, IconTint::Palette);
                    (token, [IconLayer { asset: token.asset(), tint }])
                })
                .collect()
        });
        let entry = LAYERS.iter().find(|(token, _)| *token == self).expect("every token has a layer");
        &entry.1
    }
}

// Generated from @pierre/trees 1.0.0-beta.6 (dist/builtInIcons.js, dist/style.js)
// by an extraction script; do not edit by hand.

/// A built-in Pierre file icon token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltInToken {
    Astro,
    Babel,
    Bash,
    Biome,
    Bootstrap,
    Browserslist,
    Bun,
    C,
    Claude,
    Cpp,
    Css,
    Database,
    Default,
    Docker,
    Eslint,
    Font,
    Git,
    Go,
    Graphql,
    Html,
    Image,
    Javascript,
    Json,
    Markdown,
    Mcp,
    Nextjs,
    Npm,
    Oxc,
    Postcss,
    Prettier,
    Python,
    React,
    Ruby,
    Rust,
    Sass,
    Stylelint,
    Svelte,
    Svg,
    Svgo,
    Swift,
    Table,
    Tailwind,
    Terraform,
    Text,
    Typescript,
    Vite,
    Vscode,
    Vue,
    Wasm,
    Webpack,
    Yml,
    Zig,
    Zip,
}

impl BuiltInToken {
    /// The icon's SVG asset path.
    pub fn asset(self) -> &'static str {
        match self {
            Self::Astro => "icons/files/astro.svg",
            Self::Babel => "icons/files/babel.svg",
            Self::Bash => "icons/files/bash.svg",
            Self::Biome => "icons/files/biome.svg",
            Self::Bootstrap => "icons/files/bootstrap.svg",
            Self::Browserslist => "icons/files/browserslist.svg",
            Self::Bun => "icons/files/bun.svg",
            Self::C => "icons/files/c.svg",
            Self::Claude => "icons/files/claude.svg",
            Self::Cpp => "icons/files/cpp.svg",
            Self::Css => "icons/files/css.svg",
            Self::Database => "icons/files/database.svg",
            Self::Default => "icons/files/default.svg",
            Self::Docker => "icons/files/docker.svg",
            Self::Eslint => "icons/files/eslint.svg",
            Self::Font => "icons/files/font.svg",
            Self::Git => "icons/files/git.svg",
            Self::Go => "icons/files/go.svg",
            Self::Graphql => "icons/files/graphql.svg",
            Self::Html => "icons/files/html.svg",
            Self::Image => "icons/files/image.svg",
            Self::Javascript => "icons/files/javascript.svg",
            Self::Json => "icons/files/json.svg",
            Self::Markdown => "icons/files/markdown.svg",
            Self::Mcp => "icons/files/mcp.svg",
            Self::Nextjs => "icons/files/nextjs.svg",
            Self::Npm => "icons/files/npm.svg",
            Self::Oxc => "icons/files/oxc.svg",
            Self::Postcss => "icons/files/postcss.svg",
            Self::Prettier => "icons/files/prettier.svg",
            Self::Python => "icons/files/python.svg",
            Self::React => "icons/files/react.svg",
            Self::Ruby => "icons/files/ruby.svg",
            Self::Rust => "icons/files/rust.svg",
            Self::Sass => "icons/files/sass.svg",
            Self::Stylelint => "icons/files/stylelint.svg",
            Self::Svelte => "icons/files/svelte.svg",
            Self::Svg => "icons/files/svg.svg",
            Self::Svgo => "icons/files/svgo.svg",
            Self::Swift => "icons/files/swift.svg",
            Self::Table => "icons/files/table.svg",
            Self::Tailwind => "icons/files/tailwind.svg",
            Self::Terraform => "icons/files/terraform.svg",
            Self::Text => "icons/files/text.svg",
            Self::Typescript => "icons/files/typescript.svg",
            Self::Vite => "icons/files/vite.svg",
            Self::Vscode => "icons/files/vscode.svg",
            Self::Vue => "icons/files/vue.svg",
            Self::Wasm => "icons/files/wasm.svg",
            Self::Webpack => "icons/files/webpack.svg",
            Self::Yml => "icons/files/yml.svg",
            Self::Zig => "icons/files/zig.svg",
            Self::Zip => "icons/files/zip.svg",
        }
    }

    /// The `--trees-icon-*` colour the token is tinted with; `None` is `--trees-fg-muted`.
    pub fn color(self) -> Option<TreeIconColor> {
        match self {
            Self::Astro => Some(TreeIconColor::Purple),
            Self::Babel => Some(TreeIconColor::Yellow),
            Self::Bash => Some(TreeIconColor::Green),
            Self::Biome => Some(TreeIconColor::Blue),
            Self::Bootstrap => Some(TreeIconColor::Indigo),
            Self::Browserslist => Some(TreeIconColor::Yellow),
            Self::Bun => Some(TreeIconColor::Mauve),
            Self::C => Some(TreeIconColor::Blue),
            Self::Claude => Some(TreeIconColor::Orange),
            Self::Cpp => Some(TreeIconColor::Blue),
            Self::Css => Some(TreeIconColor::Indigo),
            Self::Database => Some(TreeIconColor::Purple),
            Self::Default => Some(TreeIconColor::Gray),
            Self::Docker => Some(TreeIconColor::Blue),
            Self::Eslint => Some(TreeIconColor::Indigo),
            Self::Font => None,
            Self::Git => Some(TreeIconColor::Vermilion),
            Self::Go => Some(TreeIconColor::Cyan),
            Self::Graphql => Some(TreeIconColor::Pink),
            Self::Html => Some(TreeIconColor::Orange),
            Self::Image => Some(TreeIconColor::Pink),
            Self::Javascript => Some(TreeIconColor::Yellow),
            Self::Json => Some(TreeIconColor::Orange),
            Self::Markdown => Some(TreeIconColor::Green),
            Self::Mcp => Some(TreeIconColor::Teal),
            Self::Nextjs => None,
            Self::Npm => Some(TreeIconColor::Red),
            Self::Oxc => Some(TreeIconColor::Cyan),
            Self::Postcss => Some(TreeIconColor::Red),
            Self::Prettier => Some(TreeIconColor::Teal),
            Self::Python => Some(TreeIconColor::Blue),
            Self::React => Some(TreeIconColor::Cyan),
            Self::Ruby => Some(TreeIconColor::Red),
            Self::Rust => Some(TreeIconColor::Orange),
            Self::Sass => Some(TreeIconColor::Pink),
            Self::Stylelint => None,
            Self::Svelte => Some(TreeIconColor::Red),
            Self::Svg => Some(TreeIconColor::Orange),
            Self::Svgo => Some(TreeIconColor::Green),
            Self::Swift => Some(TreeIconColor::Orange),
            Self::Table => Some(TreeIconColor::Teal),
            Self::Tailwind => Some(TreeIconColor::Cyan),
            Self::Terraform => Some(TreeIconColor::Indigo),
            Self::Text => Some(TreeIconColor::Gray),
            Self::Typescript => Some(TreeIconColor::Blue),
            Self::Vite => Some(TreeIconColor::Purple),
            Self::Vscode => Some(TreeIconColor::Blue),
            Self::Vue => Some(TreeIconColor::Green),
            Self::Wasm => Some(TreeIconColor::Indigo),
            Self::Webpack => Some(TreeIconColor::Blue),
            Self::Yml => Some(TreeIconColor::Red),
            Self::Zig => Some(TreeIconColor::Orange),
            Self::Zip => Some(TreeIconColor::Orange),
        }
    }
}

/// Pierre's icon palette (`--trees-icon-*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeIconColor {
    Blue,
    Cyan,
    Gray,
    Green,
    Indigo,
    Mauve,
    Orange,
    Pink,
    Purple,
    Red,
    Teal,
    Vermilion,
    Yellow,
}

impl TreeIconColor {
    /// `(light, dark)` hex.
    pub fn hex(self) -> (&'static str, &'static str) {
        match self {
            Self::Blue => ("#1a85d4", "#69b1ff"),
            Self::Cyan => ("#1ca1c7", "#68cdf2"),
            Self::Gray => ("#84848a", "#adadb1"),
            Self::Green => ("#199f43", "#5ecc71"),
            Self::Indigo => ("#693acf", "#9d6afb"),
            Self::Mauve => ("#594c5b", "#79697b"),
            Self::Orange => ("#d47628", "#ffa359"),
            Self::Pink => ("#d32a61", "#ff678d"),
            Self::Purple => ("#a631be", "#d568ea"),
            Self::Red => ("#d52c36", "#ff6762"),
            Self::Teal => ("#17a5af", "#64d1db"),
            Self::Vermilion => ("#ff8c5b", "#d5512f"),
            Self::Yellow => ("#d5a910", "#ffd452"),
        }
    }
}

static FILE_NAME_TOKENS: &[(&str, BuiltInToken)] = &[
    (".babelrc", BuiltInToken::Babel),
    (".babelrc.json", BuiltInToken::Babel),
    (".bash_profile", BuiltInToken::Bash),
    (".bashrc", BuiltInToken::Bash),
    (".browserslistrc", BuiltInToken::Browserslist),
    (".dockerignore", BuiltInToken::Docker),
    (".eslintignore", BuiltInToken::Eslint),
    (".eslintrc", BuiltInToken::Eslint),
    (".eslintrc.cjs", BuiltInToken::Eslint),
    (".eslintrc.js", BuiltInToken::Eslint),
    (".eslintrc.json", BuiltInToken::Eslint),
    (".eslintrc.yaml", BuiltInToken::Eslint),
    (".eslintrc.yml", BuiltInToken::Eslint),
    (".gitattributes", BuiltInToken::Git),
    (".gitignore", BuiltInToken::Git),
    (".gitkeep", BuiltInToken::Git),
    (".gitmodules", BuiltInToken::Git),
    (".oxlintrc.json", BuiltInToken::Oxc),
    (".postcssrc", BuiltInToken::Postcss),
    (".postcssrc.json", BuiltInToken::Postcss),
    (".postcssrc.yaml", BuiltInToken::Postcss),
    (".postcssrc.yml", BuiltInToken::Postcss),
    (".prettierignore", BuiltInToken::Prettier),
    (".prettierrc", BuiltInToken::Prettier),
    (".prettierrc.cjs", BuiltInToken::Prettier),
    (".prettierrc.js", BuiltInToken::Prettier),
    (".prettierrc.json", BuiltInToken::Prettier),
    (".prettierrc.mjs", BuiltInToken::Prettier),
    (".prettierrc.toml", BuiltInToken::Prettier),
    (".prettierrc.yaml", BuiltInToken::Prettier),
    (".prettierrc.yml", BuiltInToken::Prettier),
    (".stylelintignore", BuiltInToken::Stylelint),
    (".stylelintrc", BuiltInToken::Stylelint),
    (".stylelintrc.cjs", BuiltInToken::Stylelint),
    (".stylelintrc.js", BuiltInToken::Stylelint),
    (".stylelintrc.json", BuiltInToken::Stylelint),
    (".stylelintrc.mjs", BuiltInToken::Stylelint),
    (".stylelintrc.yaml", BuiltInToken::Stylelint),
    (".stylelintrc.yml", BuiltInToken::Stylelint),
    (".terraform.lock.hcl", BuiltInToken::Terraform),
    (".zprofile", BuiltInToken::Bash),
    (".zshenv", BuiltInToken::Bash),
    (".zshrc", BuiltInToken::Bash),
    ("babel.config.cjs", BuiltInToken::Babel),
    ("babel.config.js", BuiltInToken::Babel),
    ("babel.config.json", BuiltInToken::Babel),
    ("babel.config.mjs", BuiltInToken::Babel),
    ("biome.json", BuiltInToken::Biome),
    ("biome.jsonc", BuiltInToken::Biome),
    ("bootstrap.bundle.js", BuiltInToken::Bootstrap),
    ("bootstrap.bundle.min.js", BuiltInToken::Bootstrap),
    ("bootstrap.css", BuiltInToken::Bootstrap),
    ("bootstrap.js", BuiltInToken::Bootstrap),
    ("bootstrap.min.css", BuiltInToken::Bootstrap),
    ("bootstrap.min.js", BuiltInToken::Bootstrap),
    ("bun.lock", BuiltInToken::Bun),
    ("bun.lockb", BuiltInToken::Bun),
    ("bunfig.toml", BuiltInToken::Bun),
    ("claude.md", BuiltInToken::Claude),
    ("compose.yaml", BuiltInToken::Docker),
    ("compose.yml", BuiltInToken::Docker),
    ("docker-compose.override.yml", BuiltInToken::Docker),
    ("docker-compose.yaml", BuiltInToken::Docker),
    ("docker-compose.yml", BuiltInToken::Docker),
    ("dockerfile", BuiltInToken::Docker),
    ("eslint.config.cjs", BuiltInToken::Eslint),
    ("eslint.config.js", BuiltInToken::Eslint),
    ("eslint.config.mjs", BuiltInToken::Eslint),
    ("eslint.config.mts", BuiltInToken::Eslint),
    ("eslint.config.ts", BuiltInToken::Eslint),
    ("gemfile", BuiltInToken::Ruby),
    ("next.config.js", BuiltInToken::Nextjs),
    ("next.config.mjs", BuiltInToken::Nextjs),
    ("next.config.mts", BuiltInToken::Nextjs),
    ("next.config.ts", BuiltInToken::Nextjs),
    ("postcss.config.cjs", BuiltInToken::Postcss),
    ("postcss.config.js", BuiltInToken::Postcss),
    ("postcss.config.mjs", BuiltInToken::Postcss),
    ("postcss.config.ts", BuiltInToken::Postcss),
    ("prettier.config.cjs", BuiltInToken::Prettier),
    ("prettier.config.js", BuiltInToken::Prettier),
    ("prettier.config.mjs", BuiltInToken::Prettier),
    ("rakefile", BuiltInToken::Ruby),
    ("readme.md", BuiltInToken::Markdown),
    ("stylelint.config.cjs", BuiltInToken::Stylelint),
    ("stylelint.config.js", BuiltInToken::Stylelint),
    ("stylelint.config.mjs", BuiltInToken::Stylelint),
    ("svgo.config.cjs", BuiltInToken::Svgo),
    ("svgo.config.js", BuiltInToken::Svgo),
    ("svgo.config.mjs", BuiltInToken::Svgo),
    ("svgo.config.ts", BuiltInToken::Svgo),
    ("tailwind.config.cjs", BuiltInToken::Tailwind),
    ("tailwind.config.js", BuiltInToken::Tailwind),
    ("tailwind.config.mjs", BuiltInToken::Tailwind),
    ("tailwind.config.ts", BuiltInToken::Tailwind),
    ("vite.config.js", BuiltInToken::Vite),
    ("vite.config.mjs", BuiltInToken::Vite),
    ("vite.config.mts", BuiltInToken::Vite),
    ("vite.config.ts", BuiltInToken::Vite),
    ("webpack.config.babel.js", BuiltInToken::Webpack),
    ("webpack.config.cjs", BuiltInToken::Webpack),
    ("webpack.config.js", BuiltInToken::Webpack),
    ("webpack.config.mjs", BuiltInToken::Webpack),
    ("webpack.config.ts", BuiltInToken::Webpack),
];

static EXTENSION_TOKENS: &[(&str, BuiltInToken)] = &[
    ("7z", BuiltInToken::Zip),
    ("AUTHORS", BuiltInToken::Text),
    ("CHANGELOG", BuiltInToken::Text),
    ("CONTRIBUTORS", BuiltInToken::Text),
    ("LICENSE", BuiltInToken::Text),
    ("astro", BuiltInToken::Astro),
    ("avif", BuiltInToken::Image),
    ("bash", BuiltInToken::Bash),
    ("bmp", BuiltInToken::Image),
    ("bz2", BuiltInToken::Zip),
    ("c", BuiltInToken::C),
    ("cc", BuiltInToken::Cpp),
    ("cfg", BuiltInToken::Text),
    ("cjs", BuiltInToken::Javascript),
    ("code-workspace", BuiltInToken::Vscode),
    ("conf", BuiltInToken::Text),
    ("cpp", BuiltInToken::Cpp),
    ("csh", BuiltInToken::Bash),
    ("css", BuiltInToken::Css),
    ("csv", BuiltInToken::Table),
    ("cts", BuiltInToken::Typescript),
    ("cxx", BuiltInToken::Cpp),
    ("db", BuiltInToken::Database),
    ("editorconfig", BuiltInToken::Text),
    ("env", BuiltInToken::Text),
    ("env.development", BuiltInToken::Text),
    ("env.local", BuiltInToken::Text),
    ("env.production", BuiltInToken::Text),
    ("eot", BuiltInToken::Font),
    ("erb", BuiltInToken::Ruby),
    ("fish", BuiltInToken::Bash),
    ("gemspec", BuiltInToken::Ruby),
    ("gif", BuiltInToken::Image),
    ("go", BuiltInToken::Go),
    ("gql", BuiltInToken::Graphql),
    ("graphql", BuiltInToken::Graphql),
    ("gz", BuiltInToken::Zip),
    ("h", BuiltInToken::C),
    ("hh", BuiltInToken::Cpp),
    ("hpp", BuiltInToken::Cpp),
    ("htm", BuiltInToken::Html),
    ("html", BuiltInToken::Html),
    ("hxx", BuiltInToken::Cpp),
    ("icns", BuiltInToken::Image),
    ("ico", BuiltInToken::Image),
    ("ini", BuiltInToken::Text),
    ("inl", BuiltInToken::Cpp),
    ("jar", BuiltInToken::Zip),
    ("jpeg", BuiltInToken::Image),
    ("jpg", BuiltInToken::Image),
    ("js", BuiltInToken::Javascript),
    ("json", BuiltInToken::Json),
    ("json5", BuiltInToken::Json),
    ("jsonc", BuiltInToken::Json),
    ("jsonl", BuiltInToken::Json),
    ("jsx", BuiltInToken::Javascript),
    ("ksh", BuiltInToken::Bash),
    ("less", BuiltInToken::Css),
    ("log", BuiltInToken::Text),
    ("markdown", BuiltInToken::Markdown),
    ("mcp", BuiltInToken::Mcp),
    ("md", BuiltInToken::Markdown),
    ("mdx", BuiltInToken::Markdown),
    ("mdx.tsx", BuiltInToken::Markdown),
    ("mjs", BuiltInToken::Javascript),
    ("mm", BuiltInToken::Cpp),
    ("mts", BuiltInToken::Typescript),
    ("ods", BuiltInToken::Table),
    ("otf", BuiltInToken::Font),
    ("png", BuiltInToken::Image),
    ("postcss", BuiltInToken::Css),
    ("py", BuiltInToken::Python),
    ("pyi", BuiltInToken::Python),
    ("pyw", BuiltInToken::Python),
    ("pyx", BuiltInToken::Python),
    ("rake", BuiltInToken::Ruby),
    ("rar", BuiltInToken::Zip),
    ("rb", BuiltInToken::Ruby),
    ("rs", BuiltInToken::Rust),
    ("rst", BuiltInToken::Text),
    ("rtf", BuiltInToken::Text),
    ("sass", BuiltInToken::Css),
    ("scss", BuiltInToken::Css),
    ("sh", BuiltInToken::Bash),
    ("sql", BuiltInToken::Database),
    ("sqlite", BuiltInToken::Database),
    ("sqlite3", BuiltInToken::Database),
    ("styl", BuiltInToken::Css),
    ("svelte", BuiltInToken::Svelte),
    ("svg", BuiltInToken::Svg),
    ("swift", BuiltInToken::Swift),
    ("tar", BuiltInToken::Zip),
    ("tf", BuiltInToken::Terraform),
    ("tfstate", BuiltInToken::Terraform),
    ("tfvars", BuiltInToken::Terraform),
    ("tgz", BuiltInToken::Zip),
    ("tif", BuiltInToken::Image),
    ("tiff", BuiltInToken::Image),
    ("ts", BuiltInToken::Typescript),
    ("tsv", BuiltInToken::Table),
    ("tsx", BuiltInToken::Typescript),
    ("ttf", BuiltInToken::Font),
    ("txt", BuiltInToken::Text),
    ("vue", BuiltInToken::Vue),
    ("war", BuiltInToken::Zip),
    ("wasm", BuiltInToken::Wasm),
    ("wast", BuiltInToken::Wasm),
    ("wat", BuiltInToken::Wasm),
    ("webp", BuiltInToken::Image),
    ("woff", BuiltInToken::Font),
    ("woff2", BuiltInToken::Font),
    ("xhtml", BuiltInToken::Html),
    ("xls", BuiltInToken::Table),
    ("xlsx", BuiltInToken::Table),
    ("xz", BuiltInToken::Zip),
    ("yaml", BuiltInToken::Yml),
    ("yml", BuiltInToken::Yml),
    ("zig", BuiltInToken::Zig),
    ("zip", BuiltInToken::Zip),
    ("zsh", BuiltInToken::Bash),
];

static COMPLETE_EXTENSION_OVERRIDES: &[(&str, BuiltInToken)] = &[
    ("jsx", BuiltInToken::React),
    ("sass", BuiltInToken::Sass),
    ("scss", BuiltInToken::Sass),
    ("tsx", BuiltInToken::React),
];

static ALL_TOKENS: &[BuiltInToken] = &[
    BuiltInToken::Astro,
    BuiltInToken::Babel,
    BuiltInToken::Bash,
    BuiltInToken::Biome,
    BuiltInToken::Bootstrap,
    BuiltInToken::Browserslist,
    BuiltInToken::Bun,
    BuiltInToken::C,
    BuiltInToken::Claude,
    BuiltInToken::Cpp,
    BuiltInToken::Css,
    BuiltInToken::Database,
    BuiltInToken::Default,
    BuiltInToken::Docker,
    BuiltInToken::Eslint,
    BuiltInToken::Font,
    BuiltInToken::Git,
    BuiltInToken::Go,
    BuiltInToken::Graphql,
    BuiltInToken::Html,
    BuiltInToken::Image,
    BuiltInToken::Javascript,
    BuiltInToken::Json,
    BuiltInToken::Markdown,
    BuiltInToken::Mcp,
    BuiltInToken::Nextjs,
    BuiltInToken::Npm,
    BuiltInToken::Oxc,
    BuiltInToken::Postcss,
    BuiltInToken::Prettier,
    BuiltInToken::Python,
    BuiltInToken::React,
    BuiltInToken::Ruby,
    BuiltInToken::Rust,
    BuiltInToken::Sass,
    BuiltInToken::Stylelint,
    BuiltInToken::Svelte,
    BuiltInToken::Svg,
    BuiltInToken::Svgo,
    BuiltInToken::Swift,
    BuiltInToken::Table,
    BuiltInToken::Tailwind,
    BuiltInToken::Terraform,
    BuiltInToken::Text,
    BuiltInToken::Typescript,
    BuiltInToken::Vite,
    BuiltInToken::Vscode,
    BuiltInToken::Vue,
    BuiltInToken::Wasm,
    BuiltInToken::Webpack,
    BuiltInToken::Yml,
    BuiltInToken::Zig,
    BuiltInToken::Zip,
];

