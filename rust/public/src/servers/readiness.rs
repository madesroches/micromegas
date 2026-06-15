use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Shared readiness probe used by the FlightSQL HTTP sidecar.
///
/// Probes DB + blob storage under a 2 s timeout and caches success for 1 s
/// to avoid amplifying load under rapid ALB polling.
pub struct ReadinessProbe {
    lake: Arc<DataLakeConnection>,
    ready_ok_until: Mutex<Option<Instant>>,
}

impl ReadinessProbe {
    pub fn new(lake: Arc<DataLakeConnection>) -> Self {
        Self {
            lake,
            ready_ok_until: Mutex::new(None),
        }
    }

    pub async fn check_ready(&self) -> bool {
        let now = Instant::now();
        {
            let guard = self.ready_ok_until.lock().expect("readiness cache lock");
            if let Some(ok_until) = *guard
                && ok_until > now
            {
                return true;
            }
        }

        let probe_db = sqlx::query("SELECT 1").execute(&self.lake.db_pool);
        let probe_blob = self.lake.blob_storage.probe();

        let result = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(probe_db, probe_blob)
        })
        .await;

        match result {
            Ok((Ok(_), Ok(()))) => {
                let mut guard = self.ready_ok_until.lock().expect("readiness cache lock");
                *guard = Some(Instant::now() + Duration::from_secs(1));
                true
            }
            _ => {
                let mut guard = self.ready_ok_until.lock().expect("readiness cache lock");
                *guard = None;
                false
            }
        }
    }
}
