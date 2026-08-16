use std::sync::Arc;

use tauri::State;

use crate::adapters::local_conflict_resolution::RuntimeLocalConflictResolution;
use crate::adapters::local_reconciliation_state::DatabaseLocalReconciliationStateAdapter;
use crate::adapters::temporary_rollback::FixedTemporaryRollbackStore;
use crate::domain::{ConflictCenterItem, ConflictResolutionRequest};
use crate::ports::ConflictCenterError;
use crate::services::{
    list_conflict_center_items, resolve_conflict_center_item, ConflictCenterRuntimeState,
    LocalScanConflictSource,
};
use crate::store::AppState;

#[tauri::command]
pub fn list_conflict_center_items_command(
    app_state: State<'_, AppState>,
    conflict_state: State<'_, ConflictCenterRuntimeState>,
) -> Result<Vec<ConflictCenterItem>, ConflictCenterError> {
    let baselines = conflict_state.baselines();
    let states = Arc::new(DatabaseLocalReconciliationStateAdapter::new(
        app_state.db.clone(),
        baselines.clone(),
    ));
    let source = LocalScanConflictSource::new(conflict_state.coordinator(), states);
    let resolver = RuntimeLocalConflictResolution::new(
        app_state.inner(),
        conflict_state.coordinator(),
        baselines,
    );
    let webdav = conflict_state.webdav();
    list_conflict_center_items(&[&source, webdav.as_ref()], &resolver)
}

#[tauri::command]
pub fn resolve_conflict_center_item_command(
    request: ConflictResolutionRequest,
    app_state: State<'_, AppState>,
    conflict_state: State<'_, ConflictCenterRuntimeState>,
) -> Result<(), ConflictCenterError> {
    let _guard = conflict_state.lock_resolution();
    let baselines = conflict_state.baselines();
    let states = Arc::new(DatabaseLocalReconciliationStateAdapter::new(
        app_state.db.clone(),
        baselines.clone(),
    ));
    let source = LocalScanConflictSource::new(conflict_state.coordinator(), states);
    let resolver = RuntimeLocalConflictResolution::new(
        app_state.inner(),
        conflict_state.coordinator(),
        baselines,
    );
    let webdav = conflict_state.webdav();
    let now_ms = chrono::Utc::now().timestamp_millis();
    resolve_conflict_center_item(
        &[&source, webdav.as_ref()],
        &resolver,
        &FixedTemporaryRollbackStore::runtime(),
        now_ms,
        &request,
    )
}
