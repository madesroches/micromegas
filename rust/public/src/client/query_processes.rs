use super::flightsql_client::Client;
use anyhow::Result;
use chrono::{DateTime, Utc};
use datafusion::arrow::array::RecordBatch;
use micromegas_analytics::time::TimeRange;

// use chrono::{DateTime, Utc};
pub struct ProcessQueryBuilder {
    filters: Vec<String>,
    begin: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
}

impl ProcessQueryBuilder {
    pub fn new() -> Self {
        Self {
            filters: vec![],
            begin: None,
            end: None,
        }
    }

    pub fn with_process_id(mut self, process_id: &str) -> Self {
        self.filters.push(format!("(process_id='{process_id}')"));
        self
    }

    pub fn with_username(mut self, username: &str) -> Self {
        self.filters
            .push(format!(r#"("processes.username"='{username}')"#));
        self
    }

    pub fn with_exe(mut self, exe: &str) -> Self {
        self.filters.push(format!(r#"("processes.exe"='{exe}')"#));
        self
    }

    pub fn since(mut self, begin: DateTime<Utc>) -> Self {
        let iso = begin.to_rfc3339();
        self.filters.push(format!(r#"("end_time" >= '{iso}')"#));
        self.begin = Some(begin);
        self
    }

    pub fn until(mut self, end: DateTime<Utc>) -> Self {
        let iso = end.to_rfc3339();
        self.filters.push(format!(r#"("begin_time" <= '{iso}')"#));
        self.end = Some(end);
        self
    }

    pub fn with_cpu_blocks(mut self) -> Self {
        self.filters
            .push(r#"array_has( "streams.tags", 'cpu' )"#.into());
        self
    }

    pub fn with_thread_named(mut self, thread_name: &str) -> Self {
        let filter =
            format!(r#"property_get("streams.properties", 'thread-name') = '{thread_name}'"#);
        self.filters.push(filter);
        self
    }

    pub fn into_where(&self) -> String {
        if self.filters.is_empty() {
            String::from("")
        } else {
            format!("WHERE {}", self.filters.join(" AND "))
        }
    }

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
