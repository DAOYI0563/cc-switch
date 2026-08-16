use tauri::State;

use crate::domain::LocalScanDomain;
use crate::services::local_scan::LocalScanRuntimeState;

/// Entering a managed page requests one immediate local-only domain scan.
#[tauri::command]
pub async fn local_scan_enter_page(
    state: State<'_, LocalScanRuntimeState>,
    domain: String,
) -> Result<(), String> {
    let domain = domain
        .parse::<LocalScanDomain>()
        .map_err(|error| error.to_string())?;
    state.enter_page(domain).map_err(|error| error.to_string())
}
