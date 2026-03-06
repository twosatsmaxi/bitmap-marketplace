use crate::db::Database;
use crate::models::activity::{Activity, ActivityType};
use crate::models::inscription::Inscription;
use crate::services::bitcoin_rpc::BitcoinRpc;
use crate::services::ord::OrdClient;
use anyhow::Result;
use uuid::Uuid;

pub struct InscriptionIndexer {
    db: Database,
    rpc: BitcoinRpc,
    ord: OrdClient,
    poll_interval: std::time::Duration,
}

impl InscriptionIndexer {
    pub fn new(db: Database) -> Result<Self> {
        let rpc = BitcoinRpc::new()?;
        let ord = OrdClient::new();
        Ok(Self {
            db,
            rpc,
            ord,
            poll_interval: std::time::Duration::from_secs(60),
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
                    tracing::warn!("InscriptionIndexer: failed to get block count: {}", e);
                    continue;
                }
            };

            if current_height <= last_known_height {
                continue;
            }

            tracing::info!(
                "InscriptionIndexer: syncing from height {} to {}",
                last_known_height,
                current_height
            );

            // Process each new block height range. We paginate through ord
            // inscriptions until there are no more, effectively catching up
            // to the current tip.
            if let Err(e) = self.sync_inscriptions(current_height).await {
                tracing::error!("InscriptionIndexer: sync error: {}", e);
                // Don't advance last_known_height so we retry next tick.
                continue;
            }

            last_known_height = current_height;
        }
    }

    async fn sync_inscriptions(&self, current_height: u64) -> Result<()> {
        let mut page: u32 = 0;

        loop {
            let page_data = match self.ord.list_inscriptions(page).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        "InscriptionIndexer: failed to list inscriptions page {}: {}",
                        page,
                        e
                    );
                    break;
                }
            };

            let has_more = page_data.more;

            for inscription_id in page_data.inscriptions {
                if let Err(e) = self.process_inscription(&inscription_id, current_height).await {
                    tracing::warn!(
                        "InscriptionIndexer: failed to process inscription {}: {}",
                        inscription_id,
                        e
                    );
                    // Log and continue with the next inscription.
                }
            }

            if !has_more {
                break;
            }
            page += 1;
        }

        Ok(())
    }

    async fn process_inscription(
        &self,
        inscription_id: &str,
        current_height: u64,
    ) -> Result<()> {
        // Fetch full inscription details from ord.
        let ord_inscription = self.ord.get_inscription(inscription_id).await?;

        let new_owner = match &ord_inscription.address {
            Some(addr) => addr.clone(),
            // Skip unconfirmed inscriptions that have no owner address yet.
            None => {
                tracing::debug!(
                    "InscriptionIndexer: inscription {} has no address (unconfirmed?), skipping",
                    inscription_id
                );
                return Ok(());
            }
        };

        // Check if inscription already exists in DB.
        let existing = self.db.get_inscription(inscription_id).await?;

        match existing {
            Some(existing_inscription) => {
                // Inscription already known — check for owner change (transfer).
                if existing_inscription.owner_address != new_owner {
                    tracing::info!(
                        "InscriptionIndexer: transfer detected for {} ({} -> {})",
                        inscription_id,
                        existing_inscription.owner_address,
                        new_owner
                    );

                    // Build updated inscription and upsert.
                    let updated = Inscription {
                        owner_address: new_owner.clone(),
                        ..existing_inscription.clone()
                    };
                    self.db.upsert_inscription(&updated).await?;

                    // Insert Transfer activity.
                    let activity = Activity {
                        id: Uuid::new_v4(),
                        inscription_id: inscription_id.to_string(),
                        collection_id: existing_inscription.collection_id,
                        activity_type: ActivityType::Transfer,
                        from_address: Some(existing_inscription.owner_address.clone()),
                        to_address: Some(new_owner),
                        price_sats: None,
                        tx_id: None,
                        block_height: Some(current_height as i64),
                        created_at: chrono::Utc::now(),
                    };
                    self.db.create_activity(&activity).await?;
                }
                // If owner hasn't changed, nothing to do.
            }
            None => {
                // New inscription — insert it and record a Mint activity.
                tracing::info!(
                    "InscriptionIndexer: new inscription {} owned by {}",
                    inscription_id,
                    new_owner
                );

                let genesis_timestamp =
                    ord_inscription.genesis_timestamp.map(|ts| {
                        chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
                            .unwrap_or_else(chrono::Utc::now)
                    });

                let inscription = Inscription {
                    id: Uuid::new_v4(),
                    inscription_id: inscription_id.to_string(),
                    inscription_number: ord_inscription.number,
                    content_type: ord_inscription.content_type.clone(),
                    content_length: ord_inscription.content_length.map(|l| l as i64),
                    owner_address: new_owner.clone(),
                    sat_ordinal: ord_inscription.sat.map(|s| s as i64),
                    genesis_block_height: ord_inscription.genesis_height.map(|h| h as i64),
                    genesis_timestamp,
                    collection_id: None,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                };

                self.db.upsert_inscription(&inscription).await?;

                // Insert Mint activity.
                let activity = Activity {
                    id: Uuid::new_v4(),
                    inscription_id: inscription_id.to_string(),
                    collection_id: None,
                    activity_type: ActivityType::Mint,
                    from_address: None,
                    to_address: Some(new_owner),
                    price_sats: None,
                    tx_id: None,
                    block_height: ord_inscription
                        .genesis_height
                        .map(|h| h as i64)
                        .or(Some(current_height as i64)),
                    created_at: chrono::Utc::now(),
                };
                self.db.create_activity(&activity).await?;
            }
        }

        Ok(())
    }
}
