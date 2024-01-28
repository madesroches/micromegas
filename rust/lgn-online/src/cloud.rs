use std::net::IpAddr;

use crate::Result;

static X_AWS_EC2_METADATA_TOKEN_TTL_SECONDS: &str = "X-aws-ec2-metadata-token-ttl-seconds";
static X_AWS_EC2_METADATA_TOKEN: &str = "X-aws-ec2-metadata-token";

/// Contact the AWS metadata instance to query the public IP address of the
/// host.
pub async fn get_aws_ec2_metadata_public_ipv4() -> Result<IpAddr> {
    // TOKEN=`curl -X PUT "" -H "X-aws-ec2-metadata-token-ttl-seconds: 21600"` \
    // && curl -H "X-aws-ec2-metadata-token: $TOKEN" http://169.254.169.254/latest/meta-data/public-ipv4

    let client = reqwest::Client::new();

    let token = client
        .put("http://169.254.169.254/latest/api/token")
        .header(X_AWS_EC2_METADATA_TOKEN_TTL_SECONDS, "21600")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    Ok(client
        .get("http://169.254.169.254/latest/meta-data/public-ipv4")
        .header(X_AWS_EC2_METADATA_TOKEN, token)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?
        .parse()?)
}
