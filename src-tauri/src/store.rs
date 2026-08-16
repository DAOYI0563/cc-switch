use crate::database::Database;
use crate::services::LocalScanWriteTracker;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub local_scan_writes: Arc<LocalScanWriteTracker>,
}

impl AppState {
    pub fn new(db: Arc<Database>) -> Self {
        let local_scan_writes = Arc::new(LocalScanWriteTracker::default());
        Self {
            db,
            local_scan_writes,
        }
    }
}
