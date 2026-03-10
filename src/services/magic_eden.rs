use anyhow::Result;
use serde::Deserialize;

pub struct MagicEdenListing {
    pub inscription_id: String,
    pub price_sats: i64,
    pub signed_psbt: Option<String>,
    pub seller_address: String,
}

#[derive(Debug, Deserialize)]
struct MeToken {
    id: Option<String>,
    #[serde(rename = "inscriptionId")]
    inscription_id: Option<String>,
    #[serde(rename = "listPrice")]
    list_price: Option<serde_json::Value>,
    #[serde(rename = "signedPsbt")]
    signed_psbt: Option<String>,
    owner: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MeListingsResponse {
    tokens: Option<Vec<MeToken>>,
}

pub async fn fetch_listings_by_seller(seller_address: &str) -> Result<Vec<MagicEdenListing>> {
    let client = reqwest::Client::new();
    let mut all_listings = Vec::new();
    let limit = 100usize;
    let mut offset = 0usize;

    loop {
        let url = format!(
            "https://api-mainnet.magiceden.dev/v2/ord/btc/wallets/{}/tokens?listed=true&limit={}&offset={}",
            seller_address, limit, offset
        );

        let resp = client
            .get(&url)
            .header("User-Agent", "bitmap-marketplace/1.0")
            .send()
            .await?;

        if !resp.status().is_success() {
            // Non-200 (e.g. 404 for unknown address) — treat as empty
            break;
        }

        let body: MeListingsResponse = resp.json().await?;
        let tokens = match body.tokens {
            Some(t) => t,
            None => break,
        };

        let count = tokens.len();
        for token in tokens {
            // inscription_id may be in `id` or `inscriptionId` depending on API version
            let inscription_id = token
                .inscription_id
                .or(token.id)
                .unwrap_or_default();

            if inscription_id.is_empty() {
                continue;
            }

            let price_sats = match &token.list_price {
                Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0),
                Some(serde_json::Value::String(s)) => s.parse::<i64>().unwrap_or(0),
                _ => 0,
            };

            all_listings.push(MagicEdenListing {
                inscription_id,
                price_sats,
                signed_psbt: token.signed_psbt,
                seller_address: token.owner.unwrap_or_else(|| seller_address.to_string()),
            });
        }

        if count < limit {
            break;
        }
        offset += limit;
    }

    Ok(all_listings)
}
