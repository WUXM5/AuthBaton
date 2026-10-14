use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone)]
pub struct Platform {
    pub helper: PathBuf,
    pub recovery_wait: Option<RecoveryWait>,
    #[cfg(test)]
    pub fixture_keychain: bool,
}
#[derive(Clone)]
pub struct RecoveryWait {
    deadline: Instant,
    cancel: Arc<AtomicBool>,
}
impl Platform {
    pub fn for_recovery(&self, cancel: Arc<AtomicBool>) -> Self {
        let mut platform = self.clone();
        platform.recovery_wait = Some(RecoveryWait {
            deadline: Instant::now() + Duration::from_secs(300),
            cancel,
        });
        platform
    }
    fn io_timeout(&self) -> Result<Duration> {
        let Some(wait) = &self.recovery_wait else {
            return Ok(Duration::from_secs(5));
        };
        if wait.cancel.load(Ordering::Acquire) {
            return Err("恢复已停止；保留记录供下次恢复");
        }
        let remaining = wait
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or("恢复等待超时；保留记录，请稍后重试")?;
        Ok(remaining)
    }
    fn run_native_status(
        &self,
        binary: &Path,
        args: &[&str],
        input: &[u8],
    ) -> Result<(Option<i32>, Vec<u8>)> {
        pipe_status_with_cancel(
            binary,
            args,
            input,
            self.io_timeout()?,
            self.recovery_wait.as_ref().map(|wait| wait.cancel.as_ref()),
            &[],
        )
    }
    fn run_native(&self, binary: &Path, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
        let (status, bytes) = self.run_native_status(binary, args, input)?;
        if status != Some(0) {
            self.io_timeout()?;
            return Err("原生操作失败或超时");
        }
        Ok(bytes)
    }
    fn invoke(&self, input: Value) -> Result<Value> {
        let bytes = serde_json::to_vec(&input).map_err(|_| "无法编码存储请求")?;
        let output = self.run_native(&self.helper, &[], &bytes)?;
        serde_json::from_slice(&output).map_err(|_| "原生存储返回无效结果")
    }
    pub fn read_key(&self, service: &str, account: &str) -> Result<Option<Value>> {
        #[cfg(test)]
        if self.fixture_keychain {
            return self.fixture_read_key(service, account);
        }
        self.native_read_key(service, account)
    }
    #[cfg(test)]
    fn fixture_read_key(&self, service: &str, account: &str) -> Result<Option<Value>> {
        let output =
            self.invoke(json!({"operation":"read","service":service,"account":account}))?;
        match output["status"].as_str() {
            Some("missing") => Ok(None),
            Some("ok") => {
                let bytes = STANDARD
                    .decode(output["value"].as_str().ok_or("凭据格式无效")?)
                    .map_err(|_| "凭据编码无效")?;
                Ok(Some(
                    serde_json::from_slice(&bytes).map_err(|_| "凭据格式无效")?,
                ))
            }
            _ => Err("Keychain 暂不可读；请解锁钥匙串后重试"),
        }
    }
    /// Stands in for an official client's own write, so no AuthManager writer limit applies.
    #[cfg(test)]
    pub fn write_key(&self, service: &str, account: &str, value: &Value) -> Result<()> {
        let value = STANDARD.encode(serde_json::to_vec(value).map_err(|_| "凭据格式无效")?);
        let output = self.invoke(
            json!({"operation":"write","service":service,"account":account,"value":value}),
        )?;
        if output["status"] != "ok" {
            return Err("Keychain 保存失败；未回退为明文存储");
        }
        Ok(())
    }
    pub fn delete_key(&self, service: &str, account: &str) -> Result<()> {
        #[cfg(test)]
        if self.fixture_keychain {
            return self.fixture_delete_key(service, account);
        }
        if service == "io.github.wuxm5.authbaton.vault" {
            let result = self.invoke(json!({"operation":"vault.delete","account":account}))?;
            return if result["status"] == "ok" {
                Ok(())
            } else {
                Err("无法清理临时账号凭据")
            };
        }
        self.native_delete_key(service, account)
    }
    #[cfg(test)]
    fn fixture_delete_key(&self, service: &str, account: &str) -> Result<()> {
        let output =
            self.invoke(json!({"operation":"delete","service":service,"account":account}))?;
        if matches!(output["status"].as_str(), Some("ok" | "missing")) {
            Ok(())
        } else {
            Err("无法清理临时凭据")
        }
    }
    pub fn load_db(&self, path: &Path) -> Result<Option<Value>> {
        let output = self.invoke(json!({"operation":"db.read","path":path}))?;
        match output["status"].as_str() {
            Some("missing") => Ok(None),
            Some("ok") => Ok(Some(
                serde_json::from_str(output["value"].as_str().ok_or("数据库格式无效")?)
                    .map_err(|_| "数据库格式无效")?,
            )),
            _ => Err("无法读取账号数据库"),
        }
    }
    pub fn save_db(&self, path: &Path, value: &Value) -> Result<()> {
        let output =
            self.invoke(json!({"operation":"db.write","path":path,"value":value.to_string()}))?;
        if output["status"] == "ok" {
            Ok(())
        } else {
            Err("账号数据库保存失败")
        }
    }
    pub fn native_read_key(&self, service: &str, account: &str) -> Result<Option<Value>> {
        #[cfg(test)]
        if self.fixture_keychain {
            return self.fixture_read_key(service, account);
        }
        self.require_unlocked_keychain()?;
        let (status, bytes) = self.run_native_status(
            Path::new("/usr/bin/security"),
            &["find-generic-password", "-s", service, "-a", account, "-w"],
            &[],
        )?;
        match status {
            Some(0) => decode_security_value(&bytes).map(Some),
            Some(44) => Ok(None),
            _ => Err("无法读取原生 Keychain，请检查系统访问权限"),
        }
    }
    pub fn native_write_key(&self, service: &str, account: &str, value: &Value) -> Result<()> {
        self.native_write_key_guarded(service, account, value, &|| Ok(()))
    }
    pub fn preflight_native_write(
        &self,
        service: &str,
        account: &str,
        value: &Value,
    ) -> Result<()> {
        native_write_plan(service, account, value).map(|_| ())
    }
    pub fn native_write_key_guarded(
        &self,
        service: &str,
        account: &str,
        value: &Value,
        guard: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        // Exercise the production boundary before the synthetic storage adapter too.
        let plan = native_write_plan(service, account, value)?;
        #[cfg(test)]
        if self.fixture_keychain {
            guard()?;
            self.write_key(service, account, value)?;
            return if self.fixture_read_key(service, account)?.as_ref() == Some(value) {
                Ok(())
            } else {
                Err("原生 Keychain 保存后校验失败")
            };
        }
        self.require_unlocked_keychain()?;
        guard()?;
        match plan {
            NativeWritePlan::Interactive(command) => {
                self.run_native(Path::new("/usr/bin/security"), &["-i"], command.as_bytes())?;
            }
            NativeWritePlan::Large(hex) => {
                // Same writer identity as the stdin path: direct Security APIs alter the item's
                // partition ACL. Never log this argument vector or put it through a shell.
                self.run_native(
                    Path::new("/usr/bin/security"),
                    &[
                        "add-generic-password",
                        "-U",
                        "-a",
                        account,
                        "-s",
                        service,
                        "-X",
                        &hex,
                    ],
                    &[],
                )?;
            }
        }
        if self.native_read_key(service, account)?.as_ref() != Some(value) {
            return Err("原生 Keychain 保存后校验失败");
        }
        Ok(())
    }
    pub fn native_delete_key(&self, service: &str, account: &str) -> Result<()> {
        self.require_unlocked_keychain()?;
        let (status, _) = self.run_native_status(
            Path::new("/usr/bin/security"),
            &["delete-generic-password", "-s", service, "-a", account],
            &[],
        )?;
        if matches!(status, Some(0 | 44)) {
            Ok(())
        } else {
            Err("无法清理临时 Keychain 凭据")
        }
    }
    pub fn vault_read(&self, id: &str) -> Result<Value> {
        self.vault_read_optional(id)?
            .ok_or("账号凭据尚未保存或已被移除")
    }
    pub fn vault_read_optional(&self, id: &str) -> Result<Option<Value>> {
        #[cfg(test)]
        if self.fixture_keychain {
            return self.read_key("io.github.wuxm5.authbaton.vault", id);
        }
        let output = self.invoke(json!({"operation":"vault.read","account":id}))?;
        match output["status"].as_str() {
            Some("ok") => {
                let data = STANDARD
                    .decode(output["value"].as_str().ok_or("账号存储格式无效")?)
                    .map_err(|_| "账号存储格式无效")?;
                serde_json::from_slice(&data)
                    .map(Some)
                    .map_err(|_| "账号存储格式无效")
            }
            Some("missing") => Ok(None),
            _ => Err("AuthManager 账号存储暂不可读；请检查钥匙串权限"),
        }
    }
    pub fn vault_write(&self, id: &str, value: &Value) -> Result<()> {
        #[cfg(test)]
        if self.fixture_keychain {
            self.write_key("io.github.wuxm5.authbaton.vault", id, value)?;
        } else {
            self.write_vault_item(id, value)?;
        }
        #[cfg(not(test))]
        self.write_vault_item(id, value)?;
        if self.vault_read(id)? != *value {
            return Err("账号凭据保存后校验失败");
        }
        Ok(())
    }
    fn write_vault_item(&self, id: &str, value: &Value) -> Result<()> {
        self.require_unlocked_keychain()?;
        let encoded = STANDARD.encode(serde_json::to_vec(value).map_err(|_| "凭据格式无效")?);
        let output =
            self.invoke(json!({"operation":"vault.write","account":id,"value":encoded}))?;
        if output["status"] == "ok" {
            Ok(())
        } else {
            Err("AuthManager 账号保存失败；未修改官方登录")
        }
    }
    fn require_unlocked_keychain(&self) -> Result<()> {
        #[cfg(test)]
        if self.fixture_keychain {
            return Ok(());
        }
        let output = self.invoke(json!({"operation":"keychain.status"}))?;
        if output["status"] == "ok" {
            Ok(())
        } else {
            Err("Keychain 已锁定或不可用；请在系统中解锁后重试")
        }
    }
}

pub fn install_helper(root: &Path, bundled: &Path) -> Result<PathBuf> {
    let path = root.join("credential-store-v1");
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file()
                || metadata.permissions().mode() & 0o077 != 0
                || metadata.permissions().mode() & 0o100 == 0
            {
                return Err("已安装的凭据组件不是私有可执行文件");
            }
            // Do not replace its code-signing identity during ordinary app upgrades.
            return Ok(path);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法检查凭据组件"),
    }
    let temporary = root.join(format!("helper-{}", random_id()?));
    let result = (|| {
        fs::copy(bundled, &temporary).map_err(|_| "无法准备凭据组件")?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))
            .map_err(|_| "无法保护凭据组件")?;
        fs::rename(&temporary, &path).map_err(|_| "无法安装凭据组件")?;
        Ok(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

enum NativeWritePlan {
    Interactive(String),
    Large(String),
}
fn native_write_plan(service: &str, account: &str, value: &Value) -> Result<NativeWritePlan> {
    if service.len() > 512
        || account.len() > 512
        || service.contains(['"', '\\', '\n', '\r', '\0'])
        || account.contains(['"', '\\', '\n', '\r', '\0'])
    {
        return Err("Keychain 标识无效");
    }
    let bytes = serde_json::to_vec(value).map_err(|_| "凭据格式无效")?;
    if bytes.len() > 32 * 1024 {
        return Err("原生 Keychain 数据超过 32 KB 写入上限；已停止续期，请由官方客户端处理");
    }
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let command = format!("add-generic-password -U -a \"{account}\" -s \"{service}\" -X {hex}\n");
    // security -i reads a fixed 4096-byte line, including the terminating NUL.
    if command.len() >= 4096 {
        return Ok(NativeWritePlan::Large(hex));
    }
    Ok(NativeWritePlan::Interactive(command))
}

fn decode_security_value(bytes: &[u8]) -> Result<Value> {
    let bytes = bytes.trim_ascii();
    // security find-generic-password -w renders non-printable bytes (including UTF-8)
    // as an unprefixed hex string; normalize that documented output at this boundary.
    let decoded;
    let json = if bytes.first() == Some(&b'{') {
        bytes
    } else {
        if bytes.is_empty()
            || !bytes.len().is_multiple_of(2)
            || !bytes.iter().all(u8::is_ascii_hexdigit)
        {
            return Err("原生 Keychain 格式无效");
        }
        decoded = bytes
            .chunks_exact(2)
            .map(|pair| {
                let high = (pair[0] as char).to_digit(16).unwrap() as u8;
                let low = (pair[1] as char).to_digit(16).unwrap() as u8;
                (high << 4) | low
            })
            .collect::<Vec<_>>();
        decoded.as_slice()
    };
    let value: Value = serde_json::from_slice(json).map_err(|_| "原生 Keychain 格式无效")?;
    if !value.is_object() {
        return Err("原生 Keychain 格式无效");
    }
    Ok(value)
}

pub fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|_| "无法创建账号目录")
}
pub fn read_json(path: &Path) -> Result<Option<Value>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取认证存储"),
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取认证存储")?;
    if bytes.len() > 1024 * 1024 {
        return Err("认证存储超过允许大小");
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "认证存储格式无效")
}
pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", random_id()?));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| "无法准备认证文件")?;
        serde_json::to_writer(&mut file, value).map_err(|_| "无法写入认证文件")?;
        file.sync_all().map_err(|_| "无法同步认证文件")?;
        fs::rename(&temporary, path).map_err(|_| "无法替换认证文件")
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
pub fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|_| "无法创建账号标识")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn pipe_command(
    binary: &Path,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>> {
    let (status, bytes) = pipe_status(binary, args, input, timeout)?;
    if status != Some(0) {
        return Err("原生操作失败或超时");
    }
    Ok(bytes)
}
fn pipe_status(
    binary: &Path,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
) -> Result<(Option<i32>, Vec<u8>)> {
    pipe_status_with_cancel(binary, args, input, timeout, None, &[])
}
pub fn pipe_command_with_environment(
    binary: &Path,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
    environment: &[(String, String)],
) -> Result<Vec<u8>> {
    let (status, bytes) = pipe_status_with_cancel(binary, args, input, timeout, None, environment)?;
    if status != Some(0) {
        return Err("原生操作失败或超时");
    }
    Ok(bytes)
}
pub(crate) fn pipe_status_with_cancel(
    binary: &Path,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    environment: &[(String, String)],
) -> Result<(Option<i32>, Vec<u8>)> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err("恢复已停止；保留记录供下次恢复");
    }
    let mut command = Command::new(binary);
    command
        .args(args)
        .envs(environment.iter().map(|(key, value)| (key, value)));
    pipe_status_of(command, input, timeout, cancel)
}
pub const SPAWN_FAILED: &str = "无法启动原生操作";
/// Runs a prepared command with `input` on stdin, collecting at most 5 MiB of stdout; stderr is
/// dropped. The status is None when the command was stopped for the timeout or cancellation.
pub(crate) fn pipe_status_of(
    mut command: Command,
    input: &[u8],
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Result<(Option<i32>, Vec<u8>)> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| SPAWN_FAILED)?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let bytes = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&bytes);
    });
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .by_ref()
            .take(5 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            _ if start.elapsed() >= timeout
                || cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) =>
            {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let _ = writer.join();
    let bytes = reader
        .join()
        .map_err(|_| "原生操作中断")?
        .map_err(|_| "无法读取操作结果")?;
    if bytes.len() > 5 * 1024 * 1024 {
        return Err("原生操作失败或超时");
    }
    Ok((status, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_install_preserves_existing_identity_and_rejects_unsafe_path() {
        let root =
            std::env::temp_dir().join(format!("authmanager-helper-{}", random_id().unwrap()));
        private_dir(&root).unwrap();
        let bundled = root.join("bundle");
        fs::write(&bundled, b"original-synthetic-binary").unwrap();
        let installed = install_helper(&root, &bundled).unwrap();
        fs::write(&bundled, b"different-synthetic-build").unwrap();
        assert_eq!(install_helper(&root, &bundled).unwrap(), installed);
        assert_eq!(fs::read(&installed).unwrap(), b"original-synthetic-binary");
        fs::set_permissions(&installed, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(install_helper(&root, &bundled).is_err());
        assert_eq!(fs::read(&installed).unwrap(), b"original-synthetic-binary");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn security_reader_decodes_ascii_and_hex_rendered_unicode() {
        let ascii = json!({"synthetic":"value"});
        assert_eq!(
            decode_security_value(format!("{ascii}\n").as_bytes()).unwrap(),
            ascii
        );
        let unicode = json!({"synthetic":"合成账号😀", "quoted":"a\"b\nc"});
        let mut hex: String = unicode
            .to_string()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        hex.push('\n');
        assert_eq!(decode_security_value(hex.as_bytes()).unwrap(), unicode);
        for broken in [
            b"{\"synthetic\":".as_slice(),
            b"abc",
            b"zz",
            b"1234",
            b"null",
        ] {
            assert!(decode_security_value(broken).is_err());
        }
    }
    #[test]
    fn native_write_rejects_oversize_input_before_any_io() {
        let NativeWritePlan::Interactive(command) =
            native_write_plan("synthetic", "test", &json!({"value":"x".repeat(1024)})).unwrap()
        else {
            panic!("small payload must use private stdin")
        };
        assert!(command.len() < 4096);
        // Both official clients' logins outgrow the stdin line.
        for large in [
            json!({"claudeAiOauth":{"accessToken":"synthetic"},"mcpOAuth":"x".repeat(8192)}),
            json!({"auth_mode":"chatgpt","tokens":{"id_token":"x".repeat(3000)}}),
        ] {
            assert!(matches!(
                native_write_plan("synthetic", "test", &large).unwrap(),
                NativeWritePlan::Large(_)
            ));
        }
        for fixture_keychain in [true, false] {
            let platform = Platform {
                helper: PathBuf::from("/must-not-run"),
                recovery_wait: None,
                fixture_keychain,
            };
            assert!(
                platform
                    .native_write_key(
                        "synthetic",
                        "test",
                        &json!({"claudeAiOauth":{},"mcpOAuth":"x".repeat(32768)})
                    )
                    .is_err()
            );
        }
    }
}
#[test]
fn recovery_wait_is_bounded_and_cancellation_reaps_the_child() {
    let platform = Platform {
        helper: PathBuf::from("/must-not-run"),
        recovery_wait: None,
        fixture_keychain: true,
    };
    assert_eq!(platform.io_timeout().unwrap(), Duration::from_secs(5));
    let cancel = Arc::new(AtomicBool::new(false));
    let mut recovery = platform.for_recovery(cancel.clone());
    assert!(recovery.io_timeout().unwrap() > Duration::from_secs(299));
    assert!(recovery.io_timeout().unwrap() <= Duration::from_secs(300));
    let signal = cancel.clone();
    let trigger = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        signal.store(true, Ordering::Release);
    });
    let started = Instant::now();
    assert!(
        recovery
            .run_native(
                Path::new("/usr/bin/python3"),
                &["-c", "import time; time.sleep(10)"],
                &[]
            )
            .is_err()
    );
    trigger.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    cancel.store(false, Ordering::Release);
    recovery.recovery_wait.as_mut().unwrap().deadline = Instant::now();
    assert!(
        recovery
            .invoke(json!({"operation":"must-not-run"}))
            .is_err()
    );
}
