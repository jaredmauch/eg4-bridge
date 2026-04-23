use crate::prelude::*;
use crate::register::RegisterParser;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::coordinator::PacketStats;

use chrono::TimeZone;
use influxdb_line_protocol::LineProtocolBuilder;
use url::Url;

static MEASUREMENT: &str = "eg4_inverter";

/// Minimal InfluxDB 1.x HTTP writer: POST line protocol to `/write` with `precision=s`
/// so timestamps match the JSON `time` field (Unix seconds).
#[derive(Clone)]
struct InfluxWriteClient {
    http: reqwest::Client,
    base_url: Url,
    credentials: Option<(String, String)>,
}

impl InfluxWriteClient {
    fn new(base_url: Url, credentials: Option<(&str, &str)>) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::new(),
            base_url,
            credentials: credentials.map(|(u, p)| (u.to_string(), p.to_string())),
        })
    }

    async fn send_line_protocol(&self, database: &str, body: Vec<u8>) -> Result<()> {
        let mut url = self
            .base_url
            .join("write")
            .map_err(|e| anyhow!("InfluxDB write URL: {}", e))?;
        url.query_pairs_mut()
            .append_pair("db", database)
            .append_pair("precision", "s");

        let mut req = self
            .http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body);

        if let Some((ref u, ref p)) = self.credentials {
            req = req.basic_auth(u, Some(p));
        }

        let resp = req.send().await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        Err(anyhow!("InfluxDB HTTP {}: {}", status, text))
    }
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub enum ChannelData {
    InputData(serde_json::Value),
    HoldData(serde_json::Value),
    Shutdown,
}

#[derive(Clone)]
pub struct Influx {
    config: ConfigWrapper,
    channels: Channels,
    register_parser: Option<RegisterParser>,
    shared_stats: Arc<Mutex<PacketStats>>,
}

impl Influx {
    pub fn new(config: ConfigWrapper, channels: Channels, shared_stats: Arc<Mutex<PacketStats>>) -> Self {
        let register_parser = config.register_file()
            .as_ref()
            .and_then(|file| RegisterParser::new(file).ok());

        Self {
            config,
            channels,
            register_parser,
            shared_stats,
        }
    }

    pub async fn start(&self) -> Result<()> {
        if !self.config.influx().enabled() {
            info!("influx disabled, skipping");
            return Ok(());
        }

        info!("initializing influx at {}", self.config.influx().url());

        let client = {
            let config = self.config.influx();
            let url = Url::parse(config.url())?;
            let credentials = match (config.username().as_ref(), config.password().as_ref()) {
                (Some(u), Some(p)) => Some((u.as_str(), p.as_str())),
                _ => None,
            };

            InfluxWriteClient::new(url, credentials)?
        };

        // Test the connection by writing a test point
        info!("Testing InfluxDB connection...");
        let ts = chrono::Utc::now().timestamp();
        let test_body = LineProtocolBuilder::new()
            .measurement("connection_test")
            .tag("test", "true")
            .field("value", 1_i64)
            .timestamp(ts)
            .close_line()
            .build();

        match client.send_line_protocol(&self.database(), test_body).await {
            Ok(_) => {
                info!("Successfully connected to InfluxDB");
            }
            Err(e) => {
                error!("Failed to connect to InfluxDB: {}", e);
                return Err(anyhow!("Failed to connect to InfluxDB: {}", e));
            }
        }

        // Spawn the sender task instead of awaiting it
        let self_clone = self.clone();
        tokio::spawn(async move {
            if let Err(e) = self_clone.sender(client).await {
                error!("InfluxDB sender task failed: {}", e);
            }
        });

        info!("InfluxDB sender task spawned");

        Ok(())
    }

    pub fn stop(&self) {
        let _ = self.channels.to_influx.send(ChannelData::Shutdown);
    }

    async fn sender(&self, client: InfluxWriteClient) -> Result<()> {
        use ChannelData::*;

        let mut receiver = self.channels.to_influx.subscribe();
        info!("InfluxDB sender started");

        loop {
            match receiver.recv().await {
                Ok(Shutdown) => {
                    info!("InfluxDB sender received shutdown signal");
                    break;
                }
                Ok(InputData(data)) | Ok(HoldData(data)) => {
                    info!("Received data for InfluxDB: {:?}", data);
                    let mut lp = LineProtocolBuilder::new();

                    // Extract common fields
                    let serial = data
                        .get("serial")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Missing serial in data"))?;
                    let datalog = data
                        .get("datalog")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Missing datalog in data"))?;
                    let timestamp = data
                        .get("time")
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| anyhow!("Missing time in data"))?;

                    info!(
                        "Processing data for serial={}, datalog={}, timestamp={}",
                        serial, datalog, timestamp
                    );

                    // Get raw register data
                    let raw_data = data
                        .get("raw_data")
                        .and_then(|v| v.as_object())
                        .ok_or_else(|| anyhow!("Missing raw_data in data"))?;

                    // Convert raw_data to HashMap<String, String>
                    let mut register_data = HashMap::new();
                    for (key, value) in raw_data {
                        if let Some(hex_value) = value.as_str() {
                            register_data.insert(key.clone(), hex_value.to_string());
                        }
                    }

                    info!("Converted raw data to register data: {:?}", register_data);

                    // Decode register values if we have a register parser
                    let decoded_values = if let Some(parser) = &self.register_parser {
                        parser.decode_registers(&register_data, self.config.show_unknown(), datalog)
                    } else {
                        // If no register parser, just use raw values
                        register_data
                            .iter()
                            .map(|(k, v)| (k.clone(), u16::from_str_radix(v, 16).unwrap_or(0) as f64))
                            .collect()
                    };

                    info!("Decoded values: {:?}", decoded_values);

                    if chrono::Utc.timestamp_opt(timestamp, 0).single().is_none() {
                        return Err(anyhow!("Invalid timestamp: {}", timestamp));
                    }

                    let point_count = decoded_values.len();

                    // Create points for each decoded value
                    for (name, value) in decoded_values {
                        // Add the field value
                        lp = lp
                            .measurement(MEASUREMENT)
                            .tag("serial", serial)
                            .tag("datalog", datalog)
                            .field(name.as_str(), value)
                            .timestamp(timestamp)
                            .close_line();
                        trace!(
                            "Preparing InfluxDB point: measurement={}, serial={}, datalog={}, field={}, value={}, timestamp={}",
                            MEASUREMENT, serial, datalog, name, value, timestamp
                        );
                    }

                    let points = lp.build();
                    if points.is_empty() {
                        info!("No InfluxDB points to send");
                        continue;
                    }

                    info!(
                        "Prepared {} points for InfluxDB ({} bytes line protocol)",
                        point_count,
                        points.len()
                    );

                    let mut retry_count = 0;
                    while retry_count < 3 {
                        match client.send_line_protocol(&self.database(), points.clone()).await {
                            Ok(_) => {
                                info!(
                                    "Successfully sent {} points to InfluxDB for datalog={}, serial={}",
                                    point_count, datalog, serial
                                );
                                // Increment stats after successful write
                                if let Ok(mut stats) = self.shared_stats.lock() {
                                    stats.influx_writes += 1;
                                    debug!("Incremented InfluxDB writes counter to {}", stats.influx_writes);
                                }
                                break;
                            }
                            Err(err) => {
                                error!(
                                    "InfluxDB push failed: {:?} - retrying in 10s (attempt {}/3)",
                                    err,
                                    retry_count + 1
                                );
                                if let Ok(mut stats) = self.shared_stats.lock() {
                                    stats.influx_errors += 1;
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                                retry_count += 1;
                            }
                        }
                    }
                    if retry_count == 3 {
                        error!("Failed to send data to InfluxDB after 3 attempts");
                    }
                }
                Err(e) => {
                    if let broadcast::error::RecvError::Closed = e {
                        info!("InfluxDB channel closed, shutting down sender task");
                        break;
                    } else {
                        error!("Error receiving from InfluxDB channel: {}", e);
                    }
                }
            }
        }

        info!("InfluxDB sender loop exiting");
        Ok(())
    }

    fn database(&self) -> String {
        self.config.influx().database().to_string()
    }
}
