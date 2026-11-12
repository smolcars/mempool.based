//! Compiles every `.sql` file in `src/store/migrations` into the binary, in name order, so a
//! migration is added by adding a file. `store::migrate` checks the names and the order.

use std::{env, fs, io, path::Path};

fn main() -> io::Result<()> {
    let dir = Path::new("src/store/migrations");
    println!("cargo::rerun-if-changed={}", dir.display());
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.ends_with(".sql") {
            names.push(name);
        }
    }
    names.sort();
    let mut out = String::from("&[\n");
    for name in names {
        out.push_str(&format!(
            "    ({name:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/store/migrations/\", {name:?}))),\n"
        ));
    }
    out.push_str("]\n");
    let out_dir = env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("OUT_DIR is not set"))?;
    fs::write(Path::new(&out_dir).join("migrations.rs"), out)
}
