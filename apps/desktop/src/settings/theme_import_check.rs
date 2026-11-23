//! Runs the Rust theme importers over a folder of theme files and writes one
//! JSON result per file, in the same shape as the TypeScript comparison run,
//! so the two outputs can be diffed. Usage:
//! `theme-import-check <vscode|openvsx> <in-dir> <out-dir>`.

use std::path::PathBuf;

use serde_json::{Value, json};
use t3code_web::vscode_theme_import::parse_theme_import;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [kind, input, output] = args.as_slice() else {
        anyhow::bail!("usage: theme-import-check <vscode|openvsx> <in-dir> <out-dir>");
    };
    let output = PathBuf::from(output);
    std::fs::create_dir_all(&output)?;
    let mut count = 0;
    for entry in std::fs::read_dir(input)? {
        let path = entry?.path();
        let source = std::fs::read_to_string(&path)?;
        let result: Value = match kind.as_str() {
            "vscode" => match parse_theme_import(&source) {
                Ok(theme) => {
                    let mut file = theme.to_file();
                    let map = file.as_object_mut().expect("object");
                    map.remove("version");
                    file
                }
                Err(error) => json!({ "error": error.0 }),
            },
            "openvsx" => t3code_web::open_vsx_themes::check_fixture(&source),
            other => anyhow::bail!("unknown kind {other}"),
        };
        std::fs::write(output.join(path.file_name().expect("file name")), serde_json::to_vec_pretty(&result)?)?;
        count += 1;
    }
    println!("wrote {count} results");
    Ok(())
}
