//! Shared HTTP client for retained direct-network product capabilities.

use once_cell::sync::OnceCell;
use reqwest::Client;
use std::time::Duration;

static RETAINED_CLIENT: OnceCell<Result<Client, String>> = OnceCell::new();

/// Return the process-wide client used by retained direct network queries.
///
/// This adapter deliberately has no dependency on the deleted local-proxy domain and disables
/// proxy discovery so retained provider traffic always follows the native direct-connect path.
pub fn get() -> Result<Client, String> {
    RETAINED_CLIENT
        .get_or_init(|| {
            Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(600))
                .connect_timeout(Duration::from_secs(30))
                .pool_max_idle_per_host(10)
                .tcp_keepalive(Duration::from_secs(60))
                .build()
                .map_err(|error| format!("Failed to build provider HTTP client: {error}"))
        })
        .clone()
}
