use super::Database;
use crate::models::listing::{Listing, ListingStatus};
use anyhow::Result;
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingSort {
    CreatedAt,
    PriceAsc,
    PriceDesc,
}

#[derive(Debug, Clone, Default)]
pub struct ListingFilter {
    pub collection_id: Option<Uuid>,
    pub seller_address: Option<String>,
    pub min_price_sats: Option<i64>,
    pub max_price_sats: Option<i64>,
    pub sort_by: Option<ListingSort>,
}

impl Database {
    pub async fn get_listing(&self, id: Uuid) -> Result<Option<Listing>> {
        let listing = sqlx::query_as::<_, Listing>("SELECT * FROM listings WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(listing)
    }

    pub async fn get_active_listing_by_inscription(
        &self,
        inscription_id: &str,
    ) -> Result<Option<Listing>> {
        let listing = sqlx::query_as::<_, Listing>(
            "SELECT * FROM listings WHERE inscription_id = $1 AND status = 'active' ORDER BY created_at DESC LIMIT 1"
        )
        .bind(inscription_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(listing)
    }

    pub async fn list_active_listings(
        &self,
        limit: i64,
        offset: i64,
        filter: &ListingFilter,
    ) -> Result<Vec<Listing>> {
        let mut query = QueryBuilder::<Postgres>::new("SELECT l.* FROM listings l");

        if filter.collection_id.is_some() {
            query.push(" JOIN inscriptions i ON i.inscription_id = l.inscription_id");
        }

        query.push(" WHERE l.status = ");
        query.push_bind(ListingStatus::Active);

        if let Some(collection_id) = filter.collection_id {
            query.push(" AND i.collection_id = ");
            query.push_bind(collection_id);
        }

        if let Some(seller_address) = filter.seller_address.as_deref() {
            query.push(" AND l.seller_address = ");
            query.push_bind(seller_address);
        }

        if let Some(min_price_sats) = filter.min_price_sats {
            query.push(" AND l.price_sats >= ");
            query.push_bind(min_price_sats);
        }

        if let Some(max_price_sats) = filter.max_price_sats {
            query.push(" AND l.price_sats <= ");
            query.push_bind(max_price_sats);
        }

        match filter.sort_by.unwrap_or(ListingSort::CreatedAt) {
            ListingSort::CreatedAt => query.push(" ORDER BY l.created_at DESC"),
            ListingSort::PriceAsc => query.push(" ORDER BY l.price_sats ASC, l.created_at DESC"),
            ListingSort::PriceDesc => query.push(" ORDER BY l.price_sats DESC, l.created_at DESC"),
        };

        query.push(" LIMIT ");
        query.push_bind(limit);
        query.push(" OFFSET ");
        query.push_bind(offset);

        let listings = query
            .build_query_as::<Listing>()
            .fetch_all(&self.pool)
            .await?;
        Ok(listings)
    }

    pub async fn create_listing(&self, listing: &Listing) -> Result<Listing> {
        let result = sqlx::query_as::<_, Listing>(
            r#"
            INSERT INTO listings (id, inscription_id, seller_address, price_sats, status, psbt,
                seller_pubkey, multisig_address, multisig_script, protection_status, source_marketplace)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING *
            "#
        )
        .bind(listing.id)
        .bind(&listing.inscription_id)
        .bind(&listing.seller_address)
        .bind(listing.price_sats)
        .bind(&listing.status)
        .bind(&listing.psbt)
        .bind(&listing.seller_pubkey)
        .bind(&listing.multisig_address)
        .bind(&listing.multisig_script)
        .bind(&listing.protection_status)
        .bind(&listing.source_marketplace)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    pub async fn update_locking_tx(
        &self,
        id: uuid::Uuid,
        locking_raw_tx: &str,
        seller_sale_sig: Option<&str>,
        protection_status: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE listings SET locking_raw_tx = $1, seller_sale_sig = $2, protection_status = $3, updated_at = NOW() WHERE id = $4"
        )
        .bind(locking_raw_tx)
        .bind(seller_sale_sig)
        .bind(protection_status)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn clear_locking_tx(&self, id: uuid::Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE listings SET locking_raw_tx = NULL, multisig_address = NULL, multisig_script = NULL, protection_status = 'none', updated_at = NOW() WHERE id = $1"
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_listing_status(&self, id: Uuid, status: ListingStatus) -> Result<()> {
        sqlx::query("UPDATE listings SET status = $1, updated_at = NOW() WHERE id = $2")
            .bind(status)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_listing_psbt(&self, id: Uuid, psbt: &str) -> Result<()> {
        sqlx::query("UPDATE listings SET psbt = $1, updated_at = NOW() WHERE id = $2")
            .bind(psbt)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
