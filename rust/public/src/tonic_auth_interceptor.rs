use crate::key_ring::KeyRing;
use micromegas_tracing::prelude::*;
use tonic::{Request, Status};

pub fn check_auth(req: Request<()>, keyring: &KeyRing) -> Result<Request<()>, Status> {
    let metadata = req.metadata();
    let authorization = metadata
        .get("authorization")
        .ok_or_else(|| {
            Status::internal(format!("No authorization header! metadata = {metadata:?}"))
        })?
        .to_str()
        .map_err(|e| Status::internal(format!("Error parsing header: {e}")))?;
    let bearer = "Bearer ";
    if !authorization.starts_with(bearer) {
        return Err(Status::internal("Invalid auth header!"));
    }
    let token = authorization[bearer.len()..].to_string();
    if let Some(name) = keyring.get(&token) {
        info!("caller={name}");
        Ok(req)
    } else {
        warn!("invalid API token");
        Err(Status::unauthenticated("invalid API token"))
    }
}
