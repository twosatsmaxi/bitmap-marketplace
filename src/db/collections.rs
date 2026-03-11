use super::Database;
use crate::models::collection::{Collection, CollectionStats};
use anyhow::Result;
use uuid::Uuid;

impl Database {
    pub async fn get_collection_by_id(&self, id: Uuid) -> Result<Option<Collection>> {
        let collection = sqlx::query_as::<_, Collection>("SELECT * FROM collections WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(collection)
    }

    pub async fn get_collection_by_slug(&self, slug: &str) -> Result<Option<Collection>> {
        let collection =
            sqlx::query_as::<_, Collection>("SELECT * FROM collections WHERE slug = $1")
                .bind(slug)
                .fetch_optional(&self.pool)
                .await?;
        Ok(collection)
    }

    pub async fn list_collections(&self) -> Result<Vec<Collection>> {
        let collections =
            sqlx::query_as::<_, Collection>("SELECT * FROM collections ORDER BY created_at DESC")
                .fetch_all(&self.pool)
                .await?;
        Ok(collections)
    }

    pub async fn create_collection(
        &self,
        slug: &str,
        name: &str,
        description: Option<&str>,
        image_url: Option<&str>,
        royalty_address: Option<&str>,
        royalty_bps: Option<i32>,
    ) -> Result<Collection> {
        let collection = sqlx::query_as::<_, Collection>(
            r#"
            INSERT INTO collections (slug, name, description, image_url, royalty_address, royalty_bps)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING *
            "#
        )
        .bind(slug)
        .bind(name)
        .bind(description)
        .bind(image_url)
        .bind(royalty_address)
        .bind(royalty_bps)
        .fetch_one(&self.pool)
        .await?;
        Ok(collection)
    }

    pub async fn get_collection_stats(
        &self,
        collection_id: Uuid,
    ) -> Result<Option<CollectionStats>> {
        let stats = sqlx::query_as::<_, CollectionStats>(
            r#"
            SELECT 
                $1::uuid as collection_id,
                (SELECT MIN(price_sats) FROM listings l JOIN inscriptions i ON l.inscription_id = i.inscription_id WHERE i.collection_id = $1 AND l.status = 'active') as floor_price_sats,
                (SELECT COALESCE(SUM(price_sats), 0) FROM sales s JOIN inscriptions i ON s.inscription_id = i.inscription_id WHERE i.collection_id = $1) as total_volume_sats,
                (SELECT COUNT(*) FROM listings l JOIN inscriptions i ON l.inscription_id = i.inscription_id WHERE i.collection_id = $1 AND l.status = 'active') as listed_count,
                (SELECT COUNT(*) FROM inscriptions WHERE collection_id = $1) as total_supply,
                (SELECT COUNT(DISTINCT owner_address) FROM inscriptions WHERE collection_id = $1) as owners_count
            "#
        )
        .bind(collection_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(stats)
    }
}
