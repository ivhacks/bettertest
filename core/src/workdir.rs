// Everything bettertest keeps on disk lives under one directory, shared by every unix user on the
// machine. Intern 1 runs bettertest, then intern 2 logs in as a different user and runs the same
// commands, so directories we create are 0777 and files 0666. umask would quietly turn those into
// 0755/0644 and lock intern 2 out, so permissions are set explicitly right after creating. Only
// the creator can chmod, which is fine: the creator is the only one who ever needs to.

use std::{
    fs::{self, File, Permissions},
    io::{self, ErrorKind},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::OnceLock,
};

// /tmp is tmpfs on fedora and empties on reboot. /var/tmp survives reboot and anyone can write it.
const ROOT: &str = "/var/tmp/bettertest";

// `--workdir`, when it was given. Set once, before anything reads or writes, so the db and the
// files around it cannot land in two places.
static CHOSEN: OnceLock<PathBuf> = OnceLock::new();

/// Uses `path` instead of `/var/tmp/bettertest`. One call, from `--workdir`. A second call is an
/// error: the first directory may already hold the db.
pub fn choose(path: PathBuf) -> io::Result<()> {
    CHOSEN
        .set(path)
        .map_err(|_| io::Error::other("workdir is already set"))
}

/// `/var/tmp/bettertest/<name>`, created if it does not exist yet.
pub fn dir(name: &str) -> io::Result<PathBuf> {
    let path = root()?.join(name);
    shared_dir(&path)?;
    Ok(path)
}

/// `/var/tmp/bettertest`, or the directory `--workdir` named.
pub fn root() -> io::Result<PathBuf> {
    let path = CHOSEN.get_or_init(|| PathBuf::from(ROOT));
    shared_dir(path)?;
    Ok(path.clone())
}

pub fn shared_dir(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => fs::set_permissions(path, Permissions::from_mode(0o777)),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Creates `path` as an empty 0666 file. Fails if it already exists.
pub fn new_shared_file(path: &Path) -> io::Result<File> {
    let file = File::create_new(path)?;
    file.set_permissions(Permissions::from_mode(0o666))?;
    Ok(file)
}

/// Like `new_shared_file`, but an existing file is fine and left alone.
pub fn shared_file(path: &Path) -> io::Result<()> {
    match new_shared_file(path) {
        Err(e) if e.kind() != ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}
