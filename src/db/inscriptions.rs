use super::Database;
use crate::models::inscription::Inscription;
use anyhow::Result;
use uuid::Uuid;

impl Database {
    pub async fn get_inscription(&self, inscription_id: &str) -> Result<Option<Inscription>> {
        let inscription = sqlx::query_as::<_, Inscription>(
            "SELECT * FROM inscriptions WHERE inscription_id = $1",
        )
        .bind(inscription_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(inscription)
    }

    pub async fn get_inscriptions_by_collection(
        &self,
        collection_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Inscription>> {
        let inscriptions = sqlx::query_as::<_, Inscription>(
            "SELECT * FROM inscriptions WHERE collection_id = $1 ORDER BY inscription_number ASC LIMIT $2 OFFSET $3"
        )
        .bind(collection_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(inscriptions)
    }

    pub async fn get_inscriptions_by_owner(&self, owner_address: &str) -> Result<Vec<Inscription>> {
        let inscriptions = sqlx::query_as::<_, Inscription>(
            "SELECT * FROM inscriptions WHERE owner_address = $1 ORDER BY created_at DESC",
        )
        .bind(owner_address)
        .fetch_all(&self.pool)
        .await?;
        Ok(inscriptions)
    }

    pub async fn update_inscription_owner(
        &self,
        inscription_id: &str,
        new_owner: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE inscriptions SET owner_address = $1, updated_at = NOW() WHERE inscription_id = $2",
        )
        .bind(new_owner)
        .bind(inscription_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_collection_id_for_inscription(
        &self,
        inscription_id: &str,
    ) -> Result<Option<Uuid>> {
        let cid = sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT collection_id FROM inscriptions WHERE inscription_id = $1",
        )
        .bind(inscription_id)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        Ok(cid)
    }

    pub async fn assign_inscriptions_to_collection(
        &self,
        collection_id: Uuid,
        inscription_ids: &[String],
    ) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE inscriptions SET collection_id = $1, updated_at = NOW() WHERE inscription_id = ANY($2)",
        )
        .bind(collection_id)
        .bind(inscription_ids)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn upsert_inscription(&self, inscription: &Inscription) -> Result<Inscription> {
        let result = sqlx::query_as::<_, Inscription>(
            r#"
            INSERT INTO inscriptions (
                id, inscription_id, inscription_number, content_type, content_length,
                owner_address, sat_ordinal, genesis_block_height, genesis_timestamp, collection_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (inscription_id) DO UPDATE SET
                owner_address = EXCLUDED.owner_address,
                collection_id = EXCLUDED.collection_id,
                updated_at = NOW()
            RETURNING *
            "#,
        )
        .bind(inscription.id)
        .bind(&inscription.inscription_id)
        .bind(inscription.inscription_number)
        .bind(&inscription.content_type)
        .bind(inscription.content_length)
        .bind(&inscription.owner_address)
        .bind(inscription.sat_ordinal)
        .bind(inscription.genesis_block_height)
        .bind(inscription.genesis_timestamp)
        .bind(inscription.collection_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }
}
