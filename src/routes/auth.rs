use std::collections::HashMap;
use std::sync::LazyLock;

use axum::{
    extract::{Path, Query, State},
    http::{header::SET_COOKIE, HeaderMap},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{
    db::profiles::{Profile, ProfileWallet},
    errors::AppError,
    services::jwt,
    AppState,
};

// ---------------------------------------------------------------------------
// Challenge store
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Challenge {
    message: String,
    address: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

static CHALLENGES: LazyLock<RwLock<HashMap<String, Challenge>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/challenge", get(get_challenge))
        .route("/connect", post(connect_wallet))
        .route("/profile", get(get_profile))
        .route("/wallets/{address}", delete(remove_wallet))
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChallengeQuery {
    address: String,
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
    // Try Authorization header first
    if let Some(token) = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
    {
        return Some(token);
    }
    // Fallback: read from cookie
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
    Query(query): Query<ChallengeQuery>,
) -> Result<Json<ChallengeResponse>, AppError> {
    // Generate 16-byte random nonce as hex
    let nonce_bytes: [u8; 16] = rand::random();
    let nonce = hex::encode(nonce_bytes);

    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::minutes(10);

    let issued_at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let expiration_time = expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    let message = format!(
        "Address: {}\n\nNonce: {}\n\nIssued At: {}\n\nExpiration Time: {}",
        query.address, nonce, issued_at, expiration_time
    );

    // Store challenge keyed by nonce
    {
        let mut challenges = CHALLENGES.write().await;
        // Cleanup expired challenges while we're here
        let now = chrono::Utc::now();
        challenges.retain(|_, c| c.expires_at > now);

        challenges.insert(
            nonce.clone(),
            Challenge {
                message: message.clone(),
                address: query.address,
                expires_at,
            },
        );
    }

    Ok(Json(ChallengeResponse {
        message,
        nonce,
        issued_at,
        expiration_time,
    }))
}

// ---------------------------------------------------------------------------
// POST /connect
// ---------------------------------------------------------------------------

async fn connect_wallet(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ConnectRequest>,
) -> Result<impl IntoResponse, AppError> {
    // 1. Verify the challenge exists and hasn't expired
    {
        let mut challenges = CHALLENGES.write().await;
        let challenge = challenges
            .remove(&body.nonce)
            .ok_or_else(|| AppError::BadRequest("Invalid or expired challenge nonce".into()))?;

        if challenge.expires_at < chrono::Utc::now() {
            return Err(AppError::BadRequest("Challenge has expired".into()));
        }

        if challenge.address != body.ordinals_address {
            return Err(AppError::BadRequest("Challenge address mismatch".into()));
        }

        if challenge.message != body.message {
            return Err(AppError::BadRequest("Challenge message mismatch".into()));
        }
    }

    // 2. Verify BIP-322 signature
    bip322::verify_simple_encoded(&body.ordinals_address, &body.message, &body.signature)
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
            let label = format!("Wallet {}", wallet_count + 1);

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

    db.add_wallet_to_profile(
        profile.id,
        &body.payment_address,
        &body.ordinals_address,
        "Wallet 1",
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
    let token_str = extract_token(&headers)
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
    let token_str = extract_token(&headers)
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

    let wallet_count = state.db.count_profile_wallets(claims.profile_id).await?;
    if wallet_count <= 1 {
        return Err(AppError::BadRequest(
            "Cannot remove last wallet".to_string(),
        ));
    }

    let removed = state
        .db
        .remove_wallet_from_profile(claims.profile_id, &address)
        .await?;

    if !removed {
        return Err(AppError::NotFound("Wallet not found".to_string()));
    }

    let profile = state
        .db
        .get_profile_by_id(claims.profile_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Profile not found".to_string()))?;

    let wallets = state.db.get_profile_wallets(profile.id).await?;

    Ok(Json(build_profile_response(&profile, &wallets)))
}
