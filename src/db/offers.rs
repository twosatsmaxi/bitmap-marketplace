use super::Database;
use crate::models::offer::{Offer, OfferStatus};
use anyhow::Result;
use uuid::Uuid;

impl Database {
    pub async fn get_offer(&self, id: Uuid) -> Result<Option<Offer>> {
        let offer = sqlx::query_as::<_, Offer>("SELECT * FROM offers WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(offer)
    }

    pub async fn get_offers_by_inscription(&self, inscription_id: &str) -> Result<Vec<Offer>> {
        let offers = sqlx::query_as::<_, Offer>(
            "SELECT * FROM offers WHERE inscription_id = $1 AND status = 'pending' AND (expires_at IS NULL OR expires_at > NOW()) ORDER BY price_sats DESC"
        )
        .bind(inscription_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(offers)
    }

    pub async fn create_offer(&self, offer: &Offer) -> Result<Offer> {
        let result = sqlx::query_as::<_, Offer>(
            r#"
            INSERT INTO offers (id, inscription_id, buyer_address, price_sats, status, psbt, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#
        )
        .bind(offer.id)
        .bind(&offer.inscription_id)
        .bind(&offer.buyer_address)
        .bind(offer.price_sats)
        .bind(&offer.status)
        .bind(&offer.psbt)
        .bind(offer.expires_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    pub async fn cancel_other_pending_offers(
        &self,
        inscription_id: &str,
        except_offer_id: Uuid,
    ) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE offers SET status = 'cancelled', updated_at = NOW() WHERE inscription_id = $1 AND id != $2 AND status = 'pending'",
        )
        .bind(inscription_id)
        .bind(except_offer_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn expire_stale_offers(&self) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE offers SET status = 'expired', updated_at = NOW() WHERE status = 'pending' AND expires_at IS NOT NULL AND expires_at < NOW()",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn update_offer_status(&self, id: Uuid, status: OfferStatus) -> Result<()> {
        sqlx::query("UPDATE offers SET status = $1, updated_at = NOW() WHERE id = $2")
            .bind(status)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
