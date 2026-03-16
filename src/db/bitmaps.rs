use super::Database;
use crate::models::bitmap::Bitmap;
use anyhow::Result;

impl Database {
    /// Get bitmaps by trait with pagination
    pub async fn get_bitmaps_by_trait(
        &self,
        trait_name: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Bitmap>> {
        let bitmaps = sqlx::query_as::<_, Bitmap>(
            "SELECT * FROM bitmaps WHERE traits @> ARRAY[$1] ORDER BY block_height ASC LIMIT $2 OFFSET $3"
        )
        .bind(trait_name)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(bitmaps)
    }

    /// Count bitmaps by trait for pagination metadata
    pub async fn count_bitmaps_by_trait(&self, trait_name: &str) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM bitmaps WHERE traits @> ARRAY[$1]"
        )
        .bind(trait_name)
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
        let heights: Vec<i64> = sqlx::query_scalar(
            "SELECT block_height FROM bitmaps WHERE traits @> ARRAY[$1] ORDER BY block_height ASC LIMIT $2 OFFSET $3"
        )
        .bind(trait_name)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(heights)
    }
}
