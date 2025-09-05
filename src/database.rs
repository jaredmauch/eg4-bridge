use crate::prelude::*;
use sqlx::{postgres::PgConnectOptions, postgres::PgPoolOptions, PgPool};
use std::sync::RwLock;
use std::sync::Arc;
use std::sync::Mutex;
use crate::coordinator::PacketStats;

#[derive(Debug, Clone, PartialEq)]
pub enum ChannelData {
    ReadInputAll(Box<eg4::packet::ReadInputAll>),
    Shutdown,
}

pub type Sender = broadcast::Sender<ChannelData>;

enum DatabaseType {
    MySQL,
    Postgres,
    SQLite,
}

#[derive(Clone, Debug)]
pub struct Database {
    config: config::Database,
    channels: Channels,
    pool: Arc<RwLock<Option<PgPool>>>,
    shared_stats: Arc<Mutex<PacketStats>>,
}

impl Database {
    pub fn new(config: config::Database, channels: Channels, shared_stats: Arc<Mutex<PacketStats>>) -> Self {
        Self {
            config,
            channels,
            pool: Arc::new(RwLock::new(None)),
            shared_stats,
        }
    }

    pub async fn start(&self) -> Result<()> {
        info!("initializing database");

        // Connect and migrate before starting the inserter
        self.connect().await?;
        self.migrate().await?;
        
        futures::try_join!(self.inserter())?;

        info!("database loop exiting");

        Ok(())
    }

    pub fn stop(&self) {
        let _ = self.channels.to_database.send(ChannelData::Shutdown);
    }

    fn database(&self) -> Result<DatabaseType> {
        let prefix: Vec<&str> = self.config.url().splitn(2, ':').collect();
        match prefix[0] {
            "sqlite" => Ok(DatabaseType::SQLite),
            "mysql" => Ok(DatabaseType::MySQL),
            "postgres" => Ok(DatabaseType::Postgres),
            _ => Err(anyhow!("database.rs:unsupported database {}", self.config.url())),
        }
    }

    async fn connect(&self) -> Result<()> {
        info!("Connecting to database: {}", self.config.url());
        
        // For now, only support PostgreSQL
        match self.database()? {
            DatabaseType::Postgres => {
                let options = PgConnectOptions::from_str(self.config.url())?;
                let pool = PgPoolOptions::new()
                    .max_connections(5)
                    .min_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(30))
                    .connect_with(options)
                    .await?;
                info!("Database connected successfully");
                *self.pool.write().map_err(|_| anyhow::anyhow!("Failed to acquire write lock"))? = Some(pool);
                Ok(())
            }
            _ => Err(anyhow::anyhow!("Only PostgreSQL is currently supported"))
        }
    }

    pub async fn connection(&self) -> Result<PgPool> {
        match &*self.pool.read().map_err(|_| anyhow::anyhow!("Failed to acquire read lock"))? {
            Some(pool) => Ok(pool.clone()),
            None => Err(anyhow::anyhow!("database.rs:Database not connected"))
        }
    }

    async fn migrate(&self) -> Result<()> {
        info!("Starting database migration");
        let pool = self.connection().await?;

        // Use PostgreSQL migrations
        let migrator = sqlx::migrate!("db/migrations/postgres");

        info!("Running database migrations");
        migrator.run(&pool).await?;
        info!("Database migration completed successfully");

        Ok(())
    }

    async fn inserter(&self) -> Result<()> {
        let mut receiver = self.channels.to_database.subscribe();

        // wait for database to be ready
        self.connect().await?;

        let query = format!(
            "INSERT INTO inputs ({}) VALUES {}",
            self.columns(),
            match self.database()? {
                DatabaseType::MySQL => Database::values_for_mysql(),
                _ => Database::values_for_not_mysql(),
            }
        );

        loop {
            use ChannelData::*;

            match receiver.recv().await? {
                Shutdown => break,
                ReadInputAll(data) => {
                    let mut retry_count = 0;
                    let max_retries = 3;
                    let mut backoff = 1;
                    
                    while retry_count < max_retries {
                        match self.insert(&query, &data).await {
                            Ok(_) => {
                                // Increment stats after successful insert
                                if let Ok(mut stats) = self.shared_stats.lock() {
                                    stats.database_writes += 1;
                                    debug!("Incremented database writes counter to {}", stats.database_writes);
                                }
                                break;
                            }
                            Err(err) => {
                                error!("INSERT failed: {:?} - retrying in {}s", err, backoff);
                                if let Ok(mut stats) = self.shared_stats.lock() {
                                    stats.database_errors += 1;
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                                retry_count += 1;
                                backoff *= 2;
                            }
                        }
                    }
                    
                    if retry_count == max_retries {
                        error!("Failed to insert data after {} retries", max_retries);
                    }
                }
            }
        }

        Ok(())
    }

    async fn insert(&self, query: &str, data: &eg4::packet::ReadInputAll) -> Result<()> {
        let pool = self.connection().await?;
        let mut conn = pool.acquire().await?;

        // Convert values that might overflow to i64 for SQLite compatibility
        sqlx::query(query)
            .bind(data.status as i64)
            .bind(data.v_pv_1.unwrap_or(0.0) as i64)
            .bind(data.v_pv_2.unwrap_or(0.0) as i64)
            .bind(data.v_pv_3.unwrap_or(0.0) as i64)
            .bind(data.v_bat.unwrap_or(0.0) as i64)
            .bind(data.soc as i64)
            .bind(data.soh as i64)
            .bind(data.internal_fault as i64)
            .bind(data.p_pv as i64)
            .bind(data.p_pv_1 as i64)
            .bind(data.p_pv_2 as i64)
            .bind(data.p_pv_3 as i64)
            .bind(data.p_battery as f64)
            .bind(data.p_charge as i64)
            .bind(data.p_discharge as i64)
            .bind(data.v_ac_r as i64)
            .bind(data.v_ac_s as i64)
            .bind(data.v_ac_t as i64)
            .bind(data.f_ac as f64)
            .bind(data.p_inv as i64)
            .bind(data.p_rec as i64)
            .bind(data.pf as f64)
            .bind(data.v_eps_r as i64)
            .bind(data.v_eps_s as i64)
            .bind(data.v_eps_t as i64)
            .bind(data.f_eps as f64)
            .bind(data.p_eps as i64)
            .bind(data.s_eps as i64)
            .bind(data.p_grid as f64)
            .bind(data.p_to_grid as i64)
            .bind(data.p_to_user as i64)
            .bind(data.e_pv_day as i64)
            .bind(data.e_pv_day_1 as i64)
            .bind(data.e_pv_day_2 as i64)
            .bind(data.e_pv_day_3 as i64)
            .bind(data.e_inv_day as i64)
            .bind(data.e_rec_day as i64)
            .bind(data.e_chg_day as i64)
            .bind(data.e_dischg_day as i64)
            .bind(data.e_eps_day as i64)
            .bind(data.e_to_grid_day as i64)
            .bind(data.e_to_user_day as i64)
            .bind(data.v_bus_1 as i64)
            .bind(data.v_bus_2 as i64)
            .bind(data.e_pv_all as i64)
            .bind(data.e_pv_all_1 as i64)
            .bind(data.e_pv_all_2 as i64)
            .bind(data.e_pv_all_3 as i64)
            .bind(data.e_inv_all as i64)
            .bind(data.e_rec_all as i64)
            .bind(data.e_chg_all as i64)
            .bind(data.e_dischg_all as i64)
            .bind(data.e_eps_all as i64)
            .bind(data.e_to_grid_all as i64)
            .bind(data.e_to_user_all as i64)
            .bind(data.fault_code as i64)
            .bind(data.warning_code as i64)
            .bind(data.t_inner as f64)
            .bind(data.t_rad_1 as f64)
            .bind(data.t_rad_2 as f64)
            .bind(data.t_bat as f64)
            .bind(data.runtime as i64)
            .bind(data.bms_event_1 as i64)
            .bind(data.bms_event_2 as i64)
            .bind(data.bms_fw_update_state as i64)
            .bind(data.cycle_count as i64)
            .bind(data.vbat_inv as i64)
            .bind(data.datalog.to_string())
            .persistent(true)
            .execute(&mut *conn)
            .await?;

        Ok(())
    }

    fn columns(&self) -> &'static str {
        "status, v_pv_1, v_pv_2, v_pv_3, v_bat, soc, soh, internal_fault, p_pv, p_pv_1, p_pv_2,
        p_pv_3, p_battery, p_charge, p_discharge, v_ac_r, v_ac_s, v_ac_t, f_ac, p_inv, p_rec, pf,
        v_eps_r, v_eps_s, v_eps_t, f_eps, p_eps, s_eps, p_grid, p_to_grid, p_to_user, e_pv_day,
        e_pv_day_1, e_pv_day_2, e_pv_day_3, e_inv_day, e_rec_day, e_chg_day, e_dischg_day,
        e_eps_day, e_to_grid_day, e_to_user_day, v_bus_1, v_bus_2, e_pv_all, e_pv_all_1,
        e_pv_all_2, e_pv_all_3, e_inv_all, e_rec_all, e_chg_all, e_dischg_all, e_eps_all,
        e_to_grid_all, e_to_user_all, fault_code, warning_code, t_inner, t_rad_1, t_rad_2, t_bat,
        runtime, bms_event_1, bms_event_2, bms_fw_update_state, cycle_count, vbat_inv, datalog"
    }

    fn values_for_mysql() -> &'static str {
        r#"(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#
    }

    fn values_for_not_mysql() -> &'static str {
        r#"($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
            $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28,
            $29, $30, $31, $32, $33, $34, $35, $36, $37, $38, $39, $40, $41, $42,
            $43, $44, $45, $46, $47, $48, $49, $50, $51, $52, $53, $54, $55, $56,
            $57, $58, $59, $60, $61, $62, $63, $64, $65, $66, $67, $68, $69, $70,
            $71, $72, $73, $74, $75, $76, $77, $78, $79, $80, $81, $82, $83, $84,
            $85, $86, $87, $88, $89, $90, $91)"#
    }
}
