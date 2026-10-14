use clap::Parser;
use micromegas::config::{
    DEFAULT_FLIGHTSQL_LISTEN_ADDR, DEFAULT_INGESTION_LISTEN_ADDR, DEFAULT_WEB_LISTEN_ADDR,
    FlightSqlListenArgs, IngestionListenArgs, WebListenArgs, local_connect_addr,
};
use std::net::SocketAddr;

#[derive(Parser, Debug)]
struct IngestionCli {
    #[command(flatten)]
    listen: IngestionListenArgs,
}

#[derive(Parser, Debug)]
struct FlightSqlCli {
    #[command(flatten)]
    listen: FlightSqlListenArgs,
}

#[derive(Parser, Debug)]
struct WebCli {
    #[command(flatten)]
    listen: WebListenArgs,
}

fn addr(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

#[test]
fn ingestion_parses_new_flag() {
    let cli =
        IngestionCli::try_parse_from(["t", "--ingestion-listen-addr", "127.0.0.1:9000"]).unwrap();
    assert_eq!(cli.listen.ingestion_listen_addr, addr("127.0.0.1:9000"));
}

#[test]
fn ingestion_parses_legacy_flag() {
    let cli =
        IngestionCli::try_parse_from(["t", "--listen-endpoint-http", "127.0.0.1:9000"]).unwrap();
    assert_eq!(cli.listen.ingestion_listen_addr, addr("127.0.0.1:9000"));
}

#[test]
fn ingestion_default() {
    let cli = IngestionCli::try_parse_from(["t"]).unwrap();
    assert_eq!(
        cli.listen.ingestion_listen_addr,
        DEFAULT_INGESTION_LISTEN_ADDR
    );
    assert_eq!(DEFAULT_INGESTION_LISTEN_ADDR, addr("127.0.0.1:8081"));
}

#[test]
fn flightsql_parses_new_flag() {
    let cli =
        FlightSqlCli::try_parse_from(["t", "--flightsql-listen-addr", "127.0.0.1:50061"]).unwrap();
    assert_eq!(cli.listen.flightsql_listen_addr, addr("127.0.0.1:50061"));
}

#[test]
fn flightsql_default() {
    let cli = FlightSqlCli::try_parse_from(["t"]).unwrap();
    assert_eq!(
        cli.listen.flightsql_listen_addr,
        DEFAULT_FLIGHTSQL_LISTEN_ADDR
    );
    assert_eq!(DEFAULT_FLIGHTSQL_LISTEN_ADDR, addr("0.0.0.0:50051"));
}

#[test]
fn web_parses_new_flag() {
    let cli = WebCli::try_parse_from(["t", "--web-listen-addr", "127.0.0.1:3001"]).unwrap();
    assert_eq!(cli.listen.web_listen_addr, Some(addr("127.0.0.1:3001")));
    assert_eq!(cli.listen.port, None);
}

#[test]
fn web_no_flags_yields_none() {
    let cli = WebCli::try_parse_from(["t"]).unwrap();
    assert_eq!(cli.listen.web_listen_addr, None);
    assert_eq!(cli.listen.port, None);
}

#[test]
fn web_resolve_addr_only() {
    let args = WebListenArgs {
        web_listen_addr: Some(addr("127.0.0.1:3001")),
        port: None,
    };
    assert_eq!(args.resolve().unwrap(), addr("127.0.0.1:3001"));
}

#[test]
fn web_resolve_port_only() {
    let args = WebListenArgs {
        web_listen_addr: None,
        port: Some(8000),
    };
    assert_eq!(args.resolve().unwrap(), addr("0.0.0.0:8000"));
}

#[test]
fn web_resolve_neither() {
    let args = WebListenArgs {
        web_listen_addr: None,
        port: None,
    };
    assert_eq!(args.resolve().unwrap(), DEFAULT_WEB_LISTEN_ADDR);
    assert_eq!(DEFAULT_WEB_LISTEN_ADDR, addr("0.0.0.0:3000"));
}

#[test]
fn web_resolve_both_is_error() {
    let args = WebListenArgs {
        web_listen_addr: Some(addr("127.0.0.1:3001")),
        port: Some(8000),
    };
    assert!(args.resolve().is_err());
}

#[test]
fn local_connect_addr_maps_unspecified_to_loopback() {
    assert_eq!(
        local_connect_addr(addr("0.0.0.0:50051")),
        addr("127.0.0.1:50051")
    );
    assert_eq!(local_connect_addr(addr("[::]:50051")), addr("[::1]:50051"));
}

#[test]
fn local_connect_addr_passes_specific_ip_through() {
    assert_eq!(
        local_connect_addr(addr("10.1.2.3:50051")),
        addr("10.1.2.3:50051")
    );
    assert_eq!(
        local_connect_addr(addr("[fe80::1]:50051")),
        addr("[fe80::1]:50051")
    );
}
