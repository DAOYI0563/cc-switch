use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::{
    LegacyIgnoredCounts, LegacyMigrationPreview, LegacyMigrationStatus, LegacyRetainedCounts,
};
use crate::ports::{LegacyDataError, LegacyDataErrorCode, LegacyDataSource};

mod files;
mod json;
mod sqlite;

const LEGACY_DIRECTORY: &str = ".cc-switch";
const DATABASE_FILE: &str = "cc-switch.db";
const CONFIG_FILE: &str = "config.json";
const SKILLS_FILE: &str = "skills.json";
const SETTINGS_FILE: &str = "settings.json";
const LEGACY_MAX_DATABASE_VERSION: i32 = 16;
const LEGACY_MAX_JSON_VERSION: u32 = 2;
const MAX_JSON_BYTES: u64 = 64 * 1024 * 1024;
const KNOWN_FILES: &[&str] = &[
    DATABASE_FILE,
    "cc-switch.db-journal",
    "cc-switch.db-shm",
    "cc-switch.db-wal",
    CONFIG_FILE,
    SKILLS_FILE,
    SETTINGS_FILE,
];

#[derive(Debug, Clone)]
pub struct FixedLegacyDataSource {
    root: PathBuf,
}

impl FixedLegacyDataSource {
    pub fn runtime() -> Self {
        Self::from_home(crate::config::get_home_dir())
    }

    pub fn from_home(home: impl AsRef<Path>) -> Self {
        Self {
            root: home.as_ref().join(LEGACY_DIRECTORY),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl LegacyDataSource for FixedLegacyDataSource {
    fn preview(&self) -> Result<LegacyMigrationPreview, LegacyDataError> {
        if !files::path_exists_without_following(&self.root)? {
            return Ok(empty_preview(LegacyMigrationStatus::NotFound));
        }
        files::inspect_no_links(&self.root)?;
        let root_metadata = fs::symlink_metadata(&self.root).map_err(|error| {
            files::inspection_error(&self.root, "inspect legacy directory", error)
        })?;
        if !root_metadata.is_dir() {
            return Err(LegacyDataError::new(
                LegacyDataErrorCode::InspectionFailed,
                "legacy source must be a directory",
            )
            .with_context("path", self.root.display().to_string()));
        }

        files::reject_pending_database_changes(&self.root)?;
        let before = files::collect_known_files(&self.root)?;
        let database = self.root.join(DATABASE_FILE);
        let config = self.root.join(CONFIG_FILE);

        let mut preview = if before.contains_key(DATABASE_FILE) {
            sqlite::preview_database(&database)?
        } else if before.contains_key(CONFIG_FILE) {
            json::preview_json(&config, self.root.join(SKILLS_FILE).as_path())?
        } else {
            empty_preview(LegacyMigrationStatus::Empty)
        };

        let after = files::collect_known_files(&self.root)?;
        if before != after {
            return Err(LegacyDataError::new(
                LegacyDataErrorCode::SourceChanged,
                "legacy source changed while it was being inspected",
            )
            .with_context("path", self.root.display().to_string()));
        }

        preview.files = after.values().cloned().collect();
        preview.directory_fingerprint = Some(files::directory_fingerprint(&preview.files));
        Ok(preview)
    }

    fn load_retained(
        &self,
        expected_fingerprint: &str,
    ) -> Result<crate::domain::LegacyRetainedSnapshot, LegacyDataError> {
        let before = self.preview()?;
        let actual_fingerprint = before.directory_fingerprint.as_deref().ok_or_else(|| {
            LegacyDataError::new(
                LegacyDataErrorCode::InvalidRecord,
                "legacy source is not ready for retained-data migration",
            )
        })?;
        if actual_fingerprint != expected_fingerprint {
            return Err(LegacyDataError::new(
                LegacyDataErrorCode::SourceChanged,
                "legacy source fingerprint changed before migration",
            ));
        }

        let settings_path = self.root.join(SETTINGS_FILE);
        let legacy_settings_json = if files::path_exists_without_following(&settings_path)? {
            Some(json::read_json_document(&settings_path)?)
        } else {
            None
        };
        let mut snapshot = match before.source {
            Some(crate::domain::LegacySourceKind::Sqlite) => {
                sqlite::load_retained_database(&self.root.join(DATABASE_FILE), actual_fingerprint)?
            }
            Some(crate::domain::LegacySourceKind::Json) => json::load_retained_json(
                &self.root.join(CONFIG_FILE),
                &self.root.join(SKILLS_FILE),
                actual_fingerprint,
            )?,
            None => {
                return Err(LegacyDataError::new(
                    LegacyDataErrorCode::InvalidRecord,
                    "legacy source contains no migratable configuration",
                ));
            }
        };
        snapshot.legacy_settings_json = legacy_settings_json;

        let after = self.preview()?;
        if after.directory_fingerprint.as_deref() != Some(expected_fingerprint) {
            return Err(LegacyDataError::new(
                LegacyDataErrorCode::SourceChanged,
                "legacy source changed while retained records were loaded",
            ));
        }
        if snapshot.counts() != before.retained {
            return Err(LegacyDataError::new(
                LegacyDataErrorCode::InvalidRecord,
                "retained record counts do not match the read-only preview",
            ));
        }
        Ok(snapshot)
    }
}

fn empty_preview(status: LegacyMigrationStatus) -> LegacyMigrationPreview {
    LegacyMigrationPreview {
        status,
        source: None,
        source_version: None,
        retained: LegacyRetainedCounts::default(),
        ignored: LegacyIgnoredCounts::default(),
        files: Vec::new(),
        directory_fingerprint: None,
    }
}

#[cfg(test)]
mod tests;
