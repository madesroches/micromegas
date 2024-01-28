use super::{
    jwt::{
        signature_validation::{
            AwsCognitoSignatureValidation, BoxedSignatureValidation, NoSignatureValidation,
            RsaSignatureValidation,
        },
        Validation,
    },
    BoxedAuthenticator, Error, OAuthClient, Result, TokenCache,
};
use http::Uri;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum AuthenticatorConfig {
    #[serde(rename = "oauth")]
    OAuth(OAuthClientConfig),
}

#[derive(Debug, Clone, Deserialize)]
pub struct OAuthClientConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    #[serde(default, with = "http_serde::uri")]
    pub redirect_uri: Uri,

    #[serde(default = "OAuthClientConfig::default_token_cache_application_name")]
    pub token_cache_application_name: String,
}

impl OAuthClientConfig {
    fn default_token_cache_application_name() -> String {
        "lgn-online".to_string()
    }

    /// Instantiate the authenticator from the configuration.
    ///
    /// # Errors
    ///
    /// If the configuration is invalid, an error is returned.
    pub async fn instantiate_authenticator(&self) -> Result<BoxedAuthenticator> {
        Ok(BoxedAuthenticator(Box::new(
            TokenCache::new_with_application_name(
                OAuthClient::new_from_config(self).await?,
                &self.token_cache_application_name,
            ),
        )))
    }
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum SignatureValidationConfig {
    Disabled {},
    Rsa(RsaSignatureValidationConfig),
    AwsCognito(AwsCognitoSignatureValidationConfig),
}

#[derive(Deserialize, Debug)]
pub struct RsaSignatureValidationConfig {
    n: String,
    e: String,
}

#[derive(Deserialize, Debug)]
pub struct AwsCognitoSignatureValidationConfig {
    region: String,
    user_pool_id: String,
}

impl SignatureValidationConfig {
    pub fn new() -> Result<Self> {
        lgn_config::get("authentication.signature_validation")
            .map_err(Error::from)?
            .ok_or_else(|| Error::CustomConfig("no signature validation config found".into()))
    }

    /// Instantiate a `SignatureValidation` from the configuration.
    pub async fn signature_validation(&self) -> Result<BoxedSignatureValidation> {
        Ok(BoxedSignatureValidation(match self {
            SignatureValidationConfig::Disabled {} => Box::new(NoSignatureValidation {}),
            SignatureValidationConfig::Rsa(config) => Box::new(
                RsaSignatureValidation::new_from_components(&config.n, &config.e)?,
            ),
            SignatureValidationConfig::AwsCognito(config) => Box::new(
                AwsCognitoSignatureValidation::new(&config.region, &config.user_pool_id).await?,
            ),
        }))
    }

    /// Instantiate a `Validation` from the configuration.
    pub async fn validation(&self) -> Result<Validation<BoxedSignatureValidation>> {
        match self.signature_validation().await {
            Ok(signature_validation) => Ok(Validation::new(signature_validation)),
            Err(err) => Err(err),
        }
    }
}
