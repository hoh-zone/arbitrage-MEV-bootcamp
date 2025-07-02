use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::{debug, error, info};

pub fn start<T: Into<String>>(service_id: T, interval: Duration) -> JoinHandle<()> {
    let id = service_id.into();

    tokio::spawn(worker(id, interval))
}

async fn worker(id: String, interval: Duration) {
    info!("Heartbeat worker started for {}", id);

    // write your code here
}
