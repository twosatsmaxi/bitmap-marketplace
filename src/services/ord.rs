use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;

pub struct OrdClient {
    base_url: String,
    http: Client,
}

#[derive(Debug, Deserialize)]
pub struct OrdInscription {
    pub id: String,
    pub number: i64,
    pub address: String,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub sat: Option<u64>,
    pub genesis_height: Option<u64>,
    pub genesis_timestamp: Option<u64>,
}

impl OrdClient {
    pub fn new() -> Self {
        let base_url = std::env::var("ORD_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:80".to_string());
        Self {
            base_url,
            http: Client::new(),
        }
    }

    pub async fn get_inscription(&self, inscription_id: &str) -> Result<OrdInscription> {
        let url = format!("{}/inscription/{}", self.base_url, inscription_id);
        let resp = self.http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?
            .json::<OrdInscription>()
            .await?;
        Ok(resp)
    }
}
