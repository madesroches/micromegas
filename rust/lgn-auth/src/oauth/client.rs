use std::{collections::HashMap, convert::Infallible, net::SocketAddr, sync::Arc};

use async_trait::async_trait;
use http::{Request, Response, StatusCode, Uri};
use hyper::{
    server::conn::AddrStream,
    service::{make_service_fn, service_fn},
    Body, Server,
};
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    reqwest::async_http_client,
    AccessToken, AccessTokenHash, AuthType, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, RedirectUrl, RefreshToken, Scope,
    TokenResponse,
};
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, info, warn};

use crate::{
    authenticator::{Authenticator, AuthenticatorWithClaims},
    OAuthClientConfig,
};
use crate::{ClientTokenSet, Error, Result, UserInfo};

const DEFAULT_REDIRECT_URI: &str = "http://localhost:3000";

#[derive(Debug, Clone)]
pub struct OAuthClient {
    client: CoreClient,
    client_id: ClientId,
    client_secret: Option<ClientSecret>,
    provider_metadata: CoreProviderMetadata,
    redirect_uri: RedirectUrl,
}

impl OAuthClient {
    /// Instantiate a new `OAuthClient` from the specified configuration.
    pub async fn new_from_config(config: &OAuthClientConfig) -> Result<Self> {
        let client = Self::new(
            config.issuer_url.clone(),
            config.client_id.clone(),
            config.client_secret.clone(),
        )
        .await?;

        if config.redirect_uri != Uri::default() {
            client.set_redirect_uri(&config.redirect_uri)
        } else {
            Ok(client)
        }
    }

    pub async fn new<'a, IU, ID, Secret>(
        issuer_url: IU,
        client_id: ID,
        client_secret: Option<Secret>,
    ) -> Result<Self>
    where
        IU: Into<String>,
        ID: Into<String>,
        Secret: Into<String>,
    {
        let issuer_url = IssuerUrl::new(issuer_url.into()).map_err(|error| {
            Error::Internal(format!("couldn't parse the issuer url: {}", error))
        })?;

        let client_id = ClientId::new(client_id.into());
        let client_secret = client_secret.map(|secret| ClientSecret::new(secret.into()));

        let provider_metadata = CoreProviderMetadata::discover_async(issuer_url, async_http_client)
            .await
            .map_err(|error| {
                Error::Internal(format!(
                    "couldn't retrieve the provider metadata: {}",
                    error
                ))
            })?;

        let redirect_uri = RedirectUrl::new(DEFAULT_REDIRECT_URI.to_string()).unwrap();

        let client = CoreClient::from_provider_metadata(
            provider_metadata.clone(),
            client_id.clone(),
            client_secret.clone(),
        )
        .set_auth_type(AuthType::RequestBody)
        .set_redirect_uri(redirect_uri.clone());

        Ok(Self {
            client,
            client_id,
            client_secret,
            provider_metadata,
            redirect_uri,
        })
    }

    pub fn set_redirect_uri(mut self, redirect_uri: &Uri) -> Result<Self> {
        let redirect_uri = RedirectUrl::new(redirect_uri.to_string()).map_err(|error| {
            Error::Internal(format!("couldn't parse the redirect uri: {}", error))
        })?;

        self.client = self.client.set_redirect_uri(redirect_uri.clone());

        self.redirect_uri = redirect_uri;

        Ok(self)
    }

    // TODO: Make this more generic
    /// Partly implements <https://openid.net/specs/openid-connect-rpinitiated-1_0.html>
    /// See <https://github.com/ramosbugs/openidconnect-rs/issues/40> for more
    /// Notice that not all oauth providers follow the rfc, Cognito for example
    /// use `"redirect_uri"` instead of `"post_logout_redirect_uri"`
    fn logout_url(&self) -> String {
        // Authorization url is required as per the rfc and will always be present,
        // so we can use it as a base url
        let mut url = self
            .provider_metadata
            .authorization_endpoint()
            .url()
            .clone();

        url = url.join("/logout").unwrap();

        {
            let mut query_pairs = url.query_pairs_mut();

            query_pairs.append_pair("client_id", &self.client_id);
            query_pairs.append_pair("redirect_uri", &self.redirect_uri);
        }

        url.to_string()
    }

    async fn receive_authorization_code(&self) -> Result<(String, String)> {
        // [`make_service_fn`] and [`make_service`] both expect an [`FnMut`] closure,
        // and since we're in an `async` context we can't just move the tokio channels
        // and need to use an [`Arc`].
        // Since we need to clone the channels twice the [`Arc::try_unwrap`] + [`Mutext::into_inner`]
        // trick won't work...
        // Our remaining solution is the [`Option::take`] trick
        // C.f. https://stackoverflow.com/questions/29177449/how-to-take-ownership-of-t-from-arcmutext

        let (tx_params, rx_params) = oneshot::channel();
        let tx_params = Arc::new(Mutex::new(Some(tx_params)));

        let (tx_done, rx_done) = oneshot::channel();
        let tx_done = Arc::new(Mutex::new(Some(tx_done)));

        let make_service = make_service_fn(move |socket: &AddrStream| {
            let tx_params = Arc::clone(&tx_params);
            let tx_done = Arc::clone(&tx_done);

            debug!("new connection from: {}", socket.remote_addr());

            async move {
                Ok::<_, Infallible>(service_fn(move |req: Request<Body>| {
                    let tx_params = Arc::clone(&tx_params);
                    let tx_done = Arc::clone(&tx_done);

                    async move {
                        debug!("received callback request: {:?}", req);

                        // We only accept calls to the root path.
                        if req.uri().path() != "/" {
                            warn!(
                                "rejecting request on unsupported path: {}",
                                req.uri().path()
                            );

                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::NOT_FOUND)
                                    .body(Body::empty())
                                    .unwrap(),
                            );
                        }

                        // Only GETs are valid.
                        if req.method() != hyper::Method::GET {
                            warn!(
                                "rejecting request with not allowed method: {}",
                                req.method(),
                            );

                            return Ok(Response::builder()
                                .status(StatusCode::METHOD_NOT_ALLOWED)
                                .body(Body::empty())
                                .unwrap());
                        }

                        // Find the code parameter.
                        let query = req
                            .uri()
                            .query()
                            .map(|v| form_urlencoded::parse(v.as_bytes()));

                        let mut query = if let Some(query) = query {
                            query
                        } else {
                            warn!("rejecting request as query string couldn't be parsed");

                            return Ok(Response::builder()
                                .status(StatusCode::BAD_REQUEST)
                                .body(Body::empty())
                                .unwrap());
                        };

                        let code = if let Some(code) =
                            query.find_map(|(k, v)| if k == "code" { Some(v) } else { None })
                        {
                            code
                        } else {
                            warn!("rejecting request without code");

                            return Ok(Response::builder()
                                .status(StatusCode::BAD_REQUEST)
                                .body(Body::empty())
                                .unwrap());
                        };

                        let state = if let Some(state) =
                            query.find_map(|(k, v)| if k == "state" { Some(v) } else { None })
                        {
                            state
                        } else {
                            warn!("rejecting request without state");

                            return Ok(Response::builder()
                                .status(StatusCode::BAD_REQUEST)
                                .body(Body::empty())
                                .unwrap());
                        };

                        let tx_params = tx_params.lock().await.take().unwrap();
                        let tx_done = tx_done.lock().await.take().unwrap();

                        let _error = tx_params.send((code.into_owned(), state.into_owned()));
                        let _error = tx_done.send(());

                        info!("authentication succeeded");

                        Ok(Response::new(Body::from(include_str!(
                            "../static/authentication_succeeded.html"
                        ))))
                    }
                }))
            }
        });

        let server = Server::bind(&self.get_callback_addr())
            .serve(make_service)
            .with_graceful_shutdown(async {
                rx_done.await.unwrap();

                info!("temporary http server shutdown");
            });

        if let Err(error) = server.await {
            Err(Error::InternalServer(error))
        } else {
            let code_and_state = rx_params.await.unwrap();

            info!("received authorization code and state : shutting down temporary HTTP server");

            Ok(code_and_state)
        }
    }

    async fn receive_logout_confirmation(&self) -> Result<()> {
        // [`make_service_fn`] and [`make_service`] both expect an [`FnMut`] closure,
        // and since we're in an `async` context we can't just move the tokio channels
        // and need to use an [`Arc`].
        // Since we need to clone the channels twice the [`Arc::try_unwrap`] + [`Mutext::into_inner`]
        // trick won't work...
        // Our remaining solution is the [`Option::take`] trick
        // C.f. https://stackoverflow.com/questions/29177449/how-to-take-ownership-of-t-from-arcmutext

        let (tx_done, rx_done) = oneshot::channel();
        let tx_done = Arc::new(Mutex::new(Some(tx_done)));

        let make_service = make_service_fn(move |socket: &AddrStream| {
            let tx_done = Arc::clone(&tx_done);

            debug!("new connection from: {}", socket.remote_addr());

            async move {
                Ok::<_, Infallible>(service_fn(move |req: Request<Body>| {
                    let tx_done = Arc::clone(&tx_done);

                    async move {
                        debug!("received callback request: {:?}", req);

                        // We only accept calls to the root path.
                        if req.uri().path() != "/" {
                            warn!(
                                "rejecting request on unsupported path: {}",
                                req.uri().path()
                            );

                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::NOT_FOUND)
                                    .body(Body::empty())
                                    .unwrap(),
                            );
                        }

                        // Only GETs are valid.
                        if req.method() != hyper::Method::GET {
                            warn!(
                                "rejecting request with not allowed method: {}",
                                req.method(),
                            );

                            return Ok(Response::builder()
                                .status(StatusCode::METHOD_NOT_ALLOWED)
                                .body(Body::empty())
                                .unwrap());
                        }

                        let tx_done = tx_done.lock().await.take().unwrap();

                        let _error = tx_done.send(());

                        info!("logout succeeded");

                        Ok(Response::new(Body::from(include_str!(
                            "../static/logout_succeeded.html"
                        ))))
                    }
                }))
            }
        });

        let server = Server::bind(&self.get_callback_addr())
            .serve(make_service)
            .with_graceful_shutdown(async {
                rx_done.await.unwrap();

                info!("temporary http server shutdown");
            });

        if let Err(error) = server.await {
            Err(Error::InternalServer(error))
        } else {
            info!("received logout confirmation: shutting down temporary HTTP server");

            Ok(())
        }
    }

    fn get_callback_addr(&self) -> SocketAddr {
        let port = self.redirect_uri.url().port().unwrap_or_else(|| {
            if self.redirect_uri.url().scheme() == "http" {
                80
            } else {
                443
            }
        });

        SocketAddr::from(([127, 0, 0, 1], port))
    }
}

#[async_trait]
impl Authenticator for OAuthClient {
    async fn login(
        &self,
        scopes: &[String],
        extra_params: &Option<HashMap<String, String>>,
    ) -> Result<ClientTokenSet> {
        // If we have a client secret, we proceed to a client credentials flow as it is non-interactive.
        let token_response = match &self.client_secret {
            Some(_) => self
                .client
                .exchange_client_credentials()
                .add_scopes(scopes.iter().cloned().map(Scope::new).collect::<Vec<_>>())
                .request_async(async_http_client)
                .await
                .map_err(|error| Error::Internal(format!("couldn't fetch token: {}", error)))?,
            None => {
                let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

                let mut authorization_url = self.client.authorize_url(
                    CoreAuthenticationFlow::AuthorizationCode,
                    CsrfToken::new_random,
                    Nonce::new_random,
                );

                for scope in scopes {
                    authorization_url = authorization_url.add_scope(Scope::new(scope.into()));
                }

                if let Some(extra_params) = extra_params {
                    for (name, value) in extra_params {
                        authorization_url = authorization_url.add_extra_param(name, value);
                    }
                }

                let (authorization_url, csrf_token, nonce) =
                    authorization_url.set_pkce_challenge(pkce_challenge).url();

                info!("Opening web-browser at: {}", authorization_url);

                webbrowser::open(authorization_url.as_str()).map_err(Error::InteractiveProcess)?;

                let (code, state) = self.receive_authorization_code().await?;

                if state != *csrf_token.secret() {
                    return Err(Error::Internal(
                        "Received csrf code and expected csrf code don't match".into(),
                    ));
                }

                let token_response = self
                    .client
                    .exchange_code(AuthorizationCode::new(code))
                    .set_pkce_verifier(pkce_verifier)
                    .request_async(async_http_client)
                    .await
                    .map_err(|error| Error::Internal(format!("couldn't get code {}", error)))?;

                let id_token = token_response
                    .id_token()
                    .ok_or_else(|| Error::Internal("Server did not return an ID token".into()))?;

                let claims = id_token
                    .claims(&self.client.id_token_verifier(), &nonce)
                    .map_err(|error| Error::Internal(format!("couldn't get claims {}", error)))?;

                if let Some(expected_access_token_hash) = claims.access_token_hash() {
                    let actual_access_token_hash = AccessTokenHash::from_token(
                        token_response.access_token(),
                        &id_token.signing_alg().map_err(|error| {
                            Error::Internal(format!(
                                "couldn't get the signing algorithm from id token {}",
                                error
                            ))
                        })?,
                    )
                    .map_err(|error| {
                        Error::Internal(format!("couldn't initialize access token hash {}", error))
                    })?;

                    if actual_access_token_hash != *expected_access_token_hash {
                        return Err(Error::Internal(
                            "Received access token hash and expected access token hash don't match"
                                .into(),
                        ));
                    }
                }

                token_response
            }
        };

        let mut client_token_set: ClientTokenSet = token_response.try_into()?;

        client_token_set.set_scopes(scopes);

        Ok(client_token_set)
    }

    /// Get a token set from a refresh token.
    ///
    /// If the call does not return a new refresh token within the `TokenSet`,
    /// the specified refresh token will be filled in instead.
    ///
    /// Consumes the provided [`ClientTokenSet`].
    async fn refresh_login(&self, client_token_set: ClientTokenSet) -> Result<ClientTokenSet> {
        if let Some(refresh_token) = client_token_set.refresh_token {
            let token_response = self
                .client
                .exchange_refresh_token(&RefreshToken::new(refresh_token.clone()))
                .request_async(async_http_client)
                .await
                .map_err(|error| Error::Internal(format!("couldn't get token set: {}", error)))?;

            let scopes = client_token_set.scopes;

            let mut client_token_set: ClientTokenSet = token_response.try_into()?;

            if let Some(scopes) = scopes {
                client_token_set.scopes = Some(scopes);
            }

            // Make sure we keep the refresh token if the reply doesn't contain
            // one, which it apparently does not when using a refresh token.
            if client_token_set.refresh_token.is_none() {
                client_token_set.refresh_token = Some(refresh_token);
            }

            Ok(client_token_set)
        } else {
            Err(Error::Internal(
                "provided client token set doesn't contain any refresh_token attribute".into(),
            ))
        }
    }

    /// Logout by opening an interactive browser window
    async fn logout(&self) -> Result<()> {
        // If we have a client secret, we don't need to logout as we expect a
        // non-interactive process.
        if self.client_secret.is_none() {
            let logout_url = self.logout_url();

            info!("Opening web-browser at: {}", logout_url);

            webbrowser::open(logout_url.as_str()).map_err(Error::InteractiveProcess)?;

            self.receive_logout_confirmation().await
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl AuthenticatorWithClaims for OAuthClient {
    async fn get_user_info_claims(&self, access_token: &AccessToken) -> Result<UserInfo> {
        let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_or_http()
            .enable_http2()
            .build();

        let client = hyper::Client::builder().build::<_, hyper::Body>(https_connector);

        let url = self.provider_metadata.userinfo_endpoint().ok_or_else(|| {
            Error::Internal("userinfo endpoint not provided by the issuer".into())
        })?;

        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(url.as_str())
            .header(
                hyper::header::AUTHORIZATION,
                format!("Bearer {}", access_token.secret()),
            )
            .body(Body::empty())
            .unwrap();

        let resp = client
            .request(req)
            .await
            .map_err(|e| Error::Internal(format!("failed to execute HTTP request: {}", e)))?;

        let bytes = hyper::body::to_bytes(resp.into_body())
            .await
            .map_err(|e| Error::Internal(format!("failed to read HTTP response: {}", e)))?;

        serde_json::from_slice(&bytes)
            .map_err(|e| Error::Internal(format!("failed to parse JSON user info: {}", e)))
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

        let user_info_claims = self
            .get_user_info_claims(&AccessToken::new(client_token_set.access_token))
            .await
            .map_err(Error::from)?;

        Ok(user_info_claims)
    }
}
