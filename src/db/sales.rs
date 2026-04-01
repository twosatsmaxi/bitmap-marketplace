use super::Database;
use crate::models::sale::Sale;
use anyhow::Result;
use chrono::Utc;
use uuid::Uuid;

impl Database {
    pub async fn get_sale(&self, id: Uuid) -> Result<Option<Sale>> {
        let sale = sqlx::query_as::<_, Sale>("SELECT * FROM sales WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(sale)
    }

    pub async fn get_pending_sales(&self) -> Result<Vec<Sale>> {
        let sales = sqlx::query_as::<_, Sale>(
            "SELECT * FROM sales WHERE tx_id IS NOT NULL AND confirmed_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(sales)
    }

    pub async fn confirm_sale(&self, id: Uuid, block_height: i64) -> Result<()> {
        sqlx::query("UPDATE sales SET block_height = $1, confirmed_at = $2 WHERE id = $3")
            .bind(block_height)
            .bind(Utc::now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn create_sale(&self, sale: &Sale) -> Result<Sale> {
        let result = sqlx::query_as::<_, Sale>(
            r#"
            INSERT INTO sales (id, listing_id, inscription_id, seller_address, buyer_address, price_sats, marketplace_fee_sats, tx_id, locking_tx_id, block_height, confirmed_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING *
            "#
        )
        .bind(sale.id)
        .bind(sale.listing_id)
        .bind(&sale.inscription_id)
        .bind(&sale.seller_address)
        .bind(&sale.buyer_address)
        .bind(sale.price_sats)
        .bind(sale.marketplace_fee_sats)
        .bind(&sale.tx_id)
        .bind(&sale.locking_tx_id)
        .bind(sale.block_height)
        .bind(sale.confirmed_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }
}
