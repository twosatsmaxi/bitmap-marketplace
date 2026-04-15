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

pub fn create_token(
    profile_id: Uuid,
    primary_address: &str,
    token_version: i32,
    secret: &str,
) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-key-for-unit-tests";

    #[test]
    fn create_and_verify_roundtrip() {
        let id = Uuid::new_v4();
        let token = create_token(id, "bc1qtest", 1, SECRET).unwrap();
        let claims = verify_token(&token, SECRET).unwrap();
        assert_eq!(claims.profile_id, id);
        assert_eq!(claims.primary_address, "bc1qtest");
        assert_eq!(claims.token_version, 1);
    }

    #[test]
    fn token_has_30_day_expiry() {
        let id = Uuid::new_v4();
        let token = create_token(id, "bc1qtest", 1, SECRET).unwrap();
        let claims = verify_token(&token, SECRET).unwrap();

        let now = chrono::Utc::now().timestamp() as usize;
        let thirty_days = 30 * 24 * 60 * 60;
        // Allow 5s tolerance for test execution time
        assert!(claims.exp > now + thirty_days - 5);
        assert!(claims.exp < now + thirty_days + 5);
    }

    #[test]
    fn wrong_secret_rejects() {
        let token = create_token(Uuid::new_v4(), "bc1q", 1, SECRET).unwrap();
        assert!(verify_token(&token, "wrong-secret").is_err());
    }

    #[test]
    fn tampered_payload_rejects() {
        let token = create_token(Uuid::new_v4(), "bc1q", 1, SECRET).unwrap();
        // Flip a character in the payload (second segment)
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let mut payload = parts[1].to_string();
        // Replace last char to corrupt payload
        payload.pop();
        payload.push(if parts[1].ends_with('A') { 'B' } else { 'A' });
        let tampered = format!("{}.{}.{}", parts[0], payload, parts[2]);
        assert!(verify_token(&tampered, SECRET).is_err());
    }

    #[test]
    fn expired_token_rejects() {
        // Manually craft an expired token (well past the default 60s leeway)
        let exp = (chrono::Utc::now().timestamp() - 120) as usize; // 2 minutes ago
        let claims = Claims {
            profile_id: Uuid::new_v4(),
            primary_address: "bc1q".to_string(),
            token_version: 1,
            exp,
        };
        let token = jsonwebtoken::encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap();
        assert!(verify_token(&token, SECRET).is_err());
    }

    #[test]
    fn malformed_jwt_rejects() {
        assert!(verify_token("not-a-jwt", SECRET).is_err());
        assert!(verify_token("", SECRET).is_err());
        assert!(verify_token("a.b.c", SECRET).is_err());
    }

    #[test]
    fn only_hs256_accepted() {
        // Create a token with HS384 — should be rejected by verify_token
        let claims = Claims {
            profile_id: Uuid::new_v4(),
            primary_address: "bc1q".to_string(),
            token_version: 1,
            exp: (chrono::Utc::now().timestamp() + 3600) as usize,
        };
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS384),
            &claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap();
        assert!(verify_token(&token, SECRET).is_err());
    }
}
