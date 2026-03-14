use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;

pub mod activity;
pub mod collections;
pub mod inscriptions;
pub mod listings;
pub mod offers;
pub mod sales;

#[derive(Clone)]
pub struct Database {
    pub pool: PgPool,
}

impl Database {
    pub fn new() -> Result<Self> {
        let url = std::env::var("DATABASE_URL").unwrap_or_default();

        let pool = PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(5))
            .connect_lazy(&url)?;
        Ok(Self { pool })
    }

    pub async fn run_migrations(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }
}
