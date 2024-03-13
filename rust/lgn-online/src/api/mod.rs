use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::Router;
use micromegas_tracing::info;

use crate::server::RouterExt;

pub use errors::{Error, Result};

mod errors;

#[derive(Default)]
pub struct Server {
    rest_listen_address: Option<SocketAddr>,
}

impl Server {
    #[must_use]
    pub fn set_rest_listen_address(mut self, listen_address: SocketAddr) -> Self {
        self.rest_listen_address = Some(listen_address);

        self
    }

    pub async fn run(self, router: Arc<Mutex<Router>>) -> Result<()> {
        let rest_listen_address = self.rest_listen_address.ok_or_else(|| {
            Error::RunServerFailure(
                "running as local server but no listen address was specified".to_string(),
            )
        })?;

        let rest_service = router
            .lock()
            .unwrap()
            .clone()
            .apply_development_router_options()
            .into_make_service_with_connect_info::<SocketAddr>();

        let rest_server = axum::Server::bind(&rest_listen_address).serve(rest_service);

        info!("Starting rest web server at {}...", rest_listen_address);

        rest_server.await.map_err(|err| Error::Other(err.into()))
    }
}
