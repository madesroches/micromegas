use serde::{de, Deserialize, Deserializer, Serialize};

/// Contains user information.
///
/// Standard OIDC claims, as specified in the [`OpenID`
/// RFC](https://openid.net/specs/openid-connect-core-1_0.html#StandardClaims).
///
/// # Example
///
/// ```rust
/// use lgn_auth::UserInfo;
///
/// // Bare minimum: we the `sub` field.
/// let _: UserInfo = serde_json::from_str(r#"{"sub": "foo"}"#).unwrap();
///
/// // Boolean fields support both string and bool representation.
/// let _: UserInfo = serde_json::from_str(r#"{"sub": "foo", "email_verified": "true"}"#).unwrap();
/// let _: UserInfo = serde_json::from_str(r#"{"sub": "foo", "email_verified": true}"#).unwrap();
/// ```
// TODO: Ideally we should allow for a node feature that would automatically make the `UserInfo` struct
// compatible with Napi. Unfortunately it doesn't work well with the ci and it raises lots of errors.
// #[cfg_attr(feature = "node", napi_derive::napi(object))]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub sub: String,
    pub name: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub middle_name: Option<String>,
    pub nickname: Option<String>,
    pub username: Option<String>,
    pub preferred_username: Option<String>,
    pub profile: Option<String>,
    pub picture: Option<String>,
    pub website: Option<String>,
    pub email: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_string_bool")]
    pub email_verified: Option<bool>,
    pub gender: Option<String>,
    pub birthdate: Option<String>,
    pub zoneinfo: Option<String>,
    pub locale: Option<String>,
    pub phone_number: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_string_bool")]
    pub phone_number_verified: Option<bool>,
    pub updated_at: Option<String>,

    // Azure-specific fields.
    //
    // This is a merely a convention, but we need one.
    //
    // These fields contains the Azure-specific information about the user, which allow us to query
    // the Azure API for extended user information (like the user's photo).
    #[serde(rename = "custom:azure_oid")]
    pub azure_oid: Option<String>,
    #[serde(rename = "custom:azure_tid")]
    pub azure_tid: Option<String>,
}

impl UserInfo {
    pub fn name(&self) -> String {
        if let Some(name) = &self.name {
            name.to_string()
        } else {
            match (&self.given_name, &self.middle_name, &self.family_name) {
                (Some(given_name), Some(middle_name), Some(family_name)) => {
                    format!("{} {} {}", given_name, middle_name, family_name)
                }
                (Some(given_name), None, Some(family_name)) => {
                    format!("{} {}", given_name, family_name)
                }
                (Some(given_name), Some(middle_name), None) => {
                    format!("{} {}", given_name, middle_name)
                }
                (Some(given_name), None, None) => given_name.to_string(),
                (None, Some(middle_name), Some(family_name)) => {
                    format!("{} {}", middle_name, family_name)
                }
                (None, Some(middle_name), None) => middle_name.to_string(),
                (None, None, Some(family_name)) => family_name.to_string(),
                (None, None, None) => {
                    if let Some(nickname) = &self.nickname {
                        nickname.to_string()
                    } else if let Some(email) = &self.email {
                        email.to_string()
                    } else {
                        "<unknown>".to_string()
                    }
                }
            }
        }
    }

    pub fn username(&self) -> String {
        if let Some(preferred_username) = &self.preferred_username {
            preferred_username.to_string()
        } else if let Some(username) = &self.username {
            username.to_string()
        } else if let Some(email) = &self.email {
            email.to_string()
        } else {
            "<unknown>".to_string()
        }
    }
}

/// Cognito's `email_verified` and `phone_number_verified` values are sometimes booleans and sometimes
/// strings (`"true"`/`"false"`). This function can be used with the `deserialize_with` attribute of Serde
/// in order to deserialize such attributes.
pub fn deserialize_string_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value: serde_json::Value = Deserialize::deserialize(deserializer)?;

    if value.is_null() {
        Ok(None)
    } else if let Some(b) = value.as_bool() {
        Ok(Some(b))
    } else {
        match value.as_str() {
            Some(s) => Ok(Some(s.parse().map_err(de::Error::custom)?)),
            None => Err(de::Error::custom("expected bool or string")),
        }
    }
}
