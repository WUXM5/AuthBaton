//! mkdir leases compatible with Claude Code 2.1.278's proper-lockfile protocol.
//! Directories stay empty so official stale-lock recovery can use rmdir.
use crate::platform::{Result, private_dir};
use std::fs::{self, DirBuilder, File, FileTimes, Metadata};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const BUSY: &str = "Claude 正在由其他客户端维护，稍后自动重试";
pub const LOST: &str = "Claude 协调锁已失效，已停止提交凭据";

struct Owned {
    path: PathBuf,
    file: File,
    stamp: Metadata,
    beat: Instant,
    stale: Duration,
    seconds: bool,
    lost: bool,
}
fn milliseconds(metadata: &Metadata) -> i128 {
    i128::from(metadata.mtime()) * 1000 + i128::from(metadata.mtime_nsec()) / 1_000_000
}
fn same(left: &Metadata, right: &Metadata) -> bool {
    left.is_dir()
        && right.is_dir()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
        && milliseconds(left) == milliseconds(right)
}
impl Owned {
    fn check(&mut self) -> Result<()> {
        if self.lost
            || self.beat.elapsed() >= self.stale
            || !fs::symlink_metadata(&self.path).is_ok_and(|m| same(&self.stamp, &m))
        {
            self.lost = true;
            return Err(LOST);
        }
        Ok(())
    }
    fn heartbeat(&mut self) -> Result<()> {
        self.check()?;
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| LOST)?
            .as_millis();
        let ms = if self.seconds {
            ms.div_ceil(1000) * 1000
        } else {
            ms
        };
        // Updating the open directory, rather than its path, cannot touch a successor.
        if self
            .file
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(ms as u64)))
            .is_err()
        {
            self.lost = true;
            return Err(LOST);
        }
        self.stamp = self.file.metadata().map_err(|_| LOST)?;
        self.beat = Instant::now();
        self.check()
    }
}
pub struct Lease {
    owned: Arc<Mutex<Owned>>,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}
impl Lease {
    fn acquire(path: PathBuf, stale: Duration, update: Duration) -> Result<Self> {
        match DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let before = fs::symlink_metadata(&path).map_err(|_| BUSY)?;
                if !before.is_dir()
                    || before
                        .modified()
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_none_or(|age| age <= stale)
                {
                    return Err(BUSY);
                }
                if !fs::symlink_metadata(&path).is_ok_and(|m| same(&before, &m)) {
                    return Err(BUSY);
                }
                fs::remove_dir(&path).map_err(|_| BUSY)?;
                DirBuilder::new()
                    .mode(0o700)
                    .create(&path)
                    .map_err(|_| BUSY)?;
            }
            Err(_) => return Err("无法建立 Claude 协调锁，请检查配置目录权限"),
        }
        let file = File::open(&path).map_err(|_| LOST)?;
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| LOST)?
            .as_millis()
            .div_ceil(1000)
            * 1000
            + 5;
        file.set_times(
            FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(ms as u64)),
        )
        .map_err(|_| LOST)?;
        let stamp = file.metadata().map_err(|_| LOST)?;
        let seconds = milliseconds(&stamp) % 1000 == 0;
        let owned = Arc::new(Mutex::new(Owned {
            path,
            file,
            stamp,
            beat: Instant::now(),
            stale,
            seconds,
            lost: false,
        }));
        let (stop, receiver) = mpsc::channel();
        let shared = owned.clone();
        let worker = std::thread::spawn(move || {
            while receiver.recv_timeout(update) == Err(mpsc::RecvTimeoutError::Timeout) {
                if shared.lock().unwrap().heartbeat().is_err() {
                    break;
                }
            }
        });
        let lease = Self {
            owned,
            stop,
            worker: Some(worker),
        };
        lease.check()?;
        Ok(lease)
    }
    pub fn check(&self) -> Result<()> {
        self.owned.lock().unwrap().check()
    }
    pub fn storage(root: &Path) -> Result<Self> {
        Self::acquire(
            root.join(".storage-write.lock"),
            Duration::from_secs(15),
            Duration::from_millis(7500),
        )
    }
    /// The official `<config>.lock` around saves of `~/.claude.json` (proper-lockfile defaults).
    /// Official holders keep it for one read-modify-write, so contention is retried briefly.
    pub fn config(config: &Path) -> Result<Self> {
        let mut path = config.as_os_str().to_owned();
        path.push(".lock");
        let path = PathBuf::from(path);
        for _ in 0..20 {
            match Self::acquire(
                path.clone(),
                Duration::from_secs(10),
                Duration::from_secs(5),
            ) {
                Err(BUSY) => std::thread::sleep(Duration::from_millis(100)),
                result => return result,
            }
        }
        Err(BUSY)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Ok(mut owned) = self.owned.lock()
            && owned.check().is_ok()
        {
            let _ = fs::remove_dir(&owned.path);
        }
    }
}
pub struct RefreshLocks {
    primary: Lease,
    legacy: Lease,
}
impl RefreshLocks {
    pub fn acquire(root: &Path) -> Result<Self> {
        private_dir(root)?;
        let primary = Lease::acquire(
            root.join(".oauth_refresh.lock"),
            Duration::from_secs(60),
            Duration::from_secs(5),
        )?;
        let canonical = fs::canonicalize(root).map_err(|_| "无法定位 Claude 存储目录")?;
        let mut legacy = canonical.into_os_string();
        legacy.push(".lock");
        let legacy = Lease::acquire(
            legacy.into(),
            Duration::from_secs(60),
            Duration::from_secs(5),
        )?;
        Ok(Self { primary, legacy })
    }
    pub fn check(&self) -> Result<()> {
        self.primary.check()?;
        self.legacy.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "authmanager-lock-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        private_dir(&path).unwrap();
        path
    }
    #[test]
    fn locks_are_exclusive_and_release_does_not_delete_a_successor() {
        let root = root();
        let locks = RefreshLocks::acquire(&root).unwrap();
        assert!(RefreshLocks::acquire(&root).is_err());
        assert!(root.join(".oauth_refresh.lock").is_dir());
        let old = root.join("old-lock");
        fs::rename(root.join(".oauth_refresh.lock"), &old).unwrap();
        fs::create_dir(root.join(".oauth_refresh.lock")).unwrap();
        assert!(locks.check().is_err());
        drop(locks);
        assert!(root.join(".oauth_refresh.lock").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn heartbeat_and_stale_recovery_follow_directory_mtime() {
        let root = root();
        let path = root.join("lease");
        let lease = Lease::acquire(
            path.clone(),
            Duration::from_secs(2),
            Duration::from_millis(25),
        )
        .unwrap();
        let before = milliseconds(&fs::metadata(&path).unwrap());
        std::thread::sleep(Duration::from_millis(80));
        assert_ne!(milliseconds(&fs::metadata(&path).unwrap()), before);
        lease.check().unwrap();
        drop(lease);
        fs::create_dir(&path).unwrap();
        File::open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH))
            .unwrap();
        let replacement = Lease::acquire(
            path.clone(),
            Duration::from_secs(2),
            Duration::from_millis(25),
        )
        .unwrap();
        replacement.check().unwrap();
        drop(replacement);
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn legacy_lock_is_canonical_and_failed_acquisition_releases_primary() {
        let root = root();
        let alias = root.with_extension("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let locks = RefreshLocks::acquire(&alias).unwrap();
        assert!(RefreshLocks::acquire(&root).is_err());
        drop(locks);
        let mut legacy = fs::canonicalize(&root).unwrap().into_os_string();
        legacy.push(".lock");
        fs::create_dir(PathBuf::from(&legacy)).unwrap();
        assert!(RefreshLocks::acquire(&root).is_err());
        assert!(!root.join(".oauth_refresh.lock").exists());
        fs::remove_dir(PathBuf::from(legacy)).unwrap();
        fs::remove_file(alias).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "requires the hash-pinned local Claude binary; synthetic directories only"]
    fn official_peer_interoperability() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};
        let script = std::env::var("AUTHMANAGER_CLAUDE_LOCK_PEER")
            .expect("explicit peer fixture path required");
        let root = root();
        for kind in ["refresh", "storage"] {
            let refresh = if kind == "refresh" {
                Some(RefreshLocks::acquire(&root).unwrap())
            } else {
                None
            };
            let storage = if kind == "storage" {
                Some(Lease::storage(&root).unwrap())
            } else {
                None
            };
            let output = Command::new("node")
                .args([&script, "--root", root.to_str().unwrap(), "--kind", kind])
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stdout).contains("ELOCKED"));
            if let Some(locks) = refresh {
                locks.check().unwrap();
                drop(locks);
            }
            if let Some(lock) = storage {
                lock.check().unwrap();
                drop(lock);
            }
            let mut peer = Command::new("node")
                .args([&script, "--root", root.to_str().unwrap(), "--kind", kind])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut input = peer.stdin.take().unwrap();
            let mut output = BufReader::new(peer.stdout.take().unwrap());
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            assert!(line.contains("ready"));
            if kind == "refresh" {
                assert!(RefreshLocks::acquire(&root).is_err());
            } else {
                assert!(Lease::storage(&root).is_err());
            }
            let path = root.join(if kind == "refresh" {
                ".oauth_refresh.lock"
            } else {
                ".storage-write.lock"
            });
            let before = milliseconds(&fs::metadata(&path).unwrap());
            std::thread::sleep(Duration::from_secs(if kind == "refresh" { 6 } else { 8 }));
            assert_ne!(milliseconds(&fs::metadata(&path).unwrap()), before);
            writeln!(input, "{{\"op\":\"release\"}}").unwrap();
            input.flush().unwrap();
            line.clear();
            output.read_line(&mut line).unwrap();
            assert!(line.contains("released"));
            assert!(peer.wait().unwrap().success());
            assert!(!path.exists());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
