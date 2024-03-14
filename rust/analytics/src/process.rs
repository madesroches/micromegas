
pub struct ProcessEntry{
    pub process_id: String,
    pub exe: String,
    pub username: String,
    pub realname: String,
    pub computer: String,
    pub distro: String,
    pub cpu_brand: String,
    pub tsc_frequency: i64,
    pub start_time: String,
    pub start_ticks: i64,
    pub parent_process_id: String,
}
