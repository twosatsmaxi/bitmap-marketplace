use crate::models::sale::Sale;
use super::Database;
use anyhow::Result;
use uuid::Uuid;

impl Database {
    pub async fn get_sale(&self, id: Uuid) -> Result<Option<Sale>> {
        let sale = sqlx::query_as::<_, Sale>(
            "SELECT * FROM sales WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(sale)
    }

    pub async fn create_sale(&self, sale: &Sale) -> Result<Sale> {
        let result = sqlx::query_as::<_, Sale>(
            r#"
            INSERT INTO sales (id, listing_id, inscription_id, seller_address, buyer_address, price_sats, royalty_sats, tx_id, block_height, confirmed_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING *
            "#
        )
        .bind(sale.id)
        .bind(sale.listing_id)
        .bind(&sale.inscription_id)
        .bind(&sale.seller_address)
        .bind(&sale.buyer_address)
        .bind(sale.price_sats)
        .bind(sale.royalty_sats)
        .bind(&sale.tx_id)
        .bind(sale.block_height)
        .bind(sale.confirmed_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }
}
