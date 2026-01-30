use super::flightsql_client::Client;
use anyhow::Result;
use chrono::{DateTime, Utc};
use datafusion::arrow::array::RecordBatch;
use micromegas_analytics::time::TimeRange;

/// Builder for creating process queries with various filters.
///
/// This builder allows you to construct queries to find processes based on
/// various criteria like process ID, username, executable name, time ranges,
/// and whether they contain CPU blocks or specific thread names.
pub struct ProcessQueryBuilder {
    filters: Vec<String>,
    begin: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
}

impl ProcessQueryBuilder {
    /// Creates a new ProcessQueryBuilder with no filters.
    pub fn new() -> Self {
        Self {
            filters: vec![],
            begin: None,
            end: None,
        }
    }

    /// Filters processes by exact process ID.
    pub fn with_process_id(mut self, process_id: &str) -> Self {
        self.filters.push(format!("(process_id='{process_id}')"));
        self
    }

    /// Filters processes by exact username.
    pub fn with_username(mut self, username: &str) -> Self {
        self.filters
            .push(format!(r#"("processes.username"='{username}')"#));
        self
    }

    /// Filters processes by exact executable name.
    pub fn with_exe(mut self, exe: &str) -> Self {
        self.filters.push(format!(r#"("processes.exe"='{exe}')"#));
        self
    }

    /// Filters processes that ended at or after the given time.
    pub fn since(mut self, begin: DateTime<Utc>) -> Self {
        let iso = begin.to_rfc3339();
        self.filters.push(format!(r#"("end_time" >= '{iso}')"#));
        self.begin = Some(begin);
        self
    }

    /// Filters processes that began at or before the given time.
    pub fn until(mut self, end: DateTime<Utc>) -> Self {
        let iso = end.to_rfc3339();
        self.filters.push(format!(r#"("begin_time" <= '{iso}')"#));
        self.end = Some(end);
        self
    }

    /// Filters processes that have CPU blocks (contain telemetry streams tagged with 'cpu').
    pub fn with_cpu_blocks(mut self) -> Self {
        self.filters
            .push(r#"array_has( "streams.tags", 'cpu' )"#.into());
        self
    }

    /// Filters processes that have a thread with the specified name.
    pub fn with_thread_named(mut self, thread_name: &str) -> Self {
        let filter =
            format!(r#"property_get("streams.properties", 'thread-name') = '{thread_name}'"#);
        self.filters.push(filter);
        self
    }

    /// Converts the filters into a SQL WHERE clause.
    pub fn into_where(&self) -> String {
        if self.filters.is_empty() {
            String::from("")
        } else {
            format!("WHERE {}", self.filters.join(" AND "))
        }
    }

    /// Executes the query and returns the matching processes.
    ///
    /// Returns a vector of RecordBatch containing process information including:
    /// - process_id
    /// - begin/end times
    /// - exe (executable name)
    /// - properties
    /// - computer
    /// - username
    /// - cpu_brand
    /// - distro
    pub async fn query(self, client: &mut Client) -> Result<Vec<RecordBatch>> {
        let sql_where = self.into_where();
        let sql = format!(
            r#"SELECT process_id,
                      min(begin_time) as begin,
                      max(end_time) as end,
                      "processes.exe" as exe,
                      "processes.properties" as properties,
                      "processes.computer" as computer,
                      "processes.username" as username,
                      "processes.cpu_brand" as cpu_brand,
                      "processes.distro" as distro
            FROM blocks
            {sql_where}
            GROUP BY process_id, exe, properties, computer, username, cpu_brand, distro;"#
        );
        let query_time_range = if let (Some(begin), Some(end)) = (self.begin, self.end) {
            Some(TimeRange::new(begin, end))
        } else {
            None
        };
        client.query(sql, query_time_range).await
    }
}

impl Default for ProcessQueryBuilder {
    fn default() -> Self {
        Self::new()
    }
}
