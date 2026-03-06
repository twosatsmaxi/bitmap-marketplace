use crate::models::listing::{Listing, ListingStatus};
use super::Database;
use anyhow::Result;
use uuid::Uuid;

impl Database {
    pub async fn get_listing(&self, id: Uuid) -> Result<Option<Listing>> {
        let listing = sqlx::query_as::<_, Listing>(
            "SELECT * FROM listings WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(listing)
    }

    pub async fn get_active_listing_by_inscription(&self, inscription_id: &str) -> Result<Option<Listing>> {
        let listing = sqlx::query_as::<_, Listing>(
            "SELECT * FROM listings WHERE inscription_id = $1 AND status = 'active' ORDER BY created_at DESC LIMIT 1"
        )
        .bind(inscription_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(listing)
    }

    pub async fn list_active_listings(&self, limit: i64, offset: i64) -> Result<Vec<Listing>> {
        let listings = sqlx::query_as::<_, Listing>(
            "SELECT * FROM listings WHERE status = 'active' ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(listings)
    }

    pub async fn create_listing(&self, listing: &Listing) -> Result<Listing> {
        let result = sqlx::query_as::<_, Listing>(
            r#"
            INSERT INTO listings (id, inscription_id, seller_address, price_sats, status, psbt, royalty_address, royalty_bps)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#
        )
        .bind(listing.id)
        .bind(&listing.inscription_id)
        .bind(&listing.seller_address)
        .bind(listing.price_sats)
        .bind(&listing.status)
        .bind(&listing.psbt)
        .bind(&listing.royalty_address)
        .bind(listing.royalty_bps)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    pub async fn update_listing_status(&self, id: Uuid, status: ListingStatus) -> Result<()> {
        sqlx::query(
            "UPDATE listings SET status = $1, updated_at = NOW() WHERE id = $2"
        )
        .bind(status)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_listing_psbt(&self, id: Uuid, psbt: &str) -> Result<()> {
        sqlx::query(
            "UPDATE listings SET psbt = $1, updated_at = NOW() WHERE id = $2"
        )
        .bind(psbt)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
