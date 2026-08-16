use crate::ports::{DeviceSecretId, SecretStore, SecretStoreError, SecretStoreErrorCode};

#[cfg(all(test, target_os = "windows"))]
fn test_runtime_secrets(
) -> &'static std::sync::Mutex<std::collections::HashMap<DeviceSecretId, String>> {
    static SECRETS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<DeviceSecretId, String>>,
    > = std::sync::OnceLock::new();
    SECRETS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(target_os = "windows")]
const CREDENTIAL_USERNAME: &str = "WSL Code Switch";

#[derive(Debug, Clone, Default)]
pub struct WindowsCredentialStore {
    #[cfg(all(test, target_os = "windows"))]
    test_namespace: Option<String>,
}

impl WindowsCredentialStore {
    pub fn runtime() -> Self {
        Self::default()
    }

    #[cfg(target_os = "windows")]
    fn target_name(&self, id: DeviceSecretId) -> String {
        #[cfg(test)]
        if let Some(namespace) = &self.test_namespace {
            return format!("{}#test/{namespace}", id.target_name());
        }
        id.target_name().to_string()
    }

    #[cfg(all(test, target_os = "windows"))]
    pub(crate) fn isolated(namespace: &str) -> Self {
        assert!(
            !namespace.is_empty()
                && namespace
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "test credential namespace must be a safe fixed component"
        );
        Self {
            test_namespace: Some(namespace.to_string()),
        }
    }
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_error(code: SecretStoreErrorCode, action: &str, id: DeviceSecretId) -> SecretStoreError {
    let error = std::io::Error::last_os_error();
    SecretStoreError::new(
        code,
        format!("Windows Credential Manager failed to {action}: {error}"),
    )
    .with_context("secretId", format!("{id:?}"))
    .with_context(
        "osError",
        error.raw_os_error().unwrap_or_default().to_string(),
    )
}

impl SecretStore for WindowsCredentialStore {
    fn read(&self, id: DeviceSecretId) -> Result<Option<String>, SecretStoreError> {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
            use windows_sys::Win32::Security::Credentials::{
                CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
            };

            #[cfg(test)]
            if self.test_namespace.is_none() {
                return Ok(test_runtime_secrets()
                    .lock()
                    .expect("test runtime secret store lock")
                    .get(&id)
                    .cloned());
            }

            let target_name = self.target_name(id);
            let target = wide_null(&target_name);
            let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: `target` is NUL-terminated and lives through the call; on success
            // Credential Manager initializes `credential` and transfers a CredFree buffer.
            let succeeded =
                unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
            if succeeded == 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                    return Ok(None);
                }
                return Err(windows_error(
                    SecretStoreErrorCode::ReadFailed,
                    "read secret",
                    id,
                ));
            }
            if credential.is_null() {
                return Err(SecretStoreError::new(
                    SecretStoreErrorCode::InvalidStoredValue,
                    "Windows Credential Manager returned an invalid credential buffer",
                ));
            }

            // SAFETY: the successful CredReadW call returned a valid CREDENTIALW that
            // remains allocated until CredFree below.
            let stored = unsafe { &*credential };
            let bytes = if stored.CredentialBlobSize == 0 {
                &[][..]
            } else if stored.CredentialBlob.is_null() {
                // SAFETY: the top-level credential is still owned here.
                unsafe { CredFree(credential.cast()) };
                return Err(SecretStoreError::new(
                    SecretStoreErrorCode::InvalidStoredValue,
                    "Windows Credential Manager returned an invalid secret buffer",
                ));
            } else {
                // SAFETY: CREDENTIALW declares CredentialBlobSize initialized bytes.
                unsafe {
                    std::slice::from_raw_parts(
                        stored.CredentialBlob,
                        stored.CredentialBlobSize as usize,
                    )
                }
            };
            let value = String::from_utf8(bytes.to_vec()).map_err(|_| {
                SecretStoreError::new(
                    SecretStoreErrorCode::InvalidStoredValue,
                    "stored device secret is not valid UTF-8",
                )
            });
            // SAFETY: ownership of the successful CredReadW allocation ends here.
            unsafe { CredFree(credential.cast()) };
            value.map(Some)
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = id;
            Err(SecretStoreError::new(
                SecretStoreErrorCode::UnsupportedPlatform,
                "Windows Credential Manager is only available in the Windows build",
            ))
        }
    }

    fn write(&self, id: DeviceSecretId, secret: &str) -> Result<(), SecretStoreError> {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Security::Credentials::{
                CredWriteW, CREDENTIALW, CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE,
                CRED_TYPE_GENERIC,
            };

            if secret.is_empty() {
                return Err(SecretStoreError::new(
                    SecretStoreErrorCode::InvalidSecret,
                    "device secret must not be empty; delete it explicitly instead",
                ));
            }
            let bytes = secret.as_bytes();
            if bytes.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
                return Err(SecretStoreError::new(
                    SecretStoreErrorCode::SecretTooLarge,
                    "device secret exceeds the Windows Credential Manager size limit",
                ));
            }
            #[cfg(test)]
            if self.test_namespace.is_none() {
                test_runtime_secrets()
                    .lock()
                    .expect("test runtime secret store lock")
                    .insert(id, secret.to_string());
                return Ok(());
            }
            let target_name = self.target_name(id);
            let target = wide_null(&target_name);
            let username = wide_null(CREDENTIAL_USERNAME);
            let credential = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                TargetName: target.as_ptr().cast_mut(),
                CredentialBlobSize: bytes.len() as u32,
                CredentialBlob: bytes.as_ptr().cast_mut(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                UserName: username.as_ptr().cast_mut(),
                ..CREDENTIALW::default()
            };
            // SAFETY: all pointers refer to initialized buffers that live through the call.
            // CredWriteW copies the credential data before returning.
            let succeeded = unsafe { CredWriteW(&credential, 0) };
            if succeeded == 0 {
                return Err(windows_error(
                    SecretStoreErrorCode::WriteFailed,
                    "write secret",
                    id,
                ));
            }
            Ok(())
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = (id, secret);
            Err(SecretStoreError::new(
                SecretStoreErrorCode::UnsupportedPlatform,
                "Windows Credential Manager is only available in the Windows build",
            ))
        }
    }

    fn delete(&self, id: DeviceSecretId) -> Result<(), SecretStoreError> {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
            use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};

            #[cfg(test)]
            if self.test_namespace.is_none() {
                test_runtime_secrets()
                    .lock()
                    .expect("test runtime secret store lock")
                    .remove(&id);
                return Ok(());
            }

            let target_name = self.target_name(id);
            let target = wide_null(&target_name);
            // SAFETY: `target` is NUL-terminated and remains alive through the call.
            let succeeded = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
            if succeeded == 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                    return Ok(());
                }
                return Err(windows_error(
                    SecretStoreErrorCode::DeleteFailed,
                    "delete secret",
                    id,
                ));
            }
            Ok(())
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = id;
            Err(SecretStoreError::new(
                SecretStoreErrorCode::UnsupportedPlatform,
                "Windows Credential Manager is only available in the Windows build",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_ids_map_to_three_fixed_unique_targets() {
        let targets = DeviceSecretId::ALL.map(DeviceSecretId::target_name);
        assert_eq!(targets.len(), 3);
        assert!(targets
            .iter()
            .all(|target| target.starts_with("com.zhldm.wsl-code-switch/")));
        assert_ne!(targets[0], targets[1]);
        assert_ne!(targets[0], targets[2]);
        assert_ne!(targets[1], targets[2]);
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn non_windows_build_fails_closed() {
        let error = WindowsCredentialStore::runtime()
            .read(DeviceSecretId::WebdavPassword)
            .unwrap_err();
        assert_eq!(error.code, SecretStoreErrorCode::UnsupportedPlatform);
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[serial_test::serial]
    fn credential_manager_roundtrips_overwrites_and_deletes_all_owned_secrets() {
        let store = WindowsCredentialStore::isolated("roundtrip");
        for id in DeviceSecretId::ALL {
            store.delete(id).expect("clean stale test secret");
            assert_eq!(store.read(id).expect("read missing secret"), None);
            let first = format!("first-{id:?}-密钥");
            store.write(id, &first).expect("write secret");
            assert_eq!(
                store.read(id).expect("read secret").as_deref(),
                Some(first.as_str())
            );
            let second = format!("second-{id:?}");
            store.write(id, &second).expect("overwrite secret");
            assert_eq!(
                store.read(id).expect("read overwritten secret").as_deref(),
                Some(second.as_str())
            );
            store.delete(id).expect("delete test secret");
            assert_eq!(store.read(id).expect("read deleted secret"), None);
            store
                .delete(id)
                .expect("delete missing secret is idempotent");
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[serial_test::serial]
    fn credential_manager_rejects_empty_and_oversized_values_without_overwrite() {
        use windows_sys::Win32::Security::Credentials::CRED_MAX_CREDENTIAL_BLOB_SIZE;

        let store = WindowsCredentialStore::isolated("validation");
        let id = DeviceSecretId::WebdavPassword;
        store.delete(id).unwrap();
        store.write(id, "preserved").unwrap();
        assert_eq!(
            store.write(id, "").unwrap_err().code,
            SecretStoreErrorCode::InvalidSecret
        );
        let oversized = "x".repeat(CRED_MAX_CREDENTIAL_BLOB_SIZE as usize + 1);
        assert_eq!(
            store.write(id, &oversized).unwrap_err().code,
            SecretStoreErrorCode::SecretTooLarge
        );
        assert_eq!(store.read(id).unwrap().as_deref(), Some("preserved"));
        store.delete(id).unwrap();
    }
}
