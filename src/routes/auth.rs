use axum::{
    extract::{Path, Query, State},
    http::{header::{CACHE_CONTROL, SET_COOKIE}, HeaderMap},
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Json, Router,
};
use bitcoin::address::NetworkUnchecked;
use bitcoin::Network;
use serde::{Deserialize, Serialize};


use crate::{
    db::profiles::{Profile, ProfileWallet},
    errors::AppError,
    services::jwt,
    AppState,
};

// ---------------------------------------------------------------------------
// Challenge store (lives in AppState, not a global static)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Challenge {
    pub message: String,
    pub address: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/challenge", get(get_challenge))
        .route("/connect", post(connect_wallet))
        .route("/profile", get(get_profile))
        .route("/wallets/:address", patch(update_wallet_label))
        .route("/wallets/:address", delete(remove_wallet))
        .route("/logout", post(logout))
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChallengeQuery {
    address: String,
}

/// Validates a Bitcoin address string without requiring RPC.
/// Returns true if the address is syntactically valid AND valid for the 
/// specified network (mainnet, testnet, or regtest).
fn validate_bitcoin_address(address: &str, network: Network) -> bool {
    // First, try to parse as an unchecked address (validates bech32 encoding, checksum)
    let unchecked = match address.parse::<bitcoin::Address<NetworkUnchecked>>() {
        Ok(a) => a,
        Err(_) => return false,
    };
    
    // Check if the address is valid for the specified network
    unchecked.is_valid_for_network(network)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeResponse {
    message: String,
    nonce: String,
    issued_at: String,
    expiration_time: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectRequest {
    payment_address: String,
    ordinals_address: String,
    signature: String,
    message: String,
    nonce: String,
    label: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateWalletLabelRequest {
    label: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthResponse {
    token: String,
    profile: ProfileResponse,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileResponse {
    id: String,
    primary_address: String,
    wallets: Vec<WalletResponse>,
    created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WalletResponse {
    payment_address: String,
    ordinals_address: String,
    label: String,
    linked_at: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_token(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
    {
        return Some(token);
    }
    headers
        .get("Cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';')
                .find_map(|c| {
                    let c = c.trim();
                    c.strip_prefix("bitmap_token=").map(|t| t.to_string())
                })
        })
}

/// Authenticate a request: extract token, verify JWT, load profile, check token_version.
pub async fn authenticate(headers: &HeaderMap, state: &AppState) -> Result<Profile, AppError> {
    let token_str = extract_token(headers)
        .ok_or_else(|| AppError::Unauthorized("Missing authorization token".to_string()))?;

    let claims = jwt::verify_token(&token_str, &state.jwt_secret)
        .map_err(|_| AppError::Unauthorized("Invalid or expired token".to_string()))?;

    let profile = state
        .db
        .get_profile_by_id(claims.profile_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Profile not found".to_string()))?;

    if profile.token_version != claims.token_version {
        return Err(AppError::Unauthorized("Token has been revoked".into()));
    }

    Ok(profile)
}

fn auth_response_with_cookie(auth_resp: AuthResponse, token: &str) -> impl IntoResponse {
    let cookie = format!(
        "bitmap_token={}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={}",
        token,
        30 * 24 * 60 * 60 // 30 days
    );
    (
        [(SET_COOKIE, cookie)],
        Json(auth_resp),
    )
}

fn build_profile_response(profile: &Profile, wallets: &[ProfileWallet]) -> ProfileResponse {
    ProfileResponse {
        id: profile.id.to_string(),
        primary_address: profile.primary_address.clone(),
        wallets: wallets
            .iter()
            .map(|w| WalletResponse {
                payment_address: w.payment_address.clone(),
                ordinals_address: w.ordinals_address.clone(),
                label: w.label.clone(),
                linked_at: w.linked_at.to_rfc3339(),
            })
            .collect(),
        created_at: profile.created_at.to_rfc3339(),
    }
}

// ---------------------------------------------------------------------------
// GET /challenge
// ---------------------------------------------------------------------------

async fn get_challenge(
    State(state): State<AppState>,
    Query(query): Query<ChallengeQuery>,
) -> Result<impl IntoResponse, AppError> {
    // Validate the address format before creating a challenge
    if !validate_bitcoin_address(&query.address, state.allowed_address_network) {
        return Err(AppError::BadRequest(format!(
            "Invalid Bitcoin address: {}",
            query.address
        )));
    }

    let nonce_bytes: [u8; 16] = rand::random();
    let nonce = hex::encode(nonce_bytes);

    // Capture current time once to prevent TOCTOU issues
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::minutes(10);

    let issued_at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let expiration_time = expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    let message = format!(
        "Address: {}\n\nNonce: {}\n\nIssued At: {}\n\nExpiration Time: {}",
        query.address, nonce, issued_at, expiration_time
    );

    state.challenges.insert(
        nonce.clone(),
        Challenge {
            message: message.clone(),
            address: query.address,
            expires_at,
        },
    );

    Ok((
        [(CACHE_CONTROL, "no-store, no-cache")],
        Json(ChallengeResponse {
            message,
            nonce,
            issued_at,
            expiration_time,
        }),
    ))
}

// ---------------------------------------------------------------------------
// POST /connect
// ---------------------------------------------------------------------------

async fn connect_wallet(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ConnectRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate both addresses before processing
    if !validate_bitcoin_address(&body.payment_address, state.allowed_address_network) {
        return Err(AppError::BadRequest(format!(
            "Invalid payment address: {}",
            body.payment_address
        )));
    }
    if !validate_bitcoin_address(&body.ordinals_address, state.allowed_address_network) {
        return Err(AppError::BadRequest(format!(
            "Invalid ordinals address: {}",
            body.ordinals_address
        )));
    }

    // Capture current time once to prevent TOCTOU issues
    let now = chrono::Utc::now();

    // 1. Verify the challenge exists and hasn't expired
    let challenge = {
        state.challenges
            .remove(&body.nonce)
            .ok_or_else(|| AppError::BadRequest("Challenge nonce not found — it may have expired or been invalidated. Please request a new challenge and try again.".into()))?
    };

    // Use the captured time for expiration check (TOCTOU fix)
    if challenge.expires_at < now {
        return Err(AppError::BadRequest("Challenge has expired".into()));
    }

    if challenge.address != body.ordinals_address {
        return Err(AppError::BadRequest("Challenge address mismatch".into()));
    }

    if challenge.message != body.message {
        return Err(AppError::BadRequest("Challenge message mismatch".into()));
    }

    // 2. Verify BIP-322 signature against the stored challenge.message
    // (not body.message) to eliminate any possibility of mismatch
    bip322::verify_simple_encoded(&body.ordinals_address, &challenge.message, &body.signature)
        .map_err(|e| {
            tracing::warn!(
                "BIP-322 verification failed for {}: {:?}",
                body.ordinals_address,
                e
            );
            AppError::Unauthorized("Invalid wallet signature".into())
        })?;

    let db = &state.db;

    // 3. Check if a profile already exists for this ordinals address.
    if let Some(profile) = db.get_profile_by_address(&body.ordinals_address).await? {
        let wallets = db.get_profile_wallets(profile.id).await?;
        let token = jwt::create_token(profile.id, &profile.primary_address, profile.token_version, &state.jwt_secret)
            .map_err(|e| AppError::Internal(e))?;

        let response = AuthResponse {
            token: token.clone(),
            profile: build_profile_response(&profile, &wallets),
        };
        return Ok(auth_response_with_cookie(response, &token));
    }

    // 4. No existing profile for this address — check for a JWT to link to an
    //    existing profile.
    if let Some(token_str) = extract_token(&headers) {
        if let Ok(claims) = jwt::verify_token(&token_str, &state.jwt_secret) {
            let profile = db
                .get_profile_by_id(claims.profile_id)
                .await?
                .ok_or_else(|| AppError::NotFound("Profile not found".to_string()))?;

            let wallet_count = db.count_profile_wallets(profile.id).await?;
            let label = body.label.clone().unwrap_or_else(|| format!("Wallet {}", wallet_count + 1));

            db.add_wallet_to_profile(
                profile.id,
                &body.payment_address,
                &body.ordinals_address,
                &label,
            )
            .await?;

            let wallets = db.get_profile_wallets(profile.id).await?;
            let token =
                jwt::create_token(profile.id, &profile.primary_address, profile.token_version, &state.jwt_secret)
                    .map_err(|e| AppError::Internal(e))?;

            let response = AuthResponse {
                token: token.clone(),
                profile: build_profile_response(&profile, &wallets),
            };
            return Ok(auth_response_with_cookie(response, &token));
        }
    }

    // 5. No JWT or invalid JWT — create a brand-new profile.
    let profile = db.create_profile(&body.ordinals_address).await?;

    let label = body.label.clone().unwrap_or_else(|| "Wallet 1".to_string());
    db.add_wallet_to_profile(
        profile.id,
        &body.payment_address,
        &body.ordinals_address,
        &label,
    )
    .await?;

    let wallets = db.get_profile_wallets(profile.id).await?;
    let token = jwt::create_token(profile.id, &profile.primary_address, profile.token_version, &state.jwt_secret)
        .map_err(|e| AppError::Internal(e))?;

    let response = AuthResponse {
        token: token.clone(),
        profile: build_profile_response(&profile, &wallets),
    };
    Ok(auth_response_with_cookie(response, &token))
}

// ---------------------------------------------------------------------------
// GET /profile
// ---------------------------------------------------------------------------

async fn get_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ProfileResponse>, AppError> {
    let profile = authenticate(&headers, &state).await?;
    let wallets = state.db.get_profile_wallets(profile.id).await?;
    Ok(Json(build_profile_response(&profile, &wallets)))
}

// ---------------------------------------------------------------------------
// DELETE /wallets/:address
// ---------------------------------------------------------------------------

async fn remove_wallet(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(address): Path<String>,
) -> Result<Json<ProfileResponse>, AppError> {
    let profile = authenticate(&headers, &state).await?;

    // remove_wallet_from_profile checks count internally and returns false if last wallet
    let removed = state
        .db
        .remove_wallet_from_profile(profile.id, &address)
        .await?;

    if !removed {
        // Could be last wallet or wallet not found — check which
        let wallet_count = state.db.count_profile_wallets(profile.id).await?;
        if wallet_count <= 1 {
            return Err(AppError::BadRequest("Cannot remove last wallet".to_string()));
        }
        return Err(AppError::NotFound("Wallet not found".to_string()));
    }

    let wallets = state.db.get_profile_wallets(profile.id).await?;
    Ok(Json(build_profile_response(&profile, &wallets)))
}

// ---------------------------------------------------------------------------
// PATCH /wallets/:address
// ---------------------------------------------------------------------------

async fn update_wallet_label(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(address): Path<String>,
    Json(body): Json<UpdateWalletLabelRequest>,
) -> Result<Json<ProfileResponse>, AppError> {
    let profile = authenticate(&headers, &state).await?;

    // Validate the address format
    if !validate_bitcoin_address(&address, state.allowed_address_network) {
        return Err(AppError::BadRequest(format!(
            "Invalid Bitcoin address: {}",
            address
        )));
    }

    // Validate label is not empty
    let label = body.label.trim();
    if label.is_empty() {
        return Err(AppError::BadRequest("Label cannot be empty".to_string()));
    }

    // Update the wallet label
    let updated = state
        .db
        .update_wallet_label(profile.id, &address, label)
        .await?;

    if !updated {
        return Err(AppError::NotFound("Wallet not found".to_string()));
    }

    let wallets = state.db.get_profile_wallets(profile.id).await?;
    Ok(Json(build_profile_response(&profile, &wallets)))
}

// ---------------------------------------------------------------------------
// POST /logout
// ---------------------------------------------------------------------------

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    // If the user has a valid token, revoke it by bumping token_version
    if let Ok(profile) = authenticate(&headers, &state).await {
        state.db.increment_token_version(profile.id).await?;
    }

    // Always clear the cookie, even if token was missing/invalid
    let cookie = "bitmap_token=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0";
    Ok(([(SET_COOKIE, cookie)], Json(serde_json::json!({"ok": true}))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    // -----------------------------------------------------------------------
    // validate_bitcoin_address
    // -----------------------------------------------------------------------

    #[test]
    fn valid_mainnet_bech32_address() {
        // P2WPKH mainnet address
        assert!(validate_bitcoin_address(
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
            Network::Bitcoin
        ));
    }

    #[test]
    fn valid_mainnet_taproot_address() {
        assert!(validate_bitcoin_address(
            "bc1pg9hmzwhpveyz4r0x7gn3farvz4hnuf9s2h26lpuefdr40fcgfdlqcpfu6p",
            Network::Bitcoin
        ));
    }

    #[test]
    fn testnet_address_rejected_on_mainnet() {
        // tb1 prefix = testnet
        assert!(!validate_bitcoin_address(
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx",
            Network::Bitcoin
        ));
    }

    #[test]
    fn garbage_input_rejected() {
        assert!(!validate_bitcoin_address("not-an-address", Network::Bitcoin));
        assert!(!validate_bitcoin_address("", Network::Bitcoin));
        assert!(!validate_bitcoin_address("bc1", Network::Bitcoin));
    }

    #[test]
    fn testnet_address_accepted_on_testnet() {
        assert!(validate_bitcoin_address(
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx",
            Network::Testnet
        ));
    }

    // -----------------------------------------------------------------------
    // extract_token
    // -----------------------------------------------------------------------

    #[test]
    fn extract_token_from_bearer_header() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer my-jwt-token".parse().unwrap());
        assert_eq!(extract_token(&headers), Some("my-jwt-token".to_string()));
    }

    #[test]
    fn extract_token_from_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert("Cookie", "bitmap_token=cookie-jwt; other=val".parse().unwrap());
        assert_eq!(extract_token(&headers), Some("cookie-jwt".to_string()));
    }

    #[test]
    fn bearer_takes_precedence_over_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer bearer-jwt".parse().unwrap());
        headers.insert("Cookie", "bitmap_token=cookie-jwt".parse().unwrap());
        assert_eq!(extract_token(&headers), Some("bearer-jwt".to_string()));
    }

    #[test]
    fn extract_token_returns_none_when_missing() {
        let headers = HeaderMap::new();
        assert_eq!(extract_token(&headers), None);
    }

    #[test]
    fn non_bearer_auth_scheme_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Basic dXNlcjpwYXNz".parse().unwrap());
        assert_eq!(extract_token(&headers), None);
    }

    // -----------------------------------------------------------------------
    // build_profile_response
    // -----------------------------------------------------------------------

    #[test]
    fn build_profile_response_maps_fields() {
        let now = chrono::Utc::now();
        let profile = Profile {
            id: uuid::Uuid::new_v4(),
            primary_address: "bc1qtest".to_string(),
            token_version: 1,
            created_at: now,
            updated_at: now,
        };
        let wallets = vec![ProfileWallet {
            id: uuid::Uuid::new_v4(),
            profile_id: profile.id,
            payment_address: "bc1qpay".to_string(),
            ordinals_address: "bc1qord".to_string(),
            label: "Wallet 1".to_string(),
            linked_at: now,
        }];

        let resp = build_profile_response(&profile, &wallets);
        assert_eq!(resp.id, profile.id.to_string());
        assert_eq!(resp.primary_address, "bc1qtest");
        assert_eq!(resp.wallets.len(), 1);
        assert_eq!(resp.wallets[0].payment_address, "bc1qpay");
        assert_eq!(resp.wallets[0].ordinals_address, "bc1qord");
        assert_eq!(resp.wallets[0].label, "Wallet 1");
    }

    // -----------------------------------------------------------------------
    // auth_response_with_cookie
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn cookie_has_security_attributes() {
        let now = chrono::Utc::now();
        let profile = Profile {
            id: uuid::Uuid::new_v4(),
            primary_address: "bc1q".to_string(),
            token_version: 1,
            created_at: now,
            updated_at: now,
        };
        let resp = AuthResponse {
            token: "fake-token".to_string(),
            profile: ProfileResponse {
                id: profile.id.to_string(),
                primary_address: profile.primary_address.clone(),
                wallets: vec![],
                created_at: now.to_rfc3339(),
            },
        };

        let response = auth_response_with_cookie(resp, "fake-token").into_response();
        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("Set-Cookie header missing")
            .to_str()
            .unwrap();

        assert!(cookie.contains("HttpOnly"), "missing HttpOnly");
        assert!(cookie.contains("Secure"), "missing Secure");
        assert!(cookie.contains("SameSite=Strict"), "missing SameSite=Strict");
        assert!(cookie.contains("Path=/"), "missing Path=/");
        assert!(cookie.contains("Max-Age=2592000"), "missing Max-Age");
        assert!(cookie.starts_with("bitmap_token=fake-token;"), "wrong cookie name/value");
    }

    // -----------------------------------------------------------------------
    // update_wallet_label request validation
    // -----------------------------------------------------------------------

    #[test]
    fn update_wallet_label_request_deserializes() {
        let json = r#"{"label":"My Updated Wallet"}"#;
        let req: UpdateWalletLabelRequest = serde_json::from_str(json).expect("should parse");
        assert_eq!(req.label, "My Updated Wallet");
    }

    #[test]
    fn update_wallet_label_request_camel_case() {
        let json = r#"{"label":"Test Label"}"#;
        let req: UpdateWalletLabelRequest = serde_json::from_str(json).expect("should parse");
        assert_eq!(req.label, "Test Label");
    }
}
