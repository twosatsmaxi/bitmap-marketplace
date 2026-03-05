use anyhow::Result;
use sqlx::PgPool;

pub mod collections;
pub mod inscriptions;
pub mod listings;
pub mod offers;
pub mod sales;
pub mod activity;

#[derive(Clone)]
pub struct Database {
    pub pool: PgPool,
}

impl Database {
    pub async fn new() -> Result<Self> {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set");

        let pool = PgPool::connect(&url).await?;
        Ok(Self { pool })
    }

    pub async fn run_migrations(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }
}
