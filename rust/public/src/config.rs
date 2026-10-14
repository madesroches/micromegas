//! CLI args shared by every long-running server binary.

use anyhow::{Result, bail};
use micromegas_tracing::prelude::warn;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

pub const DEFAULT_INGESTION_LISTEN_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8081);
pub const DEFAULT_FLIGHTSQL_LISTEN_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 50051);
pub const DEFAULT_WEB_LISTEN_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 3000);

/// CLI args shared by every long-running server binary.
/// Flatten into a binary's clap struct with `#[command(flatten)]`.
#[derive(clap::Args, Debug, Clone)]
pub struct CommonServerArgs {
    /// Seconds to wait for in-flight work to complete after SIGTERM.
    #[arg(
        long,
        default_value = "25",
        env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS"
    )]
    pub shutdown_grace_period_seconds: u64,
}

impl CommonServerArgs {
    pub fn grace(&self) -> Duration {
        Duration::from_secs(self.shutdown_grace_period_seconds)
    }
}

/// HTTP ingestion listen address. Flatten into a binary's clap struct.
#[derive(clap::Args, Debug, Clone)]
pub struct IngestionListenArgs {
    /// HTTP ingestion listen address
    #[arg(
        long,
        env = "MICROMEGAS_INGESTION_LISTEN_ADDR",
        default_value_t = DEFAULT_INGESTION_LISTEN_ADDR,
        alias = "listen-endpoint-http"
    )]
    pub ingestion_listen_addr: SocketAddr,
}

/// FlightSQL listen address. Flatten into a binary's clap struct.
#[derive(clap::Args, Debug, Clone)]
pub struct FlightSqlListenArgs {
    /// FlightSQL listen address
    #[arg(
        long,
        env = "MICROMEGAS_FLIGHTSQL_LISTEN_ADDR",
        default_value_t = DEFAULT_FLIGHTSQL_LISTEN_ADDR
    )]
    pub flightsql_listen_addr: SocketAddr,
}

/// Web server listen address. Call [`WebListenArgs::resolve`] to get the address.
#[derive(clap::Args, Debug, Clone)]
pub struct WebListenArgs {
    /// Web server listen address (default 0.0.0.0:3000)
    #[arg(long, env = "MICROMEGAS_WEB_LISTEN_ADDR")]
    pub web_listen_addr: Option<SocketAddr>,

    /// Deprecated: use --web-listen-addr
    #[arg(short, long, env = "MICROMEGAS_PORT", hide = true)]
    pub port: Option<u16>,
}

impl WebListenArgs {
    pub fn resolve(&self) -> Result<SocketAddr> {
        match (self.web_listen_addr, self.port) {
            (Some(_), Some(_)) => bail!(
                "both --web-listen-addr/MICROMEGAS_WEB_LISTEN_ADDR and the deprecated \
                 --port/MICROMEGAS_PORT are set; remove --port/MICROMEGAS_PORT"
            ),
            (Some(addr), None) => Ok(addr),
            (None, Some(port)) => {
                warn!(
                    "--port/MICROMEGAS_PORT is deprecated, use --web-listen-addr/MICROMEGAS_WEB_LISTEN_ADDR"
                );
                Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port))
            }
            (None, None) => Ok(DEFAULT_WEB_LISTEN_ADDR),
        }
    }
}

/// Address a same-host client dials to reach `listen`: an unspecified IP
/// (0.0.0.0 / ::) maps to the loopback of the same family.
pub fn local_connect_addr(listen: SocketAddr) -> SocketAddr {
    let ip = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, listen.port())
}
