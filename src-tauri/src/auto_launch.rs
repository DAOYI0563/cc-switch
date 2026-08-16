use crate::error::AppError;

#[cfg(target_os = "windows")]
const RUN_VALUE_NAME: &str = "WSL Code Switch";

#[cfg(target_os = "windows")]
fn run_key() -> Result<winreg::RegKey, AppError> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    let current_user = winreg::RegKey::predef(HKEY_CURRENT_USER);
    current_user
        .open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_READ | KEY_WRITE,
        )
        .map_err(|error| AppError::Message(format!("打开开机启动注册表失败: {error}")))
}

#[cfg(target_os = "windows")]
pub fn enable_auto_launch() -> Result<(), AppError> {
    let executable = std::env::current_exe()
        .map_err(|error| AppError::Message(format!("无法获取应用路径: {error}")))?;
    let command = format!("\"{}\"", executable.display());
    run_key()?
        .set_value(RUN_VALUE_NAME, &command)
        .map_err(|error| AppError::Message(format!("写入开机启动注册表失败: {error}")))
}

#[cfg(target_os = "windows")]
pub fn disable_auto_launch() -> Result<(), AppError> {
    match run_key()?.delete_value(RUN_VALUE_NAME) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::Message(format!(
            "删除开机启动注册表失败: {error}"
        ))),
    }
}

#[cfg(target_os = "windows")]
pub fn is_auto_launch_enabled() -> Result<bool, AppError> {
    use winreg::types::FromRegValue;
    let key = run_key()?;
    match key.get_raw_value(RUN_VALUE_NAME) {
        Ok(value) => String::from_reg_value(&value)
            .map(|command| !command.trim().is_empty())
            .map_err(|error| AppError::Message(format!("读取开机启动注册表失败: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::Message(format!(
            "读取开机启动注册表失败: {error}"
        ))),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn enable_auto_launch() -> Result<(), AppError> {
    Err(AppError::Message("仅支持 Windows 便携版".to_string()))
}

#[cfg(not(target_os = "windows"))]
pub fn disable_auto_launch() -> Result<(), AppError> {
    Err(AppError::Message("仅支持 Windows 便携版".to_string()))
}

#[cfg(not(target_os = "windows"))]
pub fn is_auto_launch_enabled() -> Result<bool, AppError> {
    Err(AppError::Message("仅支持 Windows 便携版".to_string()))
}
