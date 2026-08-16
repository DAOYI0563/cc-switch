use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::{
    adapters::wsl_paths::FixedWslPathResolver, domain::ManagedClientId, ports::WslPathResolver,
};

/// 获取用户主目录，带回退和日志
///
/// ## Windows 注意事项
///
/// - `dirs::home_dir()` 在 Windows 上使用 `SHGetKnownFolderPath(FOLDERID_Profile)`，
///   返回的是真实用户目录（类似 `C:\\Users\\Alice`），与 v3.10.2 行为一致。
/// - 不要直接使用 `HOME` 环境变量：它可能由 Git/Cygwin/MSYS 等第三方工具注入，
///   且不一定等于用户目录，可能导致应用数据根变化。
///
/// ## 测试隔离
///
/// 为了让 Windows CI/本地测试能稳定隔离真实用户数据，可通过 `CC_SWITCH_TEST_HOME`
/// 显式覆盖 home dir（仅用于测试/调试场景）。
pub fn get_home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("CC_SWITCH_TEST_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    dirs::home_dir().unwrap_or_else(|| {
        log::warn!("无法获取用户主目录，回退到当前目录");
        PathBuf::from(".")
    })
}

pub const APP_DATA_DIR_NAME: &str = ".wsl-code-switch";

fn app_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(APP_DATA_DIR_NAME)
}

/// 获取 Claude Code 配置目录路径
pub fn get_claude_config_dir() -> PathBuf {
    FixedWslPathResolver::runtime()
        .client_config_root(ManagedClientId::Claude)
        .windows
}

/// 默认 Claude MCP 配置文件路径 (~/.claude.json)
pub fn get_default_claude_mcp_path() -> PathBuf {
    FixedWslPathResolver::runtime().claude_state_file().windows
}

/// 获取 Claude MCP 配置文件路径
pub fn get_claude_mcp_path() -> PathBuf {
    get_default_claude_mcp_path()
}

/// 获取 Claude Code 主配置文件路径
pub fn get_claude_settings_path() -> PathBuf {
    let dir = get_claude_config_dir();
    let settings = dir.join("settings.json");
    if settings.exists() {
        return settings;
    }
    // 兼容旧版命名：若存在旧文件则继续使用
    let legacy = dir.join("claude.json");
    if legacy.exists() {
        return legacy;
    }
    // 默认新建：回落到标准文件名 settings.json（不再生成 claude.json）
    settings
}

/// 获取固定应用数据根目录（Windows: `%USERPROFILE%\.wsl-code-switch`）。
pub fn get_app_config_dir() -> PathBuf {
    app_config_dir_for_home(&get_home_dir())
}

/// 获取应用配置文件路径
pub fn get_app_config_path() -> PathBuf {
    get_app_config_dir().join("config.json")
}

/// 清理供应商名称，确保文件名安全
#[allow(dead_code)]
pub fn sanitize_provider_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

/// 获取供应商配置文件路径
#[allow(dead_code)]
pub fn get_provider_config_path(provider_id: &str, provider_name: Option<&str>) -> PathBuf {
    let base_name = provider_name
        .map(sanitize_provider_name)
        .unwrap_or_else(|| sanitize_provider_name(provider_id));

    get_claude_config_dir().join(format!("settings-{base_name}.json"))
}

/// 读取 JSON 配置文件
pub fn read_json_file<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, AppError> {
    if !path.exists() {
        return Err(AppError::Config(format!("文件不存在: {}", path.display())));
    }

    let content = fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;

    serde_json::from_str(&content).map_err(|e| AppError::json(path, e))
}

/// 递归排序 JSON 对象的键（按字母顺序），确保序列化输出是确定性的
fn sort_json_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted_map = Map::new();
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for key in keys {
                sorted_map.insert(key.clone(), sort_json_keys(&map[key]));
            }
            Value::Object(sorted_map)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_json_keys).collect()),
        other => other.clone(),
    }
}

/// 写入 JSON 配置文件并返回实际写入的字节。
pub fn write_json_file_with_contents<T: Serialize>(
    path: &Path,
    data: &T,
) -> Result<Vec<u8>, AppError> {
    // 确保目录存在
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let value = serde_json::to_value(data).map_err(|e| AppError::JsonSerialize { source: e })?;
    let sorted_value = sort_json_keys(&value);
    let json = serde_json::to_string_pretty(&sorted_value)
        .map_err(|e| AppError::JsonSerialize { source: e })?;

    let contents = json.into_bytes();
    atomic_write(path, &contents)?;
    Ok(contents)
}

/// 写入 JSON 配置文件（键按字母排序，确保确定性输出）
pub fn write_json_file<T: Serialize>(path: &Path, data: &T) -> Result<(), AppError> {
    write_json_file_with_contents(path, data).map(|_| ())
}

/// 原子写入文本文件（用于 TOML/纯文本）
pub fn write_text_file(path: &Path, data: &str) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }
    atomic_write(path, data.as_bytes())
}

/// 原子写入：写入临时文件后 rename 替换，避免半写状态
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("无效的路径".to_string()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Config("无效的文件名".to_string()))?
        .to_string_lossy()
        .to_string();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (tmp, mut file) = (|| -> Result<(PathBuf, fs::File), AppError> {
        let mut last_collision = None;
        for _ in 0..16 {
            let counter = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let candidate = parent.join(format!(
                "{file_name}.tmp.{}.{ts}.{counter}",
                std::process::id()
            ));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => return Ok((candidate, file)),
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_collision = Some((candidate, source));
                }
                Err(source) => return Err(AppError::io(&candidate, source)),
            }
        }

        let (candidate, source) = last_collision.expect("temporary filename loop must run");
        Err(AppError::io(&candidate, source))
    })()?;

    if let Err(source) = file.write_all(data).and_then(|_| file.flush()) {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(AppError::io(&tmp, source));
    }
    drop(file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let perm = meta.permissions().mode();
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(perm));
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            Foundation::ERROR_NOT_SUPPORTED, Storage::FileSystem::ReplaceFileW,
        };

        let replaced: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let replacement: Vec<u16> = tmp
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut completed = false;
        let mut last_error = None;

        for _ in 0..3 {
            // SAFETY: both path buffers are NUL-terminated UTF-16 and remain alive for the
            // duration of the call. Backup, exclusion, and reserved pointers are intentionally null.
            let replaced_ok = unsafe {
                ReplaceFileW(
                    replaced.as_ptr(),
                    replacement.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            if replaced_ok != 0 {
                completed = true;
                break;
            }

            let replace_error = std::io::Error::last_os_error();
            // WSL UNC paths reject ReplaceFileW with ERROR_NOT_SUPPORTED (50).
            // std::fs::rename uses a different replace-existing API on Windows.
            let replace_not_supported =
                replace_error.raw_os_error() == Some(ERROR_NOT_SUPPORTED as i32);
            if replace_error.kind() != std::io::ErrorKind::NotFound && !replace_not_supported {
                last_error = Some(replace_error);
                break;
            }

            match fs::rename(&tmp, path) {
                Ok(()) => {
                    completed = true;
                    break;
                }
                Err(source)
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                    ) =>
                {
                    last_error = Some(source);
                }
                Err(source) => {
                    last_error = Some(source);
                    break;
                }
            }
        }

        if !completed {
            let source = last_error.unwrap_or_else(std::io::Error::last_os_error);
            let _ = fs::remove_file(&tmp);
            return Err(AppError::IoContext {
                context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
                source,
            });
        }
    }

    #[cfg(not(windows))]
    {
        if let Err(source) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(AppError::IoContext {
                context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
                source,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_config_dir_is_fixed_under_windows_user_home() {
        let home = Path::new(r"C:\Users\zhldm");

        assert_eq!(app_config_dir_for_home(home), home.join(".wsl-code-switch"));
    }

    fn assert_atomic_write_replaces_existing_file(dir: &Path) {
        let path = dir.join("atomic-write-contract.json");
        std::fs::write(&path, b"old contents").unwrap();

        atomic_write(&path, b"new contents").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new contents");
        let tmp_prefix = "atomic-write-contract.json.tmp.";
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(tmp_prefix))
            .map(|entry| entry.path())
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files remain: {leftovers:?}"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_atomic_write_replaces_existing_file(dir.path());
    }

    #[cfg(windows)]
    #[test]
    fn atomic_write_preserves_destination_when_windows_replace_fails() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, b"old contents").unwrap();
        let held_file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();

        let result = atomic_write(&path, b"new contents");

        assert!(result.is_err());
        drop(held_file);
        assert_eq!(std::fs::read(&path).unwrap(), b"old contents");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires CC_SWITCH_WSL_TEST_DIR to point to a WSL2 UNC directory"]
    fn atomic_write_replaces_existing_wsl_unc_file() {
        let root = PathBuf::from(
            std::env::var_os("CC_SWITCH_WSL_TEST_DIR").expect("CC_SWITCH_WSL_TEST_DIR must be set"),
        );
        let home = get_home_dir();
        let temp = std::env::temp_dir();
        for (name, path) in [
            ("test root", root.as_path()),
            ("test home", home.as_path()),
            ("temporary directory", temp.as_path()),
        ] {
            let unc = path.to_string_lossy();
            assert!(
                unc.starts_with(r"\\wsl.localhost\") || unc.starts_with(r"\\wsl$\"),
                "expected {name} to be a WSL UNC path, got {unc}"
            );
            assert!(
                path.starts_with(&root),
                "expected {name} to be under {}, got {unc}",
                root.display()
            );
        }

        let dir = tempfile::Builder::new()
            .prefix("atomic-write-contract-")
            .tempdir_in(&root)
            .unwrap();
        assert_atomic_write_replaces_existing_file(dir.path());
    }

    #[test]
    fn sort_json_keys_sorts_top_level_object() {
        let input = serde_json::json!({
            "z": 1,
            "a": 2,
            "m": 3,
        });
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(serialized, r#"{"a":2,"m":3,"z":1}"#);
    }

    #[test]
    fn sort_json_keys_recurses_into_nested_objects() {
        let input = serde_json::json!({
            "outer_b": {"z": 1, "a": 2},
            "outer_a": {"y": 3, "b": 4},
        });
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(
            serialized,
            r#"{"outer_a":{"b":4,"y":3},"outer_b":{"a":2,"z":1}}"#
        );
    }

    #[test]
    fn sort_json_keys_preserves_array_order() {
        let input = serde_json::json!([3, 1, 2]);
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(serialized, "[3,1,2]");
    }

    #[test]
    fn sort_json_keys_sorts_objects_inside_arrays_but_keeps_array_order() {
        let input = serde_json::json!([
            {"z": 1, "a": 2},
            {"y": 3, "b": 4},
        ]);
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(serialized, r#"[{"a":2,"z":1},{"b":4,"y":3}]"#);
    }

    #[test]
    fn sort_json_keys_passes_through_primitives() {
        let cases = vec![
            serde_json::json!("hello"),
            serde_json::json!(42),
            serde_json::json!(3.5),
            serde_json::json!(true),
            serde_json::json!(null),
        ];
        for value in cases {
            let sorted = sort_json_keys(&value);
            assert_eq!(sorted, value);
        }
    }

    #[test]
    fn sort_json_keys_handles_empty_collections() {
        let empty_obj = serde_json::json!({});
        assert_eq!(
            serde_json::to_string(&sort_json_keys(&empty_obj)).unwrap(),
            "{}"
        );

        let empty_arr = serde_json::json!([]);
        assert_eq!(
            serde_json::to_string(&sort_json_keys(&empty_arr)).unwrap(),
            "[]"
        );
    }

    #[test]
    fn sort_json_keys_produces_identical_output_for_different_insertion_orders() {
        // 核心保证：同一逻辑配置无论键的插入顺序如何，写出的字节序列必须一致。
        let mut a = Map::new();
        a.insert("env".to_string(), serde_json::json!({"PATH": "/usr/bin"}));
        a.insert("model".to_string(), serde_json::json!("claude-sonnet-4-5"));
        a.insert("permissions".to_string(), serde_json::json!({"allow": []}));

        let mut b = Map::new();
        b.insert("permissions".to_string(), serde_json::json!({"allow": []}));
        b.insert("model".to_string(), serde_json::json!("claude-sonnet-4-5"));
        b.insert("env".to_string(), serde_json::json!({"PATH": "/usr/bin"}));

        let sorted_a = sort_json_keys(&Value::Object(a));
        let sorted_b = sort_json_keys(&Value::Object(b));

        assert_eq!(
            serde_json::to_string(&sorted_a).unwrap(),
            serde_json::to_string(&sorted_b).unwrap(),
        );
    }
}

/// 删除文件
pub fn delete_file(path: &Path) -> Result<(), AppError> {
    if path.exists() {
        fs::remove_file(path).map_err(|e| AppError::io(path, e))?;
    }
    Ok(())
}

/// 检查 Claude Code 配置状态
#[derive(Serialize, Deserialize)]
pub struct ConfigStatus {
    pub exists: bool,
    pub path: String,
}

/// 获取 Claude Code 配置状态
pub fn get_claude_config_status() -> ConfigStatus {
    let path = get_claude_settings_path();
    ConfigStatus {
        exists: path.exists(),
        path: path.to_string_lossy().to_string(),
    }
}
