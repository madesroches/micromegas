use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use directories::ProjectDirs;
use micromegas_tracing::{debug, warn};
use openidconnect::AccessToken;
use tokio::sync::{Mutex, MutexGuard};

use crate::authenticator::{Authenticator, AuthenticatorWithClaims};
use crate::UserInfo;
use crate::{jwt::UnsecureValidation, ClientTokenSet, Error, Result};
/// A `TokenCache` stores authentication tokens and handles their lifetime.
#[derive(Clone, Debug)]
pub struct TokenCache<A> {
    project_dirs: ProjectDirs,
    validation: UnsecureValidation,
    authenticator: Arc<Mutex<A>>,
}

impl<A> TokenCache<A>
where
    A: Authenticator,
{
    /// Instantiate a new `TokenCache`
    pub fn new(authenticator: A, project_dirs: ProjectDirs) -> Self {
        Self {
            project_dirs,
            validation: UnsecureValidation::default(),
            authenticator: Arc::new(Mutex::new(authenticator)),
        }
    }

    /// Instantiate a new `TokenCache`
    pub fn new_with_application_name(authenticator: A, application: &str) -> Self {
        let project_dirs = ProjectDirs::from("com", "legionlabs", application)
            .expect("failed to determine project dirs");

        Self::new(authenticator, project_dirs)
    }

    /// Get the `Authenticator` used by this `TokenCache`.
    pub async fn authenticator(&self) -> MutexGuard<'_, A> {
        self.authenticator.lock().await
    }

    fn get_tokens_file_path(&self) -> PathBuf {
        self.project_dirs.cache_dir().join("tokens.json")
    }

    /// Read the token set from the cache.
    pub fn read_token_set_from_cache(&self) -> Result<ClientTokenSet> {
        let path = self.get_tokens_file_path();
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).map_err(|e| {
            Error::Internal(format!("failed to parse JSON token set from cache: {}", e))
        })
    }

    // Write the access token to the cache.
    pub fn write_token_set_to_cache(&self, token_set: &ClientTokenSet) -> Result<()> {
        let path = self.get_tokens_file_path();
        let parent_path = path.parent().unwrap();

        std::fs::create_dir_all(parent_path).map_err(|e| {
            Error::Internal(format!(
                "creating cache directory at {}: {}",
                parent_path.display(),
                e
            ))
        })?;
        let file = File::create(&path).map_err(|e| {
            Error::Internal(format!("creating tokens file at {}: {}", path.display(), e))
        })?;
        let writer = BufWriter::new(file);
        serde_json::to_writer(writer, &token_set)
            .map_err(|e| Error::Internal(format!("failed to write JSON token set to cache: {}", e)))
    }

    // Delete the cache.
    pub fn delete_cache(&self) -> Result<()> {
        let path = self.get_tokens_file_path();
        match std::fs::remove_file(&path) {
            Ok(_) => Ok(()),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(Error::Internal(format!(
                        "deleting cache file at {}: {}",
                        path.display(),
                        e,
                    )))
                }
            }
        }
    }

    async fn refresh_login_with(
        &self,
        client_token_set: ClientTokenSet,
        authenticator: &A,
    ) -> Result<ClientTokenSet> {
        let result = authenticator
            .refresh_login(client_token_set)
            .await
            .map(|token_set| {
                if let Err(err) = self.write_token_set_to_cache(&token_set) {
                    warn!("Failed to write access token to cache: {}", err);
                }

                token_set
            });

        if result.is_err() {
            self.delete_cache()?;
        }

        result
    }
}

#[async_trait]
impl<T> Authenticator for TokenCache<T>
where
    T: Authenticator + Send + Sync,
{
    /// Get the access token from the cache if it exists, or performs an
    /// implicit refresh.
    ///
    /// If that fails too, the call will fall back to the `Authenticator`'s
    /// `login` method, which may prompt the user for credentials.
    ///
    /// If the tokens end up being refreshed, they will be stored in the cache.
    async fn login(
        &self,
        scopes: &[String],
        extra_params: &Option<HashMap<String, String>>,
    ) -> Result<ClientTokenSet> {
        let authenticator = self.authenticator().await;

        let token_set = match self.read_token_set_from_cache() {
            Ok(token_set) => {
                let access_token = &token_set.access_token[..];
                match access_token.try_into() {
                    Ok(access_token) => {
                        match self.validation.validate_claims(&access_token) {
                            Ok(_) => {
                                if !token_set.is_compliant_with_scopes(scopes) {
                                    warn!(
                                        "Cached access token scopes don't match required scopes, refreshing login...",
                                    );

                                    authenticator.login(scopes, extra_params).await?
                                } else {
                                    debug!("Reusing cached access token.");

                                    // Bail out immediately because we don't need to refresh the token and
                                    // write it to the cache in this case.
                                    return Ok(token_set);
                                }
                            }
                            Err(err) => {
                                if let Error::TokenExpired { .. } = err {
                                    debug!(
                                        "Cached access token has expired ({}): refreshing login...",
                                        err
                                    );
                                } else {
                                    warn!(
                                        "Cached access token is invalid ({}): refreshing login...",
                                        err
                                    );
                                }

                                if token_set.refresh_token.is_some() {
                                    return self
                                        .refresh_login_with(token_set, &authenticator)
                                        .await;
                                }

                                authenticator.login(scopes, extra_params).await?
                            }
                        }
                    }
                    Err(e) => {
                        warn!("invalid access token ({}): cache will be deleted", e);

                        self.delete_cache()?;

                        authenticator.login(scopes, extra_params).await?
                    }
                }
            }
            Err(Error::Io(err)) => {
                // Not having a token cache is considered a normal flow for first login.
                if err.kind() != std::io::ErrorKind::NotFound {
                    warn!("Failed to read access token from cache: {}", err);
                }

                authenticator.login(scopes, extra_params).await?
            }
            Err(err) => {
                warn!("Failed to read access token from cache: {}", err);

                authenticator.login(scopes, extra_params).await?
            }
        };

        // If we can't write the token to the cache, we can't do anything about it but
        // warn the user.
        if let Err(err) = self.write_token_set_to_cache(&token_set) {
            warn!("Failed to write access token to cache: {}", err);
        }

        Ok(token_set)
    }

    async fn refresh_login(&self, client_token_set: ClientTokenSet) -> Result<ClientTokenSet> {
        let authenticator = self.authenticator().await;

        self.refresh_login_with(client_token_set, &authenticator)
            .await
    }

    /// Perform a logout, delegating its execution to the owned `Authenticator`
    /// and clearing the cache.
    async fn logout(&self) -> Result<()> {
        let authenticator = self.authenticator().await;

        self.delete_cache()?;

        authenticator.logout().await
    }
}

#[async_trait]
impl<T> AuthenticatorWithClaims for TokenCache<T>
where
    T: AuthenticatorWithClaims + Send + Sync,
{
    async fn get_user_info_claims(&self, access_token: &AccessToken) -> Result<UserInfo> {
        self.authenticator()
            .await
            .get_user_info_claims(access_token)
            .await
    }

    async fn authenticate(
        &self,
        scopes: &[String],
        extra_params: &Option<HashMap<String, String>>,
    ) -> Result<UserInfo> {
        let client_token_set = self
            .login(scopes, extra_params)
            .await
            .map_err(Error::from)?;

        self.get_user_info_claims(&AccessToken::new(client_token_set.access_token))
            .await
    }
}
