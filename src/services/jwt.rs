use anyhow::Result;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub profile_id: Uuid,
    pub primary_address: String,
    pub token_version: i32,
    pub exp: usize,
}

pub fn create_token(profile_id: Uuid, primary_address: &str, token_version: i32, secret: &str) -> Result<String> {
    // 30 day expiry
    let exp = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::days(30))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = Claims {
        profile_id,
        primary_address: primary_address.to_string(),
        token_version,
        exp,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(token)
}

pub fn verify_token(token: &str, secret: &str) -> Result<Claims> {
    // Explicitly specify HS256 algorithm to prevent "none" algorithm attacks
    // and ensure only HMAC-SHA256 tokens are accepted
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    validation.validate_nbf = false;
    
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )?;

    Ok(token_data.claims)
}
