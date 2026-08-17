//! Stable boundaries between application policy and infrastructure adapters.

mod ai_summary;
mod conflict_center;
mod device_settings;
mod legacy_data;
mod live_provider_config;
mod local_protection;
mod local_scan;
mod local_skill;
mod retained_migration;
mod secret_store;
mod sync_crypto;
mod sync_transport;
mod temporary_rollback;
mod wsl_files;
mod wsl_paths;

pub use ai_summary::{
    AiSummaryClient, AiSummaryError, AiSummaryErrorCode, AiSummaryFuture, AiSummaryRequest,
};
pub use conflict_center::{
    ConflictCenterError, ConflictCenterErrorCode, ConflictCenterResolutionPort,
    ConflictCenterSourcePort, SyncLocalApplyPort,
};
pub use device_settings::{DeviceSettingsError, DeviceSettingsErrorCode, DeviceSettingsStore};
pub use legacy_data::{LegacyDataError, LegacyDataErrorCode, LegacyDataSource};
pub use live_provider_config::{
    LiveProviderConfigError, LiveProviderConfigErrorCode, LiveProviderConfigOperation,
    LiveProviderConfigPort, LiveProviderRecord, LiveProviderSnapshot,
};
pub use local_protection::{
    LocalProtectionError, LocalProtectionErrorCode, LocalProtectionPurpose, LocalProtector,
};
pub use local_scan::{
    LocalReconciliationBaselinePort, LocalReconciliationState, LocalReconciliationStatePort,
    LocalScanFirstObservation, LocalScanParsedRecord, LocalScanParsedSnapshot, LocalScanParserPort,
    LocalScanReadFailure, LocalScanSummaryPort, ManagedSkillInventoryPort,
};
pub use local_skill::{
    LocalSkillDirectoryCandidate, LocalSkillFile, LocalSkillLiveCandidate, LocalSkillRepository,
    LocalSkillRepositoryError, LocalSkillTree, LocalSkillTreeError, LocalSkillTreeErrorCode,
    LocalSkillTreePort, LocalSkillTreeSnapshot,
};
pub use retained_migration::{RetainedMigrationTarget, RetainedMigrationTargetError};
pub use secret_store::{DeviceSecretId, SecretStore, SecretStoreError, SecretStoreErrorCode};
pub use sync_crypto::{
    SyncCryptoError, SyncCryptoErrorCode, SyncCryptoPort, SyncCryptoRandom, SyncCryptoSession,
    SyncPlaintext,
};
pub use sync_transport::{
    SyncTransportError, SyncTransportErrorCode, SyncTransportFuture, SyncTransportPort,
    MAX_SYNC_REMOTE_OBJECT_BYTES,
};
pub use temporary_rollback::{
    TemporaryRollbackError, TemporaryRollbackErrorCode, TemporaryRollbackStore,
};
pub use wsl_files::{WslFileError, WslFileErrorCode, WslFileSystem};
pub use wsl_paths::{
    WslPathAccess, WslPathError, WslPathErrorCode, WslPathGuard, WslPathPair, WslPathResolver,
    WslPathScope,
};
