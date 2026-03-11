use crate::db::Database;
use crate::models::activity::{Activity, ActivityType};
use crate::models::listing::ListingStatus;
use crate::services::bitcoin_rpc::BitcoinRpc;
use anyhow::Result;
use uuid::Uuid;

pub struct MempoolWatcher {
    db: Database,
    rpc: BitcoinRpc,
    poll_interval: std::time::Duration,
}

impl MempoolWatcher {
    pub fn new(db: Database) -> Result<Self> {
        let rpc = BitcoinRpc::new()?;
        Ok(Self {
            db,
            rpc,
            poll_interval: std::time::Duration::from_secs(30),
        })
    }

    pub async fn run(self) -> Result<()> {
        let mut interval = tokio::time::interval(self.poll_interval);
        let mut last_known_height: u64 = 0;

        loop {
            interval.tick().await;

            let current_height = match self.rpc.get_block_count() {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!("MempoolWatcher: failed to get block count: {}", e);
                    continue;
                }
            };

            if current_height <= last_known_height {
                continue;
            }

            tracing::info!(
                "MempoolWatcher: new block detected, height={} (was {})",
                current_height,
                last_known_height
            );
            last_known_height = current_height;

            // Query all sales with tx_id set but not yet confirmed.
            let pending_sales = match sqlx::query_as::<_, crate::models::sale::Sale>(
                "SELECT * FROM sales WHERE tx_id IS NOT NULL AND confirmed_at IS NULL",
            )
            .fetch_all(&self.db.pool)
            .await
            {
                Ok(sales) => sales,
                Err(e) => {
                    tracing::warn!("MempoolWatcher: failed to query pending sales: {}", e);
                    continue;
                }
            };

            tracing::debug!(
                "MempoolWatcher: checking {} pending sale(s)",
                pending_sales.len()
            );

            for sale in pending_sales {
                let tx_id = match &sale.tx_id {
                    Some(id) => id.clone(),
                    None => continue,
                };

                // For package-broadcast sales, also check the locking tx if present.
                if let Some(ref locking_tx_id) = sale.locking_tx_id {
                    match self.rpc.get_raw_transaction(locking_tx_id) {
                        Err(e) => {
                            tracing::debug!(
                                "MempoolWatcher: locking tx {} not yet confirmed: {}",
                                locking_tx_id,
                                e
                            );
                            // Don't skip — sale tx may still confirm independently.
                        }
                        Ok(_) => {
                            tracing::debug!(
                                "MempoolWatcher: locking tx {} confirmed",
                                locking_tx_id
                            );
                        }
                    }
                }

                // If get_raw_transaction succeeds, the sale tx is confirmed.
                match self.rpc.get_raw_transaction(&tx_id) {
                    Err(e) => {
                        tracing::debug!(
                            "MempoolWatcher: tx {} not yet confirmed (or not found): {}",
                            tx_id,
                            e
                        );
                        continue;
                    }
                    Ok(_) => {
                        tracing::info!(
                            "MempoolWatcher: tx {} confirmed at height {}",
                            tx_id,
                            current_height
                        );
                    }
                }

                // --- Update sales table ---
                if let Err(e) = sqlx::query(
                    "UPDATE sales SET block_height = $1, confirmed_at = NOW() WHERE id = $2",
                )
                .bind(current_height as i64)
                .bind(sale.id)
                .execute(&self.db.pool)
                .await
                {
                    tracing::error!(
                        "MempoolWatcher: failed to update sale {} confirmation: {}",
                        sale.id,
                        e
                    );
                    continue;
                }

                // --- Update listings table: set status = sold for linked listing ---
                if let Some(listing_id) = sale.listing_id {
                    if let Err(e) = self
                        .db
                        .update_listing_status(listing_id, ListingStatus::Sold)
                        .await
                    {
                        tracing::error!(
                            "MempoolWatcher: failed to update listing {} to sold: {}",
                            listing_id,
                            e
                        );
                        // Non-fatal — continue to update inscription and activity.
                    }
                }

                // --- Update inscriptions table: set owner_address = buyer_address ---
                if let Err(e) = sqlx::query(
                    "UPDATE inscriptions SET owner_address = $1, updated_at = NOW() WHERE inscription_id = $2",
                )
                .bind(&sale.buyer_address)
                .bind(&sale.inscription_id)
                .execute(&self.db.pool)
                .await
                {
                    tracing::error!(
                        "MempoolWatcher: failed to update inscription owner for {}: {}",
                        sale.inscription_id,
                        e
                    );
                }

                // --- Fetch the listing's collection_id (best-effort) ---
                let collection_id: Option<Uuid> = match sqlx::query_scalar::<_, Option<Uuid>>(
                    "SELECT i.collection_id FROM inscriptions i WHERE i.inscription_id = $1",
                )
                .bind(&sale.inscription_id)
                .fetch_optional(&self.db.pool)
                .await
                {
                    Ok(Some(cid)) => cid,
                    _ => None,
                };

                // --- Insert into activity_feed: activity_type = sale ---
                let activity = Activity {
                    id: Uuid::new_v4(),
                    inscription_id: sale.inscription_id.clone(),
                    collection_id,
                    activity_type: ActivityType::Sale,
                    from_address: Some(sale.seller_address.clone()),
                    to_address: Some(sale.buyer_address.clone()),
                    price_sats: Some(sale.price_sats),
                    tx_id: Some(tx_id.clone()),
                    block_height: Some(current_height as i64),
                    created_at: chrono::Utc::now(),
                };

                if let Err(e) = self.db.create_activity(&activity).await {
                    tracing::error!(
                        "MempoolWatcher: failed to insert sale activity for tx {}: {}",
                        tx_id,
                        e
                    );
                }
            }
        }
    }
}
