//! Integration test for `Database` insert path. Requires PostgreSQL (same as production).
//!
//! Without a URL, the test exits successfully so `cargo test` stays usable offline.
//! To run against a local or CI database:
//!
//! ```text
//! EG4_TEST_DATABASE_URL=postgres://user:pass@localhost:5432/eg4_test cargo test --test test_database postgres_read_input_all_insertion
//! ```

mod common;
use common::*;

use futures::TryStreamExt;
use eg4_bridge::coordinator::PacketStats;
use eg4_bridge::prelude::*;
use eg4_bridge::{config, database};
use sqlx::Row;
use std::sync::{Arc, Mutex};

fn postgres_test_url() -> Option<String> {
    std::env::var("EG4_TEST_DATABASE_URL")
        .ok()
        .filter(|u| u.starts_with("postgres"))
        .or_else(|| {
            std::env::var("DATABASE_URL")
                .ok()
                .filter(|u| u.starts_with("postgres"))
        })
}

fn assert_str_eq(input: &str, expected: &str) {
    assert_eq!(input, expected);
}
fn assert_u16_eq(input: i32, expected: u16) {
    assert_eq!(input as u16, expected);
}
fn assert_i32_eq(input: i32, expected: i32) {
    assert_eq!(input, expected);
}
fn assert_f64_eq(input: f64, expected: f64) {
    assert_eq!(input, expected);
}

#[tokio::test]
async fn postgres_read_input_all_insertion() {
    let Some(url) = postgres_test_url() else {
        eprintln!(
            "skip postgres_read_input_all_insertion: set EG4_TEST_DATABASE_URL or DATABASE_URL (postgres://...)"
        );
        return;
    };

    common_setup();

    let config = config::Database {
        enabled: true,
        url,
    };
    let channels = Channels::new();
    let shared_stats = Arc::new(Mutex::new(PacketStats::default()));
    let database = Database::new(config, channels.clone(), shared_stats);

    let tf = async {
        let channel_data =
            database::ChannelData::ReadInputAll(Box::new(Factory::read_input_all()));

        let mut retries = 0;
        while channels.to_database.send(channel_data.clone()).is_err() {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            retries += 1;
            if retries > 50 {
                panic!("database not ready for messages");
            }
        }

        database.stop();

        let pool = database.connection().await?;

        let mut retries = 0;
        loop {
            let mut rows = sqlx::query("SELECT * FROM inputs").fetch(&pool);
            if let Some(row) = rows.try_next().await? {
                let ria = Factory::read_input_all();
                assert_u16_eq(row.get("status"), ria.status);
                assert_i32_eq(row.get("p_grid"), ria.p_grid);
                assert_i32_eq(row.get("p_battery"), ria.p_battery);
                assert_u16_eq(row.get("p_discharge"), ria.p_discharge);
                assert_f64_eq(row.get("e_to_user_day"), ria.e_to_user_day);
                assert_f64_eq(row.get("e_pv_all"), ria.e_pv_all);
                assert_u16_eq(row.get("t_rad_2"), ria.t_rad_2);
                assert_u16_eq(row.get("bms_event_1"), ria.bms_event_1);
                assert_u16_eq(row.get("bms_event_2"), ria.bms_event_2);
                assert_u16_eq(row.get("bms_fw_update_state"), ria.bms_fw_update_state);
                assert_u16_eq(row.get("cycle_count"), ria.cycle_count);
                assert_f64_eq(row.get("vbat_inv"), ria.vbat_inv);
                assert_str_eq(row.get("datalog"), "1234567890");
                break;
            }

            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            retries += 1;

            if retries > 50 {
                panic!("row not inserted");
            }
        }

        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(database.start(), tf).unwrap();
}
