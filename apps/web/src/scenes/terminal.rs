//! The terminal drawer with recorded output: ANSI 16/256/truecolor, SGR
//! styles, wide characters and emoji, links, and a full-screen program on the
//! alternate screen in a split beside it.

use std::rc::Rc;

use gpui_kit::*;

use crate::components::persistent_thread_terminals::PersistentThreadTerminals;
use crate::terminal::GridSize;
use crate::terminal::backend::RecordedBackend;
use crate::terminal_ui_state_store::{SplitDirection, TerminalUiStateStore, ThreadKey};
use crate::theme_palette::{ActiveAppTheme as _, Appearance};

/// Output that exercises colours, styles and wide characters.
pub fn showcase_output() -> Vec<u8> {
    let mut out = String::new();
    let prompt = "\x1b[1;32muser@host\x1b[0m:\x1b[1;34m~/project\x1b[0m$ ";
    out += &format!("{prompt}ls --color\r\n");
    out += "\x1b[1;34mapps\x1b[0m  \x1b[1;34mpackages\x1b[0m  Cargo.toml  \x1b[1;32mbuild.sh\x1b[0m  \x1b[1;36mlink\x1b[0m  \x1b[1;31marchive.tgz\x1b[0m  README.md\r\n";
    out += &format!("{prompt}./palette\r\n");
    out += "16:  ";
    for index in 0..8 {
        out += &format!("\x1b[4{index}m  \x1b[0m");
    }
    out += " ";
    for index in 0..8 {
        out += &format!("\x1b[10{index}m  \x1b[0m");
    }
    out += "  ";
    for index in 0..8 {
        out += &format!("\x1b[3{index}m■\x1b[0m\x1b[9{index}m■\x1b[0m");
    }
    out += "\r\n256: ";
    for index in (16..232).step_by(4) {
        out += &format!("\x1b[48;5;{index}m \x1b[0m");
    }
    for index in 232..256 {
        out += &format!("\x1b[48;5;{index}m \x1b[0m");
    }
    out += "\r\nrgb: ";
    for step in 0..72 {
        let t = step as f32 / 71.;
        let (r, g, b) = ((255. * (1. - t)) as u8, (120. + 100. * t) as u8, (255. * t) as u8);
        out += &format!("\x1b[48;2;{r};{g};{b}m \x1b[0m");
    }
    out += "\r\n";
    out += "\x1b[1mbold\x1b[0m \x1b[2mdim\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munderline\x1b[0m \x1b[4:3mundercurl\x1b[0m \x1b[9mstrikethrough\x1b[0m \x1b[7minverse\x1b[0m \x1b[8mhidden\x1b[0m \x1b[1;3;4;38;2;255;135;0mbold italic orange\x1b[0m\r\n";
    out += "wide: 日本語のテキスト 中文 한국어 | emoji: 🚀 ✅ 🎉 👍🏽 | combining: e\u{301} n\u{303}\r\n";
    out += "box:  ┌──────┬──────┐ ▁▂▃▄▅▆▇█ ░▒▓\r\n      └──────┴──────┘ \x1b[32m✔ done\x1b[0m \x1b[31m✘ failed\x1b[0m\r\n";
    out += "links: https://t3.codes/docs and src/terminal/core.rs:42:7 \x1b]8;;https://example.com\x1b\\an OSC 8 link\x1b]8;;\x1b\\\r\n";
    out += &format!("{prompt}cargo test\r\n");
    out += "   \x1b[1;32mCompiling\x1b[0m t3code-web v0.1.11\r\n";
    out += "    \x1b[1;32mFinished\x1b[0m `test` profile [unoptimized] target(s) in 4.21s\r\n";
    out += "test result: \x1b[32mok\x1b[0m. 42 passed; \x1b[31m0 failed\x1b[0m; 0 ignored\r\n";
    out += prompt;
    out.into_bytes()
}

/// A top-like full-screen program drawn for `size` on the alternate screen.
pub fn full_screen_output(size: GridSize) -> Vec<u8> {
    let cols = size.cols as usize;
    let rows = size.rows as usize;
    let mut out = String::from("\x1b[?1049h\x1b[?25l\x1b[H\x1b[2J");
    let bar = |label: &str, fill: f32, width: usize| -> String {
        let inner = width.saturating_sub(label.len() + 3);
        let used = (inner as f32 * fill) as usize;
        let green = used * 2 / 3;
        format!(
            "\x1b[1;36m{label}\x1b[0m\x1b[1m[\x1b[0m\x1b[32m{}\x1b[31m{}\x1b[0m{}\x1b[1m]\x1b[0m",
            "|".repeat(green),
            "|".repeat(used - green),
            " ".repeat(inner - used)
        )
    };
    let half = cols / 2;
    let fills = [0.62, 0.35, 0.88, 0.14];
    for (index, fill) in fills.iter().enumerate() {
        out += &format!("\x1b[{};1H {}", index + 1, bar(&format!("{index}"), *fill, half - 2));
    }
    out += &format!("\x1b[1;{}H\x1b[1mTasks:\x1b[0m \x1b[1;32m184\x1b[0m, 612 thr; \x1b[1;32m3\x1b[0m running", half + 2);
    out += &format!("\x1b[2;{}H\x1b[1mLoad average:\x1b[0m \x1b[1m1.42\x1b[0m 1.18 \x1b[2m0.97\x1b[0m", half + 2);
    out += &format!("\x1b[3;{}H\x1b[1mUptime:\x1b[0m \x1b[1m3 days, 04:12:55\x1b[0m", half + 2);
    out += &format!("\x1b[5;1H {}", bar("Mem", 0.54, half - 2));
    out += &format!("\x1b[6;1H {}", bar("Swp", 0.05, half - 2));
    let mut header = format!("{:>7} {:<9} {:>3} {:>6} {:>6} {:>5} {:>5} {:>9}  Command", "PID", "USER", "PRI", "VIRT", "RES", "CPU%", "MEM%", "TIME+");
    header.truncate(cols);
    out += &format!("\x1b[8;1H\x1b[30;42m{header:<cols$}\x1b[0m");
    let processes = [
        (4211, "user", 20, "1.2G", "312M", 48.3, 3.9, "1:02.44", "t3code"),
        (1872, "user", 20, "820M", "141M", 12.0, 1.8, "0:21.10", "node server.js"),
        (931, "root", 20, "64M", "12M", 3.1, 0.1, "4:44.01", "/usr/lib/systemd/systemd-journald"),
        (4302, "user", 20, "38M", "9M", 1.4, 0.1, "0:00.31", "zsh"),
        (5120, "user", 20, "211M", "54M", 0.7, 0.6, "0:03.92", "cargo build"),
        (77, "root", 0, "0", "0", 0.0, 0.0, "0:12.00", "kworker/u16:2"),
    ];
    for (index, (pid, user, pri, virt, res, cpu, mem, time, command)) in processes.iter().enumerate() {
        let row = 9 + index;
        if row >= rows {
            break;
        }
        let selected = index == 0;
        let mut line = format!("{pid:>7} {user:<9} {pri:>3} {virt:>6} {res:>6} {cpu:>5.1} {mem:>5.1} {time:>9}  {command}");
        line.truncate(cols);
        let command = &line[line.len().min(59)..];
        if selected {
            out += &format!("\x1b[{row};1H\x1b[30;46m{line:<cols$}\x1b[0m");
        } else {
            let user_color = if *user == "root" { "\x1b[2m" } else { "" };
            out += &format!(
                "\x1b[{row};1H{pid:>7} {user_color}{user:<9}\x1b[0m {pri:>3} {virt:>6} \x1b[32m{res:>6}\x1b[0m {cpu:>5.1} {mem:>5.1} \x1b[36m{time:>9}\x1b[0m  \x1b[1m{command}\x1b[0m\x1b[K"
            );
        }
    }
    let keys = [("F1", "Help"), ("F2", "Setup"), ("F3", "Search"), ("F4", "Filter"), ("F5", "Tree"), ("F9", "Kill"), ("F10", "Quit")];
    let mut footer = String::new();
    let mut width = 0;
    for (key, label) in keys {
        footer += &format!("\x1b[0m{key}\x1b[30;46m{label:<6}");
        width += key.len() + 6;
    }
    out += &format!("\x1b[{rows};1H{footer}{}\x1b[0m", " ".repeat(cols.saturating_sub(width)));
    out.into_bytes()
}

fn build_scene(appearance: Appearance, cx: &mut App) -> AnyView {
    crate::theme_palette::ThemeSettings::set_appearance(appearance, cx);
    let thread = ThreadKey("environment-1:thread-1".into());
    cx.update_global::<TerminalUiStateStore, _>(|store, _| {
        store.new_terminal(&thread, "term-1");
        store.split_terminal(&thread, "term-2", SplitDirection::Horizontal);
        store.new_terminal(&thread, "term-3");
        store.set_active_terminal(&thread, "term-1");
        store.set_terminal_height(&thread, 470.);
    });
    let factory = Rc::new(|launch: &crate::components::thread_terminal_drawer::TerminalLaunch, _: &mut App| {
        let backend: Box<dyn crate::terminal::TerminalBackend> = match launch.terminal_id.as_ref() {
            "term-1" => Box::new(RecordedBackend::new(vec![showcase_output()])),
            "term-2" => Box::new(RecordedBackend::sized(|size| vec![full_screen_output(size)])),
            _ => Box::new(RecordedBackend::new(vec![b"user@host:~/project$ ".to_vec()])),
        };
        backend
    });
    let terminals = cx.new(|cx| {
        let mut terminals = PersistentThreadTerminals::new(factory, cx);
        terminals.set_active_thread(Some((thread.clone(), "/home/user/project".into())), cx);
        terminals
    });
    cx.new(|_| TerminalScene { terminals }).into()
}

struct TerminalScene {
    terminals: Entity<PersistentThreadTerminals>,
}

impl Render for TerminalScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.canvas)
            .text_color(colors.text)
            .child(div().flex_1().min_h_0())
            .child(self.terminals.clone())
    }
}

pub fn build(_: &mut Window, cx: &mut App) -> AnyView {
    build_scene(Appearance::Dark, cx)
}

pub fn build_light(_: &mut Window, cx: &mut App) -> AnyView {
    build_scene(Appearance::Light, cx)
}
