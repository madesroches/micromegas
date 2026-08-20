use redis_exporter::info_parser::ParsedInfo;

const FIXTURE: &str = include_str!("fixtures/info_all.txt");

#[test]
fn scalar_fields() {
    let info = ParsedInfo::parse(FIXTURE);
    assert_eq!(info.get_u64("connected_clients"), Some(2));
    assert_eq!(info.get_u64("used_memory"), Some(1_024_000));
    assert_eq!(info.get_f64("mem_fragmentation_ratio"), Some(2.0));
    assert_eq!(info.get_str("role"), Some("master"));
    assert_eq!(info.get_u64("nonexistent_field"), None);
}

#[test]
fn crlf_and_comments_are_handled() {
    let info = ParsedInfo::parse("# Clients\r\nconnected_clients:7\r\n\r\n");
    assert_eq!(info.get_u64("connected_clients"), Some(7));
}

#[test]
fn non_numeric_value_yields_none_not_panic() {
    let info = ParsedInfo::parse("weird:not_a_number\n");
    assert_eq!(info.get_u64("weird"), None);
    assert_eq!(info.get_f64("weird"), None);
}

#[test]
fn keyspace_entries() {
    let info = ParsedInfo::parse(FIXTURE);
    let dbs = info.keyspace();
    assert_eq!(dbs.len(), 2);
    assert_eq!((dbs[0].db, dbs[0].keys, dbs[0].expires), (0, 5, 1));
    assert_eq!((dbs[1].db, dbs[1].keys, dbs[1].expires), (2, 42, 0));
}

#[test]
fn command_stats_entries() {
    let info = ParsedInfo::parse(FIXTURE);
    let stats = info.command_stats();
    assert_eq!(stats.len(), 2);
    let get = stats
        .iter()
        .find(|s| s.name == "get")
        .expect("cmdstat_get present in fixture");
    assert_eq!(get.calls, 4250);
    assert_eq!(get.usec, 8500);
    assert_eq!(get.usec_per_call, 2.0);
}

#[test]
fn malformed_keyspace_line_is_skipped() {
    let info = ParsedInfo::parse("db0:garbage\ndb1:keys=3,expires=0,avg_ttl=0\n");
    let dbs = info.keyspace();
    assert_eq!(dbs.len(), 1);
    assert_eq!(dbs[0].db, 1);
}
