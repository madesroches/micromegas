//! Cookie helpers shared by the auth handlers.

use super::state::AuthState;
use axum_extra::extract::cookie::{Cookie, SameSite};

/// Cookie names
pub(crate) const ID_TOKEN_COOKIE: &str = "id_token"; // ID token (JWT) for user info and FlightSQL API authorization
pub(crate) const REFRESH_TOKEN_COOKIE: &str = "refresh_token";
pub(crate) const OAUTH_STATE_COOKIE: &str = "oauth_state";

/// Create a cookie with common settings
pub fn create_cookie<'a>(
    name: &'a str,
    value: String,
    max_age_secs: i64,
    state: &AuthState,
) -> Cookie<'a> {
    let mut cookie = Cookie::build((name, value))
        .http_only(true)
        .secure(state.secure_cookies)
        .same_site(SameSite::Lax)
        .path(state.cookie_path())
        .max_age(time::Duration::seconds(max_age_secs));

    if let Some(domain) = &state.cookie_domain {
        cookie = cookie.domain(domain.clone());
    }

    cookie.build()
}

/// Create an expired cookie to clear it
pub fn clear_cookie<'a>(name: &'a str, state: &AuthState) -> Cookie<'a> {
    let mut cookie = Cookie::build((name, ""))
        .http_only(true)
        .secure(state.secure_cookies)
        .same_site(SameSite::Lax)
        .path(state.cookie_path())
        .max_age(time::Duration::seconds(0));

    if let Some(domain) = &state.cookie_domain {
        cookie = cookie.domain(domain.clone());
    }

    cookie.build()
}
