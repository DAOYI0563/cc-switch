use crate::adapters::wsl_files::WslFileAdapter;
use crate::domain::{prompt_live_filename, ManagedClientId};
use crate::error::AppError;
use crate::ports::{WslFileSystem, WslPathScope};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptLiveFileSnapshot {
    pub client: ManagedClientId,
    pub contents: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
pub struct PromptLiveFileAdapter {
    files: WslFileAdapter,
}

impl PromptLiveFileAdapter {
    pub fn runtime() -> Self {
        Self {
            files: WslFileAdapter::runtime(),
        }
    }

    pub fn capture(&self, client: ManagedClientId) -> Result<PromptLiveFileSnapshot, AppError> {
        let contents = self
            .files
            .read_optional(scope(client), prompt_live_filename(client))
            .map_err(prompt_file_error)?;
        Ok(PromptLiveFileSnapshot { client, contents })
    }

    pub fn read_text(&self, client: ManagedClientId) -> Result<Option<String>, AppError> {
        self.capture(client)?
            .contents
            .map(|contents| {
                String::from_utf8(contents).map_err(|error| {
                    AppError::InvalidInput(format!(
                        "{} 不是有效 UTF-8 文本: {error}",
                        prompt_live_filename(client)
                    ))
                })
            })
            .transpose()
    }

    pub fn write_text(&self, client: ManagedClientId, content: &str) -> Result<(), AppError> {
        self.files
            .atomic_write(
                scope(client),
                prompt_live_filename(client),
                content.as_bytes(),
            )
            .map_err(prompt_file_error)
    }

    pub fn restore(&self, snapshot: &PromptLiveFileSnapshot) -> Result<(), AppError> {
        match &snapshot.contents {
            Some(contents) => self
                .files
                .atomic_write(
                    scope(snapshot.client),
                    prompt_live_filename(snapshot.client),
                    contents,
                )
                .map_err(prompt_file_error),
            None => self
                .files
                .remove_file(
                    scope(snapshot.client),
                    prompt_live_filename(snapshot.client),
                )
                .map_err(prompt_file_error),
        }
    }
}

const fn scope(client: ManagedClientId) -> WslPathScope {
    WslPathScope::ClientConfig(client)
}

fn prompt_file_error(error: impl std::fmt::Display) -> AppError {
    AppError::Message(format!("Prompt live 文件访问失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct TestHomeGuard(Option<std::ffi::OsString>);

    impl TestHomeGuard {
        #[allow(deprecated)]
        fn set(home: &std::path::Path) -> Self {
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", home);
            Self(previous)
        }
    }

    impl Drop for TestHomeGuard {
        #[allow(deprecated)]
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var("CC_SWITCH_TEST_HOME", previous),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    #[test]
    #[serial]
    fn capture_write_and_restore_preserve_exact_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let adapter = PromptLiveFileAdapter::runtime();
        let path = temp.path().join(".claude/CLAUDE.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"original\r\nbytes").unwrap();

        let snapshot = adapter.capture(ManagedClientId::Claude).unwrap();
        adapter
            .write_text(ManagedClientId::Claude, "replacement\n")
            .unwrap();
        adapter.restore(&snapshot).unwrap();

        assert_eq!(std::fs::read(path).unwrap(), b"original\r\nbytes");
    }
}
