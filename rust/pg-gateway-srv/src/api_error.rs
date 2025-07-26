use micromegas::datafusion_postgres::pgwire;

pub fn into_api_error(
    e: impl Into<Box<dyn std::error::Error + 'static + Send + Sync>>,
) -> pgwire::error::PgWireError {
    pgwire::error::PgWireError::ApiError(e.into())
}

#[macro_export]
macro_rules! api_error {
    () => {
        |err| {
            micromegas::tracing::error!("{:?}", err);
            $crate::api_error::into_api_error(err)
        }
    };

    ($err:expr) => {{
        micromegas::tracing::error!("{:?}", $err);
        $crate::api_error::into_api_error($err)
    }};
}
