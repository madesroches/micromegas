use std::{collections::HashMap, fmt::Debug, sync::Arc};

use async_trait::async_trait;
use openidconnect::AccessToken;

use crate::{ClientTokenSet, Result, UserInfo};

#[async_trait]
pub trait Authenticator: Debug {
    /// Perform a login.
    async fn login(
        &self,
        scopes: &[String],
        extra_params: &Option<HashMap<String, String>>,
    ) -> Result<ClientTokenSet>;

    /// Perform a non-interactive login by using a refresh login.
    async fn refresh_login(&self, client_token_set: ClientTokenSet) -> Result<ClientTokenSet>;

    /// Perform a logout, possibly using an interactive prompt.
    async fn logout(&self) -> Result<()>;
}

#[async_trait]
impl<T> Authenticator for Arc<T>
where
    T: Authenticator + Send + Sync,
{
    async fn login(
        &self,
        scopes: &[String],
        extra_params: &Option<HashMap<String, String>>,
    ) -> Result<ClientTokenSet> {
        self.as_ref().login(scopes, extra_params).await
    }

    async fn refresh_login(&self, client_token_set: ClientTokenSet) -> Result<ClientTokenSet> {
        self.as_ref().refresh_login(client_token_set).await
    }

    async fn logout(&self) -> Result<()> {
        self.as_ref().logout().await
    }
}

#[async_trait]
impl<T> Authenticator for Box<T>
where
    T: Authenticator + Send + Sync,
{
    async fn login(
        &self,
        scopes: &[String],
        extra_params: &Option<HashMap<String, String>>,
    ) -> Result<ClientTokenSet> {
        self.as_ref().login(scopes, extra_params).await
    }

    async fn refresh_login(&self, client_token_set: ClientTokenSet) -> Result<ClientTokenSet> {
        self.as_ref().refresh_login(client_token_set).await
    }

    async fn logout(&self) -> Result<()> {
        self.as_ref().logout().await
    }
}

/// A boxed `Authenticator` that can be used to authenticate requests.
#[derive(Debug)]
pub struct BoxedAuthenticator(pub Box<dyn Authenticator + Send + Sync>);

#[async_trait]
impl Authenticator for BoxedAuthenticator {
    async fn login(
        &self,
        scopes: &[String],
        extra_params: &Option<HashMap<String, String>>,
    ) -> Result<ClientTokenSet> {
        self.0.login(scopes, extra_params).await
    }

    async fn refresh_login(&self, client_token_set: ClientTokenSet) -> Result<ClientTokenSet> {
        self.0.refresh_login(client_token_set).await
    }

    async fn logout(&self) -> Result<()> {
        self.0.logout().await
    }
}

#[async_trait]
pub trait AuthenticatorWithClaims: Authenticator {
    /// Fetch the user info claims.
    async fn get_user_info_claims(&self, access_token: &AccessToken) -> Result<UserInfo>;

    /// First calls the `login` method and then fetch the user info claims
    async fn authenticate(
        &self,
        scopes: &[String],
        extra_params: &Option<HashMap<String, String>>,
    ) -> Result<UserInfo>;
}
