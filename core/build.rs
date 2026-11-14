// Copies the web UI that trunk built (frontend/dist) into OUT_DIR/web, where the boss embeds it,
// with a gzip (.gz) copy of each file beside it, compressed once here at its smallest rather than
// on every request. A copy that saves little (woff2 and png are compressed already) is left out, so
// the original is what gets sent. So is an SVG's: Chrome fetches a compressed SVG image again on
// the next load, and the marks are a few hundred bytes anyway.
//
// Builds the vm tools the binary carries too (tools.sh), so every build of it has them; tools.sh
// does nothing while target/tools/<arch>/ is from this version of it.

use std::{
    error::Error,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

fn main() -> Result<(), Box<dyn Error>> {
    let dist = Path::new("../frontend/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    let web = PathBuf::from(std::env::var("OUT_DIR")?).join("web");
    if web.exists() {
        fs::remove_dir_all(&web)?;
    }
    fs::create_dir_all(&web)?;
    // A checkout that has not built the frontend yet still builds; its boss serves no UI.
    if dist.is_dir() {
        copy(dist, &web)?;
    }

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH")?;
    println!("cargo:rerun-if-changed=../tools.sh");
    println!("cargo:rerun-if-changed=../target/tools/{arch}.stamp");
    // Its progress to stderr: cargo reads a build script's stdout as instructions.
    let tools = Command::new("../tools.sh")
        .arg(&arch)
        .stdout(io::stderr())
        .status()?;
    if !tools.success() {
        return Err(format!("tools.sh {arch} failed").into());
    }
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(from)? {
        let path = entry?.path();
        let target = to.join(path.file_name().ok_or("a file without a name")?);
        if path.is_dir() {
            fs::create_dir_all(&target)?;
            copy(&path, &target)?;
            continue;
        }
        let data = fs::read(&path)?;
        fs::write(&target, &data)?;
        if path.extension().is_some_and(|extension| extension == "svg") {
            continue;
        }
        let mut gzip = flate2::write::GzEncoder::new(vec![], flate2::Compression::best());
        gzip.write_all(&data)?;
        let gzip = gzip.finish()?;
        if gzip.len() * 10 < data.len() * 9 {
            let mut name = target.into_os_string();
            name.push(".gz");
            fs::write(name, gzip)?;
        }
    }
    Ok(())
}
