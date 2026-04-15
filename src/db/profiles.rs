use super::Database;
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct Profile {
    pub id: Uuid,
    pub primary_address: String,
    pub token_version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ProfileWallet {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub payment_address: String,
    pub ordinals_address: String,
    pub label: String,
    pub linked_at: DateTime<Utc>,
}

impl Database {
    /// Create a new profile
    pub async fn create_profile(&self, primary_address: &str) -> Result<Profile> {
        let profile = sqlx::query_as::<_, Profile>(
            "INSERT INTO profiles (primary_address) VALUES ($1) RETURNING *",
        )
        .bind(primary_address)
        .fetch_one(&self.pool)
        .await?;
        Ok(profile)
    }

    /// Get profile by any linked ordinals address
    pub async fn get_profile_by_address(&self, ordinals_address: &str) -> Result<Option<Profile>> {
        let profile = sqlx::query_as::<_, Profile>(
            "SELECT p.* FROM profiles p \
             JOIN profile_wallets pw ON p.id = pw.profile_id \
             WHERE pw.ordinals_address = $1",
        )
        .bind(ordinals_address)
        .fetch_optional(&self.pool)
        .await?;
        Ok(profile)
    }

    /// Get profile by ID
    pub async fn get_profile_by_id(&self, profile_id: Uuid) -> Result<Option<Profile>> {
        let profile = sqlx::query_as::<_, Profile>("SELECT * FROM profiles WHERE id = $1")
            .bind(profile_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(profile)
    }

    /// Add a wallet to a profile
    pub async fn add_wallet_to_profile(
        &self,
        profile_id: Uuid,
        payment_address: &str,
        ordinals_address: &str,
        label: &str,
    ) -> Result<ProfileWallet> {
        let wallet = sqlx::query_as::<_, ProfileWallet>(
            "INSERT INTO profile_wallets (profile_id, payment_address, ordinals_address, label) \
             VALUES ($1, $2, $3, $4) RETURNING *",
        )
        .bind(profile_id)
        .bind(payment_address)
        .bind(ordinals_address)
        .bind(label)
        .fetch_one(&self.pool)
        .await?;
        Ok(wallet)
    }

    /// Remove a wallet from a profile. Returns false if it's the last wallet (won't delete).
    pub async fn remove_wallet_from_profile(
        &self,
        profile_id: Uuid,
        ordinals_address: &str,
    ) -> Result<bool> {
        let count = self.count_profile_wallets(profile_id).await?;
        if count <= 1 {
            return Ok(false);
        }

        sqlx::query("DELETE FROM profile_wallets WHERE profile_id = $1 AND ordinals_address = $2")
            .bind(profile_id)
            .bind(ordinals_address)
            .execute(&self.pool)
            .await?;

        Ok(true)
    }

    /// Get all wallets for a profile
    pub async fn get_profile_wallets(&self, profile_id: Uuid) -> Result<Vec<ProfileWallet>> {
        let wallets = sqlx::query_as::<_, ProfileWallet>(
            "SELECT * FROM profile_wallets WHERE profile_id = $1 ORDER BY linked_at",
        )
        .bind(profile_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(wallets)
    }

    /// Get all ordinals addresses for a profile
    pub async fn get_all_ordinals_addresses(&self, profile_id: Uuid) -> Result<Vec<String>> {
        let addresses: Vec<String> = sqlx::query_scalar(
            "SELECT ordinals_address FROM profile_wallets WHERE profile_id = $1",
        )
        .bind(profile_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(addresses)
    }

    /// Increment token_version (used for JWT revocation)
    pub async fn increment_token_version(&self, profile_id: Uuid) -> Result<i32> {
        let row = sqlx::query_scalar::<_, i32>(
            "UPDATE profiles SET token_version = token_version + 1 WHERE id = $1 RETURNING token_version"
        )
        .bind(profile_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Count wallets for a profile
    pub async fn count_profile_wallets(&self, profile_id: Uuid) -> Result<i64> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM profile_wallets WHERE profile_id = $1")
                .bind(profile_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    /// Update the label for a wallet belonging to a profile
    /// Returns true if a row was updated, false if wallet not found for this profile
    pub async fn update_wallet_label(
        &self,
        profile_id: Uuid,
        ordinals_address: &str,
        label: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE profile_wallets SET label = $1 WHERE profile_id = $2 AND ordinals_address = $3",
        )
        .bind(label)
        .bind(profile_id)
        .bind(ordinals_address)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }
}
