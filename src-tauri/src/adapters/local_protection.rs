use crate::ports::{
    LocalProtectionError, LocalProtectionErrorCode, LocalProtectionPurpose, LocalProtector,
};

#[cfg(target_os = "windows")]
const ENTROPY_PREFIX: &[u8] = b"com.zhldm.wsl-code-switch\0local-protection-v1\0";
#[cfg(target_os = "windows")]
const PLAINTEXT_FRAME: &[u8] = b"WSL_CODE_SWITCH_LOCAL_PROTECTION_V1\0";

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsDpapiProtector;

#[cfg(target_os = "windows")]
fn entropy_for(purpose: LocalProtectionPurpose) -> Vec<u8> {
    let mut entropy = Vec::with_capacity(ENTROPY_PREFIX.len() + purpose.as_str().len());
    entropy.extend_from_slice(ENTROPY_PREFIX);
    entropy.extend_from_slice(purpose.as_str().as_bytes());
    entropy
}

#[cfg(target_os = "windows")]
fn blob(
    bytes: &[u8],
) -> Result<windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB, LocalProtectionError> {
    use windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB;

    let length = u32::try_from(bytes.len()).map_err(|_| {
        LocalProtectionError::new(
            LocalProtectionErrorCode::InvalidInput,
            "local protection input exceeds the Windows DPAPI size limit",
        )
    })?;
    Ok(CRYPT_INTEGER_BLOB {
        cbData: length,
        pbData: bytes.as_ptr().cast_mut(),
    })
}

#[cfg(target_os = "windows")]
fn copy_and_free_output(
    output: windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB,
) -> Result<Vec<u8>, LocalProtectionError> {
    use windows_sys::Win32::Foundation::LocalFree;

    if output.cbData > 0 && output.pbData.is_null() {
        return Err(LocalProtectionError::new(
            LocalProtectionErrorCode::UnprotectFailed,
            "Windows DPAPI returned an invalid output buffer",
        ));
    }

    let copied = if output.cbData == 0 {
        Vec::new()
    } else {
        // SAFETY: DPAPI returned `pbData` with exactly `cbData` initialized bytes.
        // The allocation remains live until LocalFree below.
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
    };
    if !output.pbData.is_null() {
        // SAFETY: CryptProtectData/CryptUnprotectData allocate the output with LocalAlloc,
        // and ownership is transferred to this caller.
        unsafe {
            LocalFree(output.pbData.cast());
        }
    }
    Ok(copied)
}

#[cfg(target_os = "windows")]
fn windows_error(code: LocalProtectionErrorCode, action: &str) -> LocalProtectionError {
    let error = std::io::Error::last_os_error();
    LocalProtectionError::new(code, format!("Windows DPAPI failed to {action}: {error}"))
        .with_context(
            "osError",
            error.raw_os_error().unwrap_or_default().to_string(),
        )
}

impl LocalProtector for WindowsDpapiProtector {
    fn protect(
        &self,
        purpose: LocalProtectionPurpose,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, LocalProtectionError> {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Security::Cryptography::{
                CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
            };

            let mut framed = Vec::with_capacity(PLAINTEXT_FRAME.len() + plaintext.len());
            framed.extend_from_slice(PLAINTEXT_FRAME);
            framed.extend_from_slice(plaintext);
            let entropy = entropy_for(purpose);
            let input = blob(&framed)?;
            let entropy_blob = blob(&entropy)?;
            let mut output = CRYPT_INTEGER_BLOB::default();
            // SAFETY: all input buffers remain alive for the call, optional pointers are null,
            // and `output` is initialized for CryptProtectData to populate.
            let succeeded = unsafe {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    &entropy_blob,
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if succeeded == 0 {
                return Err(windows_error(
                    LocalProtectionErrorCode::ProtectFailed,
                    "protect data",
                ));
            }
            copy_and_free_output(output)
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = (purpose, plaintext);
            Err(LocalProtectionError::new(
                LocalProtectionErrorCode::UnsupportedPlatform,
                "Windows DPAPI is only available in the Windows build",
            ))
        }
    }

    fn unprotect(
        &self,
        purpose: LocalProtectionPurpose,
        protected: &[u8],
    ) -> Result<Vec<u8>, LocalProtectionError> {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Security::Cryptography::{
                CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
            };

            if protected.is_empty() {
                return Err(LocalProtectionError::new(
                    LocalProtectionErrorCode::InvalidInput,
                    "protected local data must not be empty",
                ));
            }
            let entropy = entropy_for(purpose);
            let input = blob(protected)?;
            let entropy_blob = blob(&entropy)?;
            let mut output = CRYPT_INTEGER_BLOB::default();
            // SAFETY: all input buffers remain alive for the call, optional pointers are null,
            // and `output` is initialized for CryptUnprotectData to populate.
            let succeeded = unsafe {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    &entropy_blob,
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if succeeded == 0 {
                return Err(windows_error(
                    LocalProtectionErrorCode::UnprotectFailed,
                    "unprotect data",
                ));
            }
            let framed = copy_and_free_output(output)?;
            let plaintext = framed.strip_prefix(PLAINTEXT_FRAME).ok_or_else(|| {
                LocalProtectionError::new(
                    LocalProtectionErrorCode::UnprotectFailed,
                    "protected local data has an invalid application frame",
                )
            })?;
            Ok(plaintext.to_vec())
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = (purpose, protected);
            Err(LocalProtectionError::new(
                LocalProtectionErrorCode::UnsupportedPlatform,
                "Windows DPAPI is only available in the Windows build",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn non_windows_build_fails_closed() {
        let error = WindowsDpapiProtector
            .protect(LocalProtectionPurpose::TemporaryRollback, b"payload")
            .unwrap_err();
        assert_eq!(error.code, LocalProtectionErrorCode::UnsupportedPlatform);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dpapi_is_user_scoped_entropy_bound_and_tamper_evident() {
        let protector = WindowsDpapiProtector;
        let plaintext = b"wsl-code-switch-dpapi-plaintext-marker-73f8";
        let protected = protector
            .protect(LocalProtectionPurpose::TemporaryRollback, plaintext)
            .expect("protect with current Windows user");

        assert_ne!(protected, plaintext);
        assert!(!protected
            .windows(plaintext.len())
            .any(|window| window == plaintext));
        assert_eq!(
            protector
                .unprotect(LocalProtectionPurpose::TemporaryRollback, &protected)
                .expect("unprotect with the same user and entropy"),
            plaintext
        );
        assert_eq!(
            protector
                .unprotect(LocalProtectionPurpose::DailyBriefCheckpoint, &protected)
                .unwrap_err()
                .code,
            LocalProtectionErrorCode::UnprotectFailed
        );

        let mut tampered = protected;
        let middle = tampered.len() / 2;
        tampered[middle] ^= 0x80;
        assert_eq!(
            protector
                .unprotect(LocalProtectionPurpose::TemporaryRollback, &tampered)
                .unwrap_err()
                .code,
            LocalProtectionErrorCode::UnprotectFailed
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn dpapi_roundtrips_an_empty_payload() {
        let protected = WindowsDpapiProtector
            .protect(LocalProtectionPurpose::TemporaryRollback, b"")
            .unwrap();
        assert_eq!(
            WindowsDpapiProtector
                .unprotect(LocalProtectionPurpose::TemporaryRollback, &protected)
                .unwrap(),
            b""
        );
    }
}
