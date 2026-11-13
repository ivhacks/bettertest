// The programs vms need (qemu, its firmware, debugfs, e2fsck, ssh), built static by tools.sh and
// carried in this binary, so a worker needs none of them installed. They are written out the first
// time they are needed, once per version of them, into a directory named after their contents.

use crate::workdir;
use openssl::sha::Sha256;
use rust_embed::Embed;
use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Embed)]
#[cfg_attr(target_arch = "x86_64", folder = "../target/tools/x86_64/")]
#[cfg_attr(target_arch = "aarch64", folder = "../target/tools/aarch64/")]
struct Tools;

/// Where the tool `name` is, written out from this binary if it is not there yet.
pub fn path(name: &str) -> Result<PathBuf, String> {
    static DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    let dir =
        DIR.get_or_init(|| unpack().map_err(|e| format!("could not unpack the vm tools: {e}")));
    dir.clone().map(|dir| dir.join(name))
}

fn unpack() -> io::Result<PathBuf> {
    let mut hash = Sha256::new();
    for name in Tools::iter() {
        hash.update(name.as_bytes());
        if let Some(file) = Tools::get(&name) {
            hash.update(&file.metadata.sha256_hash());
        }
    }
    let version: String = hash.finish()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let tools = workdir::dir("tools")?;
    let dir = tools.join(&version);
    if !dir.exists() {
        // Written beside it and renamed, so a half-written directory is never taken for a whole
        // one. Of two workers starting at once, the second finds the first one's there.
        let partial = tools.join(format!(".{}", uuid::Uuid::new_v4().simple()));
        let written = write_all(&partial).and_then(|()| fs::rename(&partial, &dir));
        if written.is_err() {
            let _ = fs::remove_dir_all(&partial);
            if !dir.exists() {
                written?;
            }
        }
    }
    // Older versions' tools. Their vms that are still running keep going; qemu has its file open.
    for entry in fs::read_dir(&tools)?.flatten() {
        let name = entry.file_name();
        if name != *version && !name.to_string_lossy().starts_with('.') {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(dir)
}

fn write_all(dir: &Path) -> io::Result<()> {
    workdir::shared_dir(dir)?;
    for name in Tools::iter() {
        let Some(file) = Tools::get(&name) else {
            continue;
        };
        let mut out = fs::File::create_new(dir.join(&*name))?;
        out.set_permissions(fs::Permissions::from_mode(0o755))?;
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(&*file.data)
            .map_err(|e| io::Error::other(format!("{name}: {e}")))?;
        io::copy(&mut decoder, &mut out)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../test/tools.rs"]
mod tests;
