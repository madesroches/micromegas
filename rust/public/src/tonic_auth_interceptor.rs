use crate::key_ring::KeyRing;
use micromegas_tracing::prelude::*;
use tonic::{Request, Status};

pub fn check_auth(req: Request<()>, keyring: &KeyRing) -> Result<Request<()>, Status> {
    info!("check_auth");
    let metadata = req.metadata();
    let authorization = metadata
        .get(http::header::AUTHORIZATION.as_str())
        .ok_or_else(|| {
            warn!("missing authorization header");
            Status::unauthenticated(format!("No authorization header! metadata = {metadata:?}"))
        })?
        .to_str()
        .map_err(|e| {
            warn!("error parsing authorization header");
            Status::unauthenticated(format!("Error parsing header: {e}"))
        })?;
    let bearer = "Bearer ";
    if !authorization.starts_with(bearer) {
        warn!("Invalid auth header");
        return Err(Status::unauthenticated("Invalid auth header"));
    }
    let token = authorization[bearer.len()..].to_string();
    if let Some(name) = keyring.get(&token.into()) {
        info!("caller={name}");
        Ok(req)
    } else {
        warn!("invalid API token");
        Err(Status::unauthenticated("invalid API token"))
    }
}
