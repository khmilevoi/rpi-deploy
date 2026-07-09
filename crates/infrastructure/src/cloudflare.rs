use async_trait::async_trait;
use base64::Engine as _;
use pi_domain::contracts::{CloudflareApi, TunnelCreds};
use pi_domain::error::DomainError;
use rand::RngCore;

const API: &str = "https://api.cloudflare.com/client/v4";

fn api_err(msg: impl std::fmt::Display) -> DomainError {
    DomainError::Ingress(format!("cloudflare api: {msg}"))
}

/// The credentials file a locally-managed tunnel reads at runtime (no cert.pem).
pub fn credentials_json(creds: &TunnelCreds) -> String {
    serde_json::json!({
        "AccountTag": creds.account_tag,
        "TunnelID": creds.tunnel_id,
        "TunnelName": creds.tunnel_name,
        "TunnelSecret": creds.tunnel_secret,
    })
    .to_string()
}

/// 32 random bytes, base64-standard — the tunnel secret shared with create.
pub fn new_tunnel_secret() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::STANDARD.encode(buf)
}

pub struct HttpCloudflare {
    token: String,
    account_id: Option<String>,
    client: reqwest::Client,
    base: String,
}

impl HttpCloudflare {
    pub fn new(token: String, account_id: Option<String>) -> HttpCloudflare {
        HttpCloudflare {
            token,
            account_id,
            client: reqwest::Client::new(),
            base: API.to_string(),
        }
    }
}

#[async_trait]
impl CloudflareApi for HttpCloudflare {
    async fn zone_id(&self, zone: &str) -> Result<String, DomainError> {
        let v: serde_json::Value = self
            .client
            .get(format!("{}/zones?name={zone}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(api_err)?
            .json()
            .await
            .map_err(api_err)?;
        v["result"][0]["id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| api_err(format!("zone {zone} not found")))
    }

    async fn find_or_create_tunnel(&self, name: &str) -> Result<TunnelCreds, DomainError> {
        let account = self
            .account_id
            .clone()
            .ok_or_else(|| api_err("account_id required"))?;
        // adopt existing by name
        let list: serde_json::Value = self
            .client
            .get(format!(
                "{}/accounts/{account}/cfd_tunnel?name={name}&is_deleted=false",
                self.base
            ))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(api_err)?
            .json()
            .await
            .map_err(api_err)?;
        if let Some(id) = list["result"][0]["id"].as_str() {
            // Existing tunnel: we cannot recover its secret, so the caller must
            // already hold creds. Signal adoption with an empty secret.
            return Ok(TunnelCreds {
                account_tag: account,
                tunnel_id: id.to_string(),
                tunnel_name: name.to_string(),
                tunnel_secret: String::new(),
            });
        }
        let secret = new_tunnel_secret();
        let created: serde_json::Value = self
            .client
            .post(format!("{}/accounts/{account}/cfd_tunnel", self.base))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "name": name, "tunnel_secret": secret }))
            .send()
            .await
            .map_err(api_err)?
            .json()
            .await
            .map_err(api_err)?;
        let id = created["result"]["id"]
            .as_str()
            .ok_or_else(|| api_err("create tunnel: no id in response"))?;
        Ok(TunnelCreds {
            account_tag: account,
            tunnel_id: id.to_string(),
            tunnel_name: name.to_string(),
            tunnel_secret: secret,
        })
    }

    async fn put_tunnel_cname(
        &self,
        zone: &str,
        name: &str,
        tunnel_id: &str,
    ) -> Result<(), DomainError> {
        let zid = self.zone_id(zone).await?;
        let content = format!("{tunnel_id}.cfargotunnel.com");
        // find existing record id
        let existing: serde_json::Value = self
            .client
            .get(format!(
                "{}/zones/{zid}/dns_records?type=CNAME&name={name}",
                self.base
            ))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(api_err)?
            .json()
            .await
            .map_err(api_err)?;
        let body = serde_json::json!({
            "type": "CNAME", "name": name, "content": content, "proxied": true
        });
        let req = match existing["result"][0]["id"].as_str() {
            Some(rid) => self
                .client
                .put(format!("{}/zones/{zid}/dns_records/{rid}", self.base)),
            None => self
                .client
                .post(format!("{}/zones/{zid}/dns_records", self.base)),
        };
        req.bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(api_err)?
            .error_for_status()
            .map_err(api_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_json_has_the_four_fields() {
        let creds = TunnelCreds {
            account_tag: "acc".into(),
            tunnel_id: "tid".into(),
            tunnel_name: "myboard".into(),
            tunnel_secret: "c2VjcmV0".into(),
        };
        let v: serde_json::Value = serde_json::from_str(&credentials_json(&creds)).unwrap();
        assert_eq!(v["AccountTag"], "acc");
        assert_eq!(v["TunnelID"], "tid");
        assert_eq!(v["TunnelName"], "myboard");
        assert_eq!(v["TunnelSecret"], "c2VjcmV0");
    }

    #[test]
    fn tunnel_secret_is_32_bytes_base64() {
        let s = new_tunnel_secret();
        let raw = base64::engine::general_purpose::STANDARD.decode(s).unwrap();
        assert_eq!(raw.len(), 32);
    }
}
