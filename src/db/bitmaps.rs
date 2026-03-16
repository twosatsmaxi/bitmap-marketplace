use super::Database;
use crate::models::bitmap::Bitmap;
use anyhow::Result;

/// All punk-related traits for grouped queries
const PUNK_TRAITS: &[&str] = &[
    "pristine_punk",
    "perfect_punk",
    "perfect_punk_7tx",
    "perfect_punk_10tx",
    "perfect_punk_13tx",
    "perfect_punk_17tx",
    "perfect_punk_21tx",
    "perfect_punk_26tx",
    "perfect_punk_43tx",
    "standard_punk",
    "wide_neck_punk",
];

impl Database {
    /// Get bitmaps by trait with pagination
    pub async fn get_bitmaps_by_trait(
        &self,
        trait_name: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Bitmap>> {
        let bitmaps = if trait_name == "punk" {
            // Grouped query: match ANY punk trait
            sqlx::query_as::<_, Bitmap>(
                "SELECT * FROM bitmaps WHERE traits && $1 ORDER BY block_height ASC LIMIT $2 OFFSET $3"
            )
            .bind(PUNK_TRAITS)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else {
            // Standard query: match specific trait
            sqlx::query_as::<_, Bitmap>(
                "SELECT * FROM bitmaps WHERE traits @> ARRAY[$1] ORDER BY block_height ASC LIMIT $2 OFFSET $3"
            )
            .bind(trait_name)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(bitmaps)
    }

    /// Count bitmaps by trait for pagination metadata
    pub async fn count_bitmaps_by_trait(&self, trait_name: &str) -> Result<i64> {
        let count: i64 = if trait_name == "punk" {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM bitmaps WHERE traits && $1"
            )
            .bind(PUNK_TRAITS)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM bitmaps WHERE traits @> ARRAY[$1]"
            )
            .bind(trait_name)
            .fetch_one(&self.pool)
            .await?
        };
        Ok(count)
    }

    /// Get just block heights by trait (lightweight for explore endpoint)
    pub async fn get_block_heights_by_trait(
        &self,
        trait_name: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<i64>> {
        let heights: Vec<i64> = if trait_name == "punk" {
            sqlx::query_scalar(
                "SELECT block_height FROM bitmaps WHERE traits && $1 ORDER BY block_height ASC LIMIT $2 OFFSET $3"
            )
            .bind(PUNK_TRAITS)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT block_height FROM bitmaps WHERE traits @> ARRAY[$1] ORDER BY block_height ASC LIMIT $2 OFFSET $3"
            )
            .bind(trait_name)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(heights)
    }
}
