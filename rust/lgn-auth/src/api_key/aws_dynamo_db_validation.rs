use async_trait::async_trait;
use aws_sdk_dynamodb::model::AttributeValue;
use aws_sdk_dynamodb::Region;
use micromegas_tracing::span_fn;

use super::{Error, Result};

use super::{ApiKey, ApiKeyValidator};

pub struct AwsDynamoDbValidation {
    table_name: String,
    client: aws_sdk_dynamodb::Client,
}

impl AwsDynamoDbValidation {
    #[span_fn]
    pub async fn new(region: Option<String>, table_name: impl Into<String>) -> Result<Self> {
        let config = aws_config::from_env();

        let config = if let Some(region) = region {
            let region = Region::new(region);

            config.region(region)
        } else {
            config
        }
        .load()
        .await;

        let client = aws_sdk_dynamodb::Client::new(&config);
        let table_name = table_name.into();

        Ok(Self { table_name, client })
    }

    fn get_api_key_attr(api_key: ApiKey) -> AttributeValue {
        AttributeValue::S(api_key.0)
    }
}

#[async_trait]
impl ApiKeyValidator for AwsDynamoDbValidation {
    async fn validate_api_key(&self, api_key: ApiKey) -> Result<()> {
        let api_key_attr = Self::get_api_key_attr(api_key.clone());

        match self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("api_key", api_key_attr)
            .send()
            .await
        {
            Ok(output) => output.item.ok_or(Error::InvalidApiKey(api_key)).map(|_| ()),
            Err(err) => Err(Error::Unspecified(format!(
                "unexpected error while reading API key from AWS DynamoDB: {}",
                err
            ))),
        }
    }
}
