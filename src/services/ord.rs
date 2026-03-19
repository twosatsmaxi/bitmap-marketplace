use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use reqwest::Client;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Response structs
// ---------------------------------------------------------------------------

/// A single inscription as returned by GET /inscription/{id}
#[derive(Debug, Clone, Deserialize)]
pub struct OrdInscription {
    pub id: String,
    pub number: i64,
    /// The current owner address. May be absent for unconfirmed inscriptions.
    pub address: Option<String>,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    /// The sat on which this inscription is written.
    pub sat: Option<u64>,
    /// The block height where this inscription was created.
    pub height: Option<u64>,
    pub genesis_height: Option<u64>,
    pub genesis_timestamp: Option<i64>,
    /// Output value of the UTXO holding the inscription.
    pub value: Option<u64>,
    /// Number of child inscriptions.
    #[serde(default)]
    pub child_count: u64,
    /// Child inscription IDs.
    #[serde(default)]
    pub children: Vec<String>,
}

/// A page of inscription IDs as returned by GET /inscriptions?page={n}
#[derive(Debug, Clone, Deserialize)]
pub struct InscriptionPage {
    pub inscriptions: Vec<String>,
    pub page_index: u32,
    pub more: bool,
    pub page_size: u32,
}

/// Address info as returned by GET /address/{addr}
#[derive(Debug, Clone, Deserialize)]
pub struct OrdAddressResponse {
    #[serde(default)]
    pub inscriptions: Vec<String>,
}

/// Sat info as returned by GET /sat/{n}
#[derive(Debug, Clone, Deserialize)]
pub struct SatInfo {
    pub number: u64,
    pub decimal: Option<String>,
    pub degree: Option<String>,
    pub percentile: Option<String>,
    pub name: Option<String>,
    pub height: Option<u64>,
    pub cycle: Option<u32>,
    pub epoch: Option<u32>,
    pub period: Option<u32>,
    pub offset: Option<u64>,
    pub rarity: Option<String>,
    pub timestamp: Option<i64>,
    /// The inscription ID sitting on this sat, if any.
    pub inscription: Option<String>,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// HTTP client for the ord REST API.
#[derive(Clone)]
pub struct OrdClient {
    base_url: String,
    http: Client,
}

impl OrdClient {
    /// Create a new `OrdClient`.
    ///
    /// The base URL is taken from the `ORD_URL` environment variable,
    /// falling back to `http://127.0.0.1:80` when the variable is absent.
    pub fn new() -> Self {
        let base_url =
            std::env::var("ORD_URL").unwrap_or_else(|_| "http://127.0.0.1:80".to_string());
        Self {
            base_url,
            http: Client::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Issue a GET request that expects a JSON body.
    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .with_context(|| format!("GET {url}: request failed"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable body>".to_string());
            return Err(anyhow!("GET {url}: HTTP {status}: {body}"));
        }

        response
            .json::<T>()
            .await
            .with_context(|| format!("GET {url}: failed to deserialize JSON response"))
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Fetch a single inscription by its ID.
    ///
    /// Calls `GET /inscription/{id}`.
    pub async fn get_inscription(&self, id: &str) -> Result<OrdInscription> {
        self.get_json::<OrdInscription>(&format!("/inscription/{id}"))
            .await
    }

    /// Fetch the raw content bytes for an inscription.
    ///
    /// Calls `GET /content/{id}` and returns the response body verbatim,
    /// which may be any media type (image, text, JSON, …).
    pub async fn get_inscription_content(&self, id: &str) -> Result<Bytes> {
        let url = format!("{}/content/{id}", self.base_url);
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}: request failed"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable body>".to_string());
            return Err(anyhow!("GET {url}: HTTP {status}: {body}"));
        }

        response
            .bytes()
            .await
            .with_context(|| format!("GET {url}: failed to read response bytes"))
    }

    /// Fetch a page of inscription IDs.
    ///
    /// Calls `GET /inscriptions?page={page}`.
    pub async fn list_inscriptions(&self, page: u32) -> Result<InscriptionPage> {
        self.get_json::<InscriptionPage>(&format!("/inscriptions?page={page}"))
            .await
    }

    /// Fetch all inscriptions owned by a Bitcoin address.
    ///
    /// Calls `GET /inscriptions/address/{addr}?page={n}` repeatedly until
    /// `more` is `false`, then resolves each ID to a full `OrdInscription`
    /// via `get_inscription`.
    ///
    /// # Note
    /// The ord REST API does not expose a dedicated per-address endpoint in all
    /// versions. This implementation targets the `/inscriptions/address/{addr}`
    /// path. If your ord node version uses a different route, adjust accordingly.
    pub async fn get_inscriptions_by_address(&self, addr: &str) -> Result<Vec<OrdInscription>> {
        let mut all_ids: Vec<String> = Vec::new();
        let mut page: u32 = 0;

        // Paginate through all pages until the server signals there are no more.
        loop {
            let page_data = self
                .get_json::<InscriptionPage>(&format!("/inscriptions/address/{addr}?page={page}"))
                .await?;

            all_ids.extend(page_data.inscriptions);

            if !page_data.more {
                break;
            }
            page += 1;
        }

        // Resolve every inscription ID to a full inscription object.
        // We issue requests sequentially to avoid overwhelming the ord node.
        let mut inscriptions: Vec<OrdInscription> = Vec::with_capacity(all_ids.len());
        for id in &all_ids {
            let inscription = self.get_inscription(id).await?;
            inscriptions.push(inscription);
        }

        Ok(inscriptions)
    }

    /// Fetch all inscription IDs owned by a Bitcoin address.
    ///
    /// Calls `GET /address/{address}` and returns just the inscription IDs.
    pub async fn get_address_inscription_ids(&self, address: &str) -> Result<Vec<String>> {
        let resp = self
            .get_json::<OrdAddressResponse>(&format!("/address/{address}"))
            .await?;
        Ok(resp.inscriptions)
    }

    /// Fetch sat metadata by sat number.
    ///
    /// Calls `GET /sat/{sat}`.
    pub async fn get_sat_info(&self, sat: u64) -> Result<SatInfo> {
        self.get_json::<SatInfo>(&format!("/sat/{sat}")).await
    }
}

impl Default for OrdClient {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-var tests to avoid race conditions between parallel test threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_ord_client_default_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("ORD_URL");
        let client = OrdClient::new();
        assert_eq!(client.base_url, "http://127.0.0.1:80");
    }

    #[test]
    fn test_ord_client_custom_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("ORD_URL", "http://ord.example.com:8080");
        let client = OrdClient::new();
        assert_eq!(client.base_url, "http://ord.example.com:8080");
        std::env::remove_var("ORD_URL");
    }

    #[test]
    fn test_deserialize_inscription() {
        let json = r#"{
            "id": "abc123i0",
            "number": 42,
            "address": "bc1pxxx",
            "content_type": "text/plain",
            "content_length": 12,
            "sat": 1234567,
            "genesis_height": 800000,
            "genesis_timestamp": 1700000000,
            "value": 10000
        }"#;
        let inscription: OrdInscription = serde_json::from_str(json).unwrap();
        assert_eq!(inscription.id, "abc123i0");
        assert_eq!(inscription.number, 42);
        assert_eq!(inscription.sat, Some(1234567));
        assert_eq!(inscription.value, Some(10000));
    }

    #[test]
    fn test_deserialize_inscription_page() {
        let json = r#"{
            "inscriptions": ["id1", "id2"],
            "page_index": 0,
            "more": true,
            "page_size": 100
        }"#;
        let page: InscriptionPage = serde_json::from_str(json).unwrap();
        assert_eq!(page.inscriptions, vec!["id1", "id2"]);
        assert!(page.more);
        assert_eq!(page.page_size, 100);
    }

    #[test]
    fn test_deserialize_sat_info() {
        let json = r#"{
            "number": 1234567,
            "decimal": "3.1234567",
            "degree": "0°3′15″1234567‴",
            "percentile": "0.005924%",
            "name": "satoshi",
            "height": 800000,
            "cycle": 0,
            "epoch": 3,
            "period": 399,
            "offset": 0,
            "rarity": "common",
            "timestamp": 1700000000,
            "inscription": "abc123i0"
        }"#;
        let sat: SatInfo = serde_json::from_str(json).unwrap();
        assert_eq!(sat.number, 1234567);
        assert_eq!(sat.rarity.as_deref(), Some("common"));
        assert_eq!(sat.inscription.as_deref(), Some("abc123i0"));
    }
}
