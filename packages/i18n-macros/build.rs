//! Deflates every non-English catalog in `apps/web/assets/i18n/{web,desktop}` into
//! `OUT_DIR`, so `embedded_catalogs!()` can `include_bytes!` them. English is the
//! source text and is never embedded.

use std::{fs, io::Write, path::Path};

use flate2::{Compression, write::DeflateEncoder};

fn main() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/assets/i18n");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("i18n");
    println!("cargo:rerun-if-changed={}", assets.display());
    for app in ["web", "desktop"] {
        let dir = assets.join(app);
        println!("cargo:rerun-if-changed={}", dir.display());
        fs::create_dir_all(out.join(app)).unwrap();
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let Some(locale) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            if path.extension().is_none_or(|e| e != "json") || locale == "en" {
                continue;
            }
            println!("cargo:rerun-if-changed={}", path.display());
            let json = fs::read(&path).unwrap();
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
            encoder.write_all(&json).unwrap();
            fs::write(out.join(app).join(format!("{locale}.json.deflate")), encoder.finish().unwrap()).unwrap();
        }
    }
}
