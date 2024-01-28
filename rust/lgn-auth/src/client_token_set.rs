use std::{collections::BTreeSet, time::Duration};

use openidconnect::{core::CoreTokenResponse, OAuth2TokenResponse, TokenResponse};
use serde::{Deserialize, Serialize};

use super::Error;

/// A set of tokens as given to clients.
#[derive(Serialize, Deserialize, Debug)]
pub struct ClientTokenSet {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<Duration>,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub scopes: Option<Vec<String>>,
}

impl ClientTokenSet {
    pub fn set_scopes(&mut self, scopes: &[String]) -> &mut Self {
        self.scopes = Some(scopes.to_vec());

        self
    }

    /// Checks whether or not the [`ClientTokenSet`] is compliant with
    /// the provided scopes.
    pub fn is_compliant_with_scopes(&self, scopes: &[String]) -> bool {
        // For now we only return `true` if the scopes are stricly equal but it could change in the future
        BTreeSet::from_iter(&self.scopes.clone().unwrap_or_default()) == BTreeSet::from_iter(scopes)
    }
}

impl TryFrom<CoreTokenResponse> for ClientTokenSet {
    type Error = Error;

    fn try_from(core_token_response: CoreTokenResponse) -> Result<Self, Self::Error> {
        Ok(Self {
            access_token: core_token_response.access_token().secret().clone(),
            token_type: serde_json::to_value(core_token_response.token_type())
                .map_err(|error| {
                    Error::Internal(format!("couldn't serialize the token type: {}", error))
                })?
                .as_str()
                .ok_or_else(|| Error::Internal("Token type is not a value of type string".into()))?
                .to_string(),
            expires_in: core_token_response.expires_in(),
            refresh_token: core_token_response
                .refresh_token()
                .map(|refresh_token| refresh_token.secret().clone()),
            id_token: core_token_response.id_token().map(ToString::to_string),
            scopes: core_token_response
                .scopes()
                .map(|scopes| scopes.iter().map(|scope| scope.to_string()).collect()),
        })
    }
}
