use super::Database;
use crate::models::activity::Activity;
use anyhow::Result;
use uuid::Uuid;

impl Database {
    pub async fn get_activity_feed(&self, limit: i64, offset: i64) -> Result<Vec<Activity>> {
        let activities = sqlx::query_as::<_, Activity>(
            "SELECT * FROM activity_feed ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(activities)
    }

    pub async fn get_activity_by_collection(
        &self,
        collection_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Activity>> {
        let activities = sqlx::query_as::<_, Activity>(
            "SELECT * FROM activity_feed WHERE collection_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(collection_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(activities)
    }

    pub async fn get_activity_by_inscription(
        &self,
        inscription_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Activity>> {
        let activities = sqlx::query_as::<_, Activity>(
            "SELECT * FROM activity_feed WHERE inscription_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(inscription_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(activities)
    }

    pub async fn create_activity(&self, activity: &Activity) -> Result<Activity> {
        let result = sqlx::query_as::<_, Activity>(
            r#"
            INSERT INTO activity_feed (id, inscription_id, collection_id, activity_type, from_address, to_address, price_sats, tx_id, block_height)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING *
            "#
        )
        .bind(activity.id)
        .bind(&activity.inscription_id)
        .bind(activity.collection_id)
        .bind(&activity.activity_type)
        .bind(&activity.from_address)
        .bind(&activity.to_address)
        .bind(activity.price_sats)
        .bind(&activity.tx_id)
        .bind(activity.block_height)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }
}
