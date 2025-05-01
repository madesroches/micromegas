// use chrono::{DateTime, Utc};
pub struct ProcessQueryBuilder {
    filters: Vec<String>,
}

impl ProcessQueryBuilder {
    pub fn new() -> Self {
        Self { filters: vec![] }
    }

    pub fn with_process_id(mut self, process_id: &str) -> Self {
        self.filters.push(format!("(process_id='{process_id}')"));
        self
    }

    pub fn with_cpu_blocks(mut self) -> Self {
        self.filters
            .push(r#"array_has( "streams.tags", 'cpu' )"#.into());
        self
    }

    pub fn into_where(&self) -> String {
        if self.filters.is_empty() {
            String::from("")
        } else {
            format!("WHERE {}", self.filters.join(" AND "))
        }
    }

    pub fn build(self) -> String {
        let sql_where = self.into_where();
        let sql = format!(
            "SELECT process_id, min(begin_time) as begin, max(end_time) as end
            FROM blocks
            {sql_where}
            GROUP BY process_id;"
        );
        sql
    }
}
