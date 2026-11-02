// 派生自 cc-same crates/core/src/fsx.rs（v0.1.12, MIT, Songkeys）
//! Careful file-system primitives: never follow symlinks, write atomically, keep mtimes,
//! clone instead of copy where the file system allows it.
//!
//! AuthManager: macOS only. Temporary files are made here (cc-same used the tempfile crate) and
//! clones go through `clonefile` (cc-same used reflink-copy).

use std::collections::hash_map::RandomState;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Prefix of our temporary files. Desktop ignores anything not named `local_*`/`deleted_*`.
pub const TMP_PREFIX: &str = ".authmanager-tmp-";

pub fn to_ns(t: SystemTime) -> i128 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

pub fn from_ns(ns: i128) -> SystemTime {
    if ns >= 0 {
        UNIX_EPOCH + Duration::from_nanos(ns as u64)
    } else {
        UNIX_EPOCH - Duration::from_nanos(ns.unsigned_abs() as u64)
    }
}

pub fn mtime_ns(md: &fs::Metadata) -> i128 {
    md.modified().map(to_ns).unwrap_or(0)
}

pub fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn other(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

pub fn nlink(md: &fs::Metadata) -> u64 {
    md.nlink()
}

pub fn mode(md: &fs::Metadata) -> u32 {
    md.permissions().mode() & 0o777
}

fn open_nofollow(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)
}

/// Read a regular file without following a symlink at the leaf, up to `limit` bytes.
pub fn read_limited(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let f = open_nofollow(path)?;
    let len = f.metadata()?.len();
    if len > limit {
        return Err(other("file too large"));
    }
    let mut buf = Vec::with_capacity(len as usize + 1);
    f.take(limit + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > limit {
        return Err(other("file too large"));
    }
    Ok(buf)
}

pub fn read_json(path: &Path, limit: u64) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::from_slice(&read_limited(path, limit)?)?)
}

pub fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

fn set_mtime(file: &File, mtime_ns: i128) -> io::Result<()> {
    file.set_modified(from_ns(mtime_ns))
}

fn set_private(file: &File) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| other(format!("bad path: {}", path.display())))
}

/// A temporary file next to where it will go, removed again unless it is moved into place.
struct TempFile {
    path: PathBuf,
    file: File,
    keep: bool,
}

impl TempFile {
    /// `<dir>/.authmanager-tmp-<random>`, created new (never an existing file or a symlink), 0600.
    fn create_in(dir: &Path) -> io::Result<TempFile> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seed = RandomState::new();
        for _ in 0..64 {
            let mut h = seed.build_hasher();
            h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
            h.write_u32(std::process::id());
            h.write_u128(to_ns(SystemTime::now()) as u128);
            let path = dir.join(format!("{TMP_PREFIX}{:016x}", h.finish()));
            match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
                Ok(file) => return Ok(TempFile { path, file, keep: false }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(other(format!("cannot create a temporary file in {}", dir.display())))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Rename `from` to `to` unless `to` exists (`AlreadyExists` then).
fn rename_no_clobber(from: &Path, to: &Path) -> io::Result<()> {
    let (f, t) = (c_path(from)?, c_path(to)?);
    if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        // A file system without exclusive renames: a hard link fails the same way on an existing
        // name, and the temporary name goes after.
        Some(libc::ENOTSUP) | Some(libc::EINVAL) => {
            fs::hard_link(from, to)?;
            let _ = fs::remove_file(from);
            Ok(())
        }
        _ => Err(e),
    }
}

/// Write `payload` to `path` through a temporary file in the same folder, fsync it, give it
/// `mtime_ns`, then rename it into place. With `no_clobber` an existing file is left alone and
/// `Ok(false)` is returned. Never writes through a symlink.
pub fn atomic_write(path: &Path, payload: &[u8], mtime_ns: Option<i128>, no_clobber: bool) -> io::Result<bool> {
    if is_symlink(path) {
        return Err(other(format!("refusing to write through a symlink: {}", path.display())));
    }
    let dir = path.parent().ok_or_else(|| other("no parent directory"))?;
    let mut tmp = TempFile::create_in(dir)?;
    tmp.file.write_all(payload)?;
    tmp.file.sync_all()?;
    set_private(&tmp.file)?;
    if let Some(ns) = mtime_ns {
        set_mtime(&tmp.file, ns)?;
    }
    if no_clobber {
        match rename_no_clobber(&tmp.path, path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
            Err(e) => return Err(e),
        }
    } else {
        fs::rename(&tmp.path, path)?;
    }
    tmp.keep = true;
    Ok(true)
}

fn mkdir_private(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)
}

pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    match mkdir_private(path) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

/// `mkdir -p` below `root` that refuses symlinked or non-directory components.
pub fn ensure_real_dir(path: &Path, root: &Path) -> io::Result<()> {
    let rel = path.strip_prefix(root).map_err(|_| other(format!("path escapes its root: {}", path.display())))?;
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        cur.push(c);
        match fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_symlink() || !md.is_dir() => {
                return Err(other(format!("not a real directory: {}", cur.display())));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => mkdir_private(&cur)?,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `clonefile`'s `CLONE_NOFOLLOW` (`<sys/clonefile.h>`).
const CLONE_NOFOLLOW: u32 = 0x0001;

/// A copy-on-write clone (APFS), or a plain copy where the file system cannot clone. Never
/// replaces an existing `dst`.
fn clone_or_copy(src: &Path, dst: &Path) -> io::Result<()> {
    let (s, d) = (c_path(src)?, c_path(dst)?);
    if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), CLONE_NOFOLLOW) } == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if e.kind() == io::ErrorKind::AlreadyExists {
        return Err(e);
    }
    fs::copy(src, dst).map(|_| ())
}

/// Copy-on-write clone (APFS) or a plain copy, then restore mode and mtime.
pub fn clone_file(src: &Path, dst: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(src)?;
    if !md.is_file() {
        return Err(other(format!("not a regular file: {}", src.display())));
    }
    clone_or_copy(src, dst)?;
    let f = OpenOptions::new().write(true).open(dst)?;
    f.set_permissions(fs::Permissions::from_mode(mode(&md)))?;
    set_mtime(&f, mtime_ns(&md))
}

/// Recursively clone `src` into `dst` (which must not exist). Symlinks inside are recreated
/// as symlinks; they are never followed.
pub fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    mkdir_private(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ft.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(entry.path())?, &to)?;
        } else if ft.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if ft.is_file() {
            clone_file(&entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Move a file or folder, falling back to copy + delete across volumes.
pub fn move_path(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        create_private_dir_all(parent)?;
    }
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let md = fs::symlink_metadata(src)?;
            if md.is_dir() {
                copy_tree(src, dst)?;
                fs::remove_dir_all(src)
            } else {
                clone_file(src, dst)?;
                fs::remove_file(src)
            }
        }
        Err(e) => Err(e),
    }
}

/// A path that does not exist yet: `base`, `base.1`, `base.2`, …
pub fn unique_path(base: PathBuf) -> PathBuf {
    if fs::symlink_metadata(&base).is_err() {
        return base;
    }
    for n in 1.. {
        let mut s = base.clone().into_os_string();
        s.push(format!(".{n}"));
        let candidate = PathBuf::from(s);
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_are_private_and_no_clobber_leaves_a_file_alone() {
        let tmp = crate::testing::tempdir().unwrap();
        let path = tmp.path().join("a.json");
        assert!(atomic_write(&path, b"one", Some(1_000_000_000), false).unwrap());
        let md = fs::metadata(&path).unwrap();
        assert_eq!((mode(&md), mtime_ns(&md)), (0o600, 1_000_000_000));
        assert!(!atomic_write(&path, b"two", None, true).unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"one");
        let new = tmp.path().join("b.json");
        assert!(atomic_write(&new, b"three", None, true).unwrap());
        let names: Vec<_> = fs::read_dir(tmp.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names.len(), 2, "no temporary file is left behind: {names:?}");
    }

    #[test]
    fn a_clone_keeps_contents_mode_and_time() {
        let tmp = crate::testing::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::write(&src, "data").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o640)).unwrap();
        File::options().write(true).open(&src).unwrap().set_modified(from_ns(2_000_000_000)).unwrap();
        let dst = tmp.path().join("dst");
        clone_file(&src, &dst).unwrap();
        let md = fs::metadata(&dst).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"data");
        assert_eq!((mode(&md), mtime_ns(&md)), (0o640, 2_000_000_000));
        assert!(clone_file(&src, &dst).is_err(), "an existing file is never replaced");
    }
}
