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

/// All perfect punk variants for grouped query
const PERFECT_PUNK_TRAITS: &[&str] = &[
    "perfect_punk",
    "perfect_punk_7tx",
    "perfect_punk_10tx",
    "perfect_punk_13tx",
    "perfect_punk_17tx",
    "perfect_punk_21tx",
    "perfect_punk_26tx",
    "perfect_punk_43tx",
];

impl Database {
    /// Get bitmap by block height
    pub async fn get_bitmap_by_height(&self, block_height: i64) -> Result<Option<Bitmap>> {
        let bitmap = sqlx::query_as::<_, Bitmap>("SELECT * FROM bitmaps WHERE block_height = $1")
            .bind(block_height)
            .fetch_optional(&self.pool)
            .await?;
        Ok(bitmap)
    }

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
                "SELECT * FROM bitmaps WHERE traits && $1 ORDER BY block_height ASC LIMIT $2 OFFSET $3",
            )
            .bind(PUNK_TRAITS)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else if trait_name == "perfect_punk" {
            // Grouped query: match ANY perfect punk variant
            sqlx::query_as::<_, Bitmap>(
                "SELECT * FROM bitmaps WHERE traits && $1 ORDER BY block_height ASC LIMIT $2 OFFSET $3",
            )
            .bind(PERFECT_PUNK_TRAITS)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else {
            // Standard query: match specific trait
            sqlx::query_as::<_, Bitmap>(
                "SELECT * FROM bitmaps WHERE traits @> ARRAY[$1] ORDER BY block_height ASC LIMIT $2 OFFSET $3",
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
            sqlx::query_scalar("SELECT COUNT(*) FROM bitmaps WHERE traits && $1")
                .bind(PUNK_TRAITS)
                .fetch_one(&self.pool)
                .await?
        } else if trait_name == "perfect_punk" {
            sqlx::query_scalar("SELECT COUNT(*) FROM bitmaps WHERE traits && $1")
                .bind(PERFECT_PUNK_TRAITS)
                .fetch_one(&self.pool)
                .await?
        } else {
            sqlx::query_scalar("SELECT COUNT(*) FROM bitmaps WHERE traits @> ARRAY[$1]")
                .bind(trait_name)
                .fetch_one(&self.pool)
                .await?
        };
        Ok(count)
    }

    /// Get bitmaps by inscription IDs with pagination (excludes encoded_bytes)
    pub async fn get_bitmaps_by_inscription_ids(
        &self,
        ids: &[String],
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Bitmap>> {
        let bitmaps = sqlx::query_as::<_, Bitmap>(
            "SELECT block_height, inscription_id, inscription_num, NULL::bytea as encoded_bytes, tx_count, block_timestamp, traits, created_at, updated_at \
             FROM bitmaps WHERE inscription_id = ANY($1) ORDER BY block_height ASC LIMIT $2 OFFSET $3",
        )
        .bind(ids)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(bitmaps)
    }

    /// Count bitmaps matching given inscription IDs
    pub async fn count_bitmaps_by_inscription_ids(&self, ids: &[String]) -> Result<i64> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM bitmaps WHERE inscription_id = ANY($1)")
                .bind(ids)
                .fetch_one(&self.pool)
                .await?;
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
                "SELECT block_height FROM bitmaps WHERE traits && $1 ORDER BY block_height ASC LIMIT $2 OFFSET $3",
            )
            .bind(PUNK_TRAITS)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else if trait_name == "perfect_punk" {
            sqlx::query_scalar(
                "SELECT block_height FROM bitmaps WHERE traits && $1 ORDER BY block_height ASC LIMIT $2 OFFSET $3",
            )
            .bind(PERFECT_PUNK_TRAITS)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT block_height FROM bitmaps WHERE traits @> ARRAY[$1] ORDER BY block_height ASC LIMIT $2 OFFSET $3",
            )
            .bind(trait_name)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(heights)
    }

    /// Get trait counts for a set of inscription IDs (for portfolio stats)
    pub async fn get_trait_counts_by_inscription_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<(String, i64)>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT trait, COUNT(*) as count FROM (
                SELECT unnest(traits) as trait FROM bitmaps WHERE inscription_id = ANY($1)
            ) t GROUP BY trait ORDER BY count DESC"
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Get bitmaps by inscription IDs filtered by trait
    pub async fn get_bitmaps_by_inscription_ids_and_trait(
        &self,
        ids: &[String],
        trait_name: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Bitmap>> {
        let bitmaps = if trait_name == "punk" {
            sqlx::query_as::<_, Bitmap>(
                "SELECT block_height, inscription_id, inscription_num, NULL::bytea as encoded_bytes, tx_count, block_timestamp, traits, created_at, updated_at \
                 FROM bitmaps WHERE inscription_id = ANY($1) AND traits && $2 ORDER BY block_height ASC LIMIT $3 OFFSET $4",
            )
            .bind(ids)
            .bind(PUNK_TRAITS)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else if trait_name == "perfect_punk" {
            sqlx::query_as::<_, Bitmap>(
                "SELECT block_height, inscription_id, inscription_num, NULL::bytea as encoded_bytes, tx_count, block_timestamp, traits, created_at, updated_at \
                 FROM bitmaps WHERE inscription_id = ANY($1) AND traits && $2 ORDER BY block_height ASC LIMIT $3 OFFSET $4",
            )
            .bind(ids)
            .bind(PERFECT_PUNK_TRAITS)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, Bitmap>(
                "SELECT block_height, inscription_id, inscription_num, NULL::bytea as encoded_bytes, tx_count, block_timestamp, traits, created_at, updated_at \
                 FROM bitmaps WHERE inscription_id = ANY($1) AND traits @> ARRAY[$2] ORDER BY block_height ASC LIMIT $3 OFFSET $4",
            )
            .bind(ids)
            .bind(trait_name)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(bitmaps)
    }

    /// Count bitmaps by inscription IDs filtered by trait
    pub async fn count_bitmaps_by_inscription_ids_and_trait(
        &self,
        ids: &[String],
        trait_name: &str,
    ) -> Result<i64> {
        let count: i64 = if trait_name == "punk" {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM bitmaps WHERE inscription_id = ANY($1) AND traits && $2"
            )
            .bind(ids)
            .bind(PUNK_TRAITS)
            .fetch_one(&self.pool)
            .await?
        } else if trait_name == "perfect_punk" {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM bitmaps WHERE inscription_id = ANY($1) AND traits && $2"
            )
            .bind(ids)
            .bind(PERFECT_PUNK_TRAITS)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM bitmaps WHERE inscription_id = ANY($1) AND traits @> ARRAY[$2]"
            )
            .bind(ids)
            .bind(trait_name)
            .fetch_one(&self.pool)
            .await?
        };
        Ok(count)
    }

    /// Get just block heights for a set of inscription IDs (lightweight for bitfield)
    pub async fn get_block_heights_by_inscription_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<i64>> {
        let heights: Vec<i64> = sqlx::query_scalar(
            "SELECT block_height FROM bitmaps WHERE inscription_id = ANY($1) ORDER BY block_height ASC"
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(heights)
    }

    /// Get the maximum block height across all bitmaps (for total grid extent)
    pub async fn get_max_block_height(&self) -> Result<i64> {
        let max: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(block_height), 0) FROM bitmaps")
            .fetch_one(&self.pool)
            .await?;
        Ok(max)
    }
}
