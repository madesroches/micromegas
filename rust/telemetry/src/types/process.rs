#[derive(Clone, PartialEq)]
pub struct Process {
    pub process_id: String,
    pub exe: String,
    pub username: String,
    pub realname: String,
    pub computer: String,
    pub distro: String,
    pub cpu_brand: String,
    pub tsc_frequency: u64,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub start_ticks: i64,
    pub parent_process_id: String,
}

impl From<crate::api::components::Process> for Process {
    fn from(process: crate::api::components::Process) -> Self {
        Process {
            process_id: process.process_id,
            exe: process.exe,
            username: process.username,
            realname: process.realname,
            computer: process.computer,
            distro: process.distro,
            cpu_brand: process.cpu_brand,
            tsc_frequency: process.tsc_frequency,
            start_time: process.start_time,
            start_ticks: process.start_ticks,
            parent_process_id: process.parent_process_id,
        }
    }
}

impl From<Process> for crate::api::components::Process {
    fn from(process: Process) -> Self {
        crate::api::components::Process {
            process_id: process.process_id,
            exe: process.exe,
            username: process.username,
            realname: process.realname,
            computer: process.computer,
            distro: process.distro,
            cpu_brand: process.cpu_brand,
            tsc_frequency: process.tsc_frequency,
            start_time: process.start_time,
            start_ticks: process.start_ticks,
            parent_process_id: process.parent_process_id,
        }
    }
}
