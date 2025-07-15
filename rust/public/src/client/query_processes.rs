use super::flightsql_client::Client;
use anyhow::Result;
use chrono::{DateTime, Utc};
use datafusion::arrow::array::RecordBatch;
use micromegas_analytics::time::TimeRange;

// use chrono::{DateTime, Utc};
/// A builder for querying processes.
pub struct ProcessQueryBuilder {
    filters: Vec<String>,
    begin: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
}

impl ProcessQueryBuilder {
    /// Creates a new `ProcessQueryBuilder`.
    ///
    /// Initializes an empty query builder with no filters.
    pub fn new() -> Self {
        Self {
            filters: vec![],
            begin: None,
            end: None,
        }
    }

    /// Adds a filter by process ID.
    pub fn with_process_id(mut self, process_id: &str) -> Self {
        self.filters.push(format!("(process_id='{process_id}')"));
        self
    }

    /// Adds a filter by username.
    pub fn with_username(mut self, username: &str) -> Self {
        self.filters
            .push(format!(r#"("processes.username"='{username}')"#));
        self
    }

    /// Adds a filter by executable name.
    pub fn with_exe(mut self, exe: &str) -> Self {
        self.filters.push(format!(r#"("processes.exe"='{exe}')"#));
        self
    }

    /// Sets the start time for the query.
    pub fn since(mut self, begin: DateTime<Utc>) -> Self {
        let iso = begin.to_rfc3339();
        self.filters.push(format!(r#"("end_time" >= '{iso}')"#));
        self.begin = Some(begin);
        self
    }

    /// Sets the end time for the query.
    pub fn until(mut self, end: DateTime<Utc>) -> Self {
        let iso = end.to_rfc3339();
        self.filters.push(format!(r#"("begin_time" <= '{iso}')"#));
        self.end = Some(end);
        self
    }

    /// Adds a filter to include only processes with CPU blocks.
    pub fn with_cpu_blocks(mut self) -> Self {
        self.filters
            .push(r#"array_has( "streams.tags", 'cpu' )"#.into());
        self
    }

    /// Adds a filter to include only processes with a specific thread name.
    pub fn with_thread_named(mut self, thread_name: &str) -> Self {
        let filter =
            format!(r#"property_get("streams.properties", 'thread-name') = '{thread_name}'"#);
        self.filters.push(filter);
        self
    }

    /// Builds the WHERE clause of the SQL query.
    pub fn into_where(&self) -> String {
        if self.filters.is_empty() {
            String::from("")
        } else {
            format!("WHERE {}", self.filters.join(" AND "))
        }
    }

    /// Executes the query and returns the results as a vector of `RecordBatch`.
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
        let mut query_time_range = None;
        if self.begin.is_some() && self.end.is_some() {
            query_time_range = Some(TimeRange::new(self.begin.unwrap(), self.end.unwrap()));
        }
        client.query(sql, query_time_range).await
    }
}
