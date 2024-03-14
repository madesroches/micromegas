use micromegas_analytics::log_entry::LogEntry;
use micromegas_tracing::Level;

pub(crate) trait Searchable<Needle> {
    fn matches(&self, needle: Needle) -> bool;
}

impl Searchable<Level> for LogEntry {
    fn matches(&self, level: Level) -> bool {
        self.level() <= level
    }
}

impl<'a> Searchable<&'a [String]> for LogEntry {
    fn matches(&self, needles: &'a [String]) -> bool {
        for needle in needles {
            if self.target.to_lowercase().contains(needle)
                || self.msg.to_lowercase().contains(needle)
            {
                return true;
            }
        }

        false
    }
}
