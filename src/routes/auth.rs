use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use crate::{
    db::profiles::{Profile, ProfileWallet},
    errors::AppError,
    services::jwt,
    AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/connect", post(connect_wallet))
        .route("/profile", get(get_profile))
        .route("/wallets/{address}", delete(remove_wallet))
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectRequest {
    payment_address: String,
    ordinals_address: String,
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
    headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
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
// POST /connect
// ---------------------------------------------------------------------------

async fn connect_wallet(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ConnectRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    let db = &state.db;

    // 1. Check if a profile already exists for this ordinals address.
    if let Some(profile) = db.get_profile_by_address(&body.ordinals_address).await? {
        let wallets = db.get_profile_wallets(profile.id).await?;
        let token = jwt::create_token(profile.id, &profile.primary_address, &state.jwt_secret)
            .map_err(|e| AppError::Internal(e))?;

        return Ok(Json(AuthResponse {
            token,
            profile: build_profile_response(&profile, &wallets),
        }));
    }

    // 2. No existing profile for this address — check for a JWT to link to an
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
                jwt::create_token(profile.id, &profile.primary_address, &state.jwt_secret)
                    .map_err(|e| AppError::Internal(e))?;

            return Ok(Json(AuthResponse {
                token,
                profile: build_profile_response(&profile, &wallets),
            }));
        }
    }

    // 3. No JWT or invalid JWT — create a brand-new profile.
    let profile = db.create_profile(&body.ordinals_address).await?;

    db.add_wallet_to_profile(
        profile.id,
        &body.payment_address,
        &body.ordinals_address,
        "Wallet 1",
    )
    .await?;

    let wallets = db.get_profile_wallets(profile.id).await?;
    let token = jwt::create_token(profile.id, &profile.primary_address, &state.jwt_secret)
        .map_err(|e| AppError::Internal(e))?;

    Ok(Json(AuthResponse {
        token,
        profile: build_profile_response(&profile, &wallets),
    }))
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
