use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::app_config::LegacyAppType;
use crate::error::AppError;

fn default_true() -> bool {
    true
}

fn default_remote_root() -> String {
    "cc-switch-sync".to_string()
}

fn default_profile() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSyncSettings {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default, skip_serializing)]
    pub password: String,
    #[serde(default = "default_remote_root")]
    pub remote_root: String,
    #[serde(default = "default_profile")]
    pub profile: String,
}

impl Default for WebDavSyncSettings {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            username: String::new(),
            password: String::new(),
            remote_root: default_remote_root(),
            profile: default_profile(),
        }
    }
}

impl WebDavSyncSettings {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.base_url.trim().is_empty() {
            return Err(AppError::InvalidInput("WebDAV 地址不能为空".to_string()));
        }
        if self.username.trim().is_empty() {
            return Err(AppError::InvalidInput("WebDAV 用户名不能为空".to_string()));
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.base_url = self.base_url.trim().to_string();
        self.username = self.username.trim().to_string();
        self.remote_root = self.remote_root.trim().to_string();
        self.profile = self.profile.trim().to_string();
        if self.remote_root.is_empty() {
            self.remote_root = default_remote_root();
        }
        if self.profile.is_empty() {
            self.profile = default_profile();
        }
    }

    fn is_empty(&self) -> bool {
        self.base_url.is_empty() && self.username.is_empty() && self.password.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default = "default_true")]
    pub show_in_tray: bool,
    #[serde(default)]
    pub use_app_window_controls: bool,
    #[serde(default)]
    pub launch_on_startup: bool,
    #[serde(default)]
    pub silent_startup: bool,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_sync: Option<WebDavSyncSettings>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_in_tray: true,
            use_app_window_controls: false,
            launch_on_startup: false,
            silent_startup: false,
            language: Some("zh".to_string()),
            webdav_sync: None,
        }
    }
}

impl AppSettings {
    fn normalize(&mut self) {
        self.language = Some("zh".to_string());
        if let Some(sync) = &mut self.webdav_sync {
            sync.normalize();
            if sync.is_empty() {
                self.webdav_sync = None;
            }
        }
    }

    fn load() -> Self {
        let path = settings_path();
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&contents) {
            Ok(mut settings) => {
                settings.normalize();
                settings
            }
            Err(error) => {
                log::warn!("解析设置文件失败 {}: {error}", path.display());
                Self::default()
            }
        }
    }
}

static SETTINGS: OnceLock<RwLock<AppSettings>> = OnceLock::new();

fn settings_path() -> std::path::PathBuf {
    crate::config::get_app_config_dir().join("settings.json")
}

fn store() -> &'static RwLock<AppSettings> {
    SETTINGS.get_or_init(|| RwLock::new(AppSettings::load()))
}

pub fn get_settings() -> AppSettings {
    store()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

pub fn get_settings_for_frontend() -> AppSettings {
    let mut settings = get_settings();
    if let Some(sync) = settings.webdav_sync.as_mut() {
        sync.password.clear();
    }
    settings
}

pub fn update_settings(mut settings: AppSettings) -> Result<(), AppError> {
    save_with_secret_policy(&mut settings, false)?;
    *store().write().unwrap_or_else(|error| error.into_inner()) = settings;
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn reload_settings() -> Result<(), AppError> {
    *store().write().unwrap_or_else(|error| error.into_inner()) = AppSettings::load();
    Ok(())
}

pub fn get_effective_current_provider(
    db: &crate::database::Database,
    app_type: &LegacyAppType,
) -> Result<Option<String>, AppError> {
    db.get_current_provider(app_type.as_str())
}

pub fn get_webdav_sync_settings() -> Option<WebDavSyncSettings> {
    let settings = get_settings().webdav_sync?;
    #[cfg(target_os = "windows")]
    {
        use crate::ports::{DeviceSecretId, SecretStore};

        let mut settings = settings;
        match crate::adapters::secret_store::WindowsCredentialStore::runtime()
            .read(DeviceSecretId::WebdavPassword)
        {
            Ok(Some(password)) => settings.password = password,
            Ok(None) => {}
            Err(error) => log::warn!("读取 WebDAV 设备凭据失败: {error}"),
        }
        Some(settings)
    }
    #[cfg(not(target_os = "windows"))]
    Some(settings)
}

pub fn set_webdav_sync_settings(
    settings: Option<WebDavSyncSettings>,
    password_touched: bool,
) -> Result<(), AppError> {
    let delete_password = password_touched
        && settings
            .as_ref()
            .is_none_or(|settings| settings.password.is_empty());
    let mut next = get_settings();
    next.webdav_sync = settings;
    save_with_secret_policy(&mut next, delete_password)?;
    *store().write().unwrap_or_else(|error| error.into_inner()) = next;
    Ok(())
}

fn save_with_secret_policy(
    settings: &mut AppSettings,
    delete_webdav_password: bool,
) -> Result<(), AppError> {
    settings.normalize();
    #[cfg(target_os = "windows")]
    let previous_password = {
        use crate::ports::{DeviceSecretId, SecretStore};

        let secrets = crate::adapters::secret_store::WindowsCredentialStore::runtime();
        let previous = secrets
            .read(DeviceSecretId::WebdavPassword)
            .map_err(|error| AppError::Config(format!("读取 WebDAV 旧凭据失败: {error}")))?;
        if let Some(password) = settings
            .webdav_sync
            .as_ref()
            .map(|settings| settings.password.trim())
            .filter(|password| !password.is_empty())
        {
            secrets
                .write(DeviceSecretId::WebdavPassword, password)
                .map_err(|error| AppError::Config(format!("保存 WebDAV 凭据失败: {error}")))?;
        } else if delete_webdav_password {
            secrets
                .delete(DeviceSecretId::WebdavPassword)
                .map_err(|error| AppError::Config(format!("删除 WebDAV 凭据失败: {error}")))?;
        }
        previous
    };
    let _ = delete_webdav_password;
    if let Some(sync) = settings.webdav_sync.as_mut() {
        sync.password.clear();
    }
    let contents =
        serde_json::to_vec_pretty(settings).map_err(|source| AppError::JsonSerialize { source })?;
    let result = crate::config::atomic_write(&settings_path(), &contents);
    #[cfg(target_os = "windows")]
    return if let Err(error) = result {
        use crate::ports::{DeviceSecretId, SecretStore};

        let secrets = crate::adapters::secret_store::WindowsCredentialStore::runtime();
        let rollback = match previous_password {
            Some(password) => secrets.write(DeviceSecretId::WebdavPassword, &password),
            None => secrets.delete(DeviceSecretId::WebdavPassword),
        };
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(AppError::Config(format!(
                "{error}; WebDAV 凭据回滚失败: {rollback_error}"
            ))),
        };
    } else {
        Ok(())
    };
    #[cfg(not(target_os = "windows"))]
    result
}
