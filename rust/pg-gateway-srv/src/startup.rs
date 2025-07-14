use crate::state::SharedState;
use async_trait::async_trait;
use futures::Sink;
use micromegas::datafusion_postgres::pgwire;
use micromegas::{client::flightsql_client_factory::BearerFlightSQLClientFactory, tracing::info};
use pgwire::api::auth::{finish_authentication, DefaultServerParameterProvider};
use pgwire::{
    api::{
        auth::{save_startup_parameters_to_metadata, StartupHandler},
        ClientInfo,
    },
    error::{PgWireError, PgWireResult},
    messages::{PgWireBackendMessage, PgWireFrontendMessage},
};
use std::{fmt::Debug, sync::Arc};

/// Handles the startup phase of a PostgreSQL connection.
pub struct StartupH {
    state: crate::state::SharedState,
}

impl StartupH {
    pub fn new(state: SharedState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl StartupHandler for StartupH {
    async fn on_startup<C>(
        &self,
        client: &mut C,
        message: PgWireFrontendMessage,
    ) -> PgWireResult<()>
    where
        C: ClientInfo + Sink<PgWireBackendMessage> + Unpin + Send,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        info!("on_startup message={message:?}");
        if let PgWireFrontendMessage::Startup(ref startup) = message {
            save_startup_parameters_to_metadata(client, startup);
            finish_authentication(client, &DefaultServerParameterProvider::default()).await?;

            self.state
                .lock()
                .await
                .set_factory(Arc::new(BearerFlightSQLClientFactory::new("".into())));
            info!("ready for query");
        }
        Ok(())
    }
}
