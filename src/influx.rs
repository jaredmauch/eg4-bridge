use crate::prelude::*;
use crate::register::RegisterParser;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::coordinator::PacketStats;

use chrono::TimeZone;
use rinfluxdb::line_protocol::{r#async::Client, LineBuilder};

static MEASUREMENT: &str = "eg4_inverter";

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
            let url = reqwest::Url::parse(config.url())?;
            let credentials = match (config.username(), config.password()) {
                (Some(u), Some(p)) => Some((u, p)),
                _ => None,
            };

            Client::new(url, credentials)?
        };

        // Test the connection by writing a test point
        info!("Testing InfluxDB connection...");
        let test_point = LineBuilder::new("connection_test")
            .insert_tag("test", "true")
            .insert_field("value", 1i64)
            .set_timestamp(chrono::Utc::now())
            .build();

        match client.send(&self.database(), &[test_point]).await {
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

    async fn sender(&self, client: Client) -> Result<()> {
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
                    let mut points = Vec::new();
                    
                    // Extract common fields
                    let serial = data.get("serial")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Missing serial in data"))?;
                    let datalog = data.get("datalog")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Missing datalog in data"))?;
                    let timestamp = data.get("time")
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| anyhow!("Missing time in data"))?;

                    info!("Processing data for serial={}, datalog={}, timestamp={}", serial, datalog, timestamp);

                    // Get raw register data
                    let raw_data = data.get("raw_data")
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
                        register_data.iter()
                            .map(|(k, v)| (k.clone(), u16::from_str_radix(v, 16).unwrap_or(0) as f64))
                            .collect()
                    };

                    info!("Decoded values: {:?}", decoded_values);

                    // Create points for each decoded value
                    for (name, value) in decoded_values {
                        let mut line = LineBuilder::new(MEASUREMENT)
                            .insert_tag("serial", serial)
                            .insert_tag("datalog", datalog)
                            .set_timestamp(chrono::Utc.timestamp_opt(timestamp, 0)
                                .single()
                                .ok_or_else(|| anyhow!("Invalid timestamp: {}", timestamp))?);

                        // Add the field value
                        line = line.insert_field(name.as_str(), value);
                        points.push(line.build());
                        trace!("Preparing InfluxDB point: measurement={}, serial={}, datalog={}, field={}, value={}, timestamp={}", 
                            MEASUREMENT, serial, datalog, name, value, timestamp);
                    }

                    info!("Prepared {} points for InfluxDB", points.len());

                    let mut retry_count = 0;
                    while retry_count < 3 {
                        match client.send(&self.database(), &points).await {
                            Ok(_) => {
                                info!("Successfully sent {} points to InfluxDB for datalog={}, serial={}", 
                                    points.len(), datalog, serial);
                                // Increment stats after successful write
                                if let Ok(mut stats) = self.shared_stats.lock() {
                                    stats.influx_writes += 1;
                                    debug!("Incremented InfluxDB writes counter to {}", stats.influx_writes);
                                }
                                break;
                            }
                            Err(err) => {
                                error!("InfluxDB push failed: {:?} - retrying in 10s (attempt {}/3)", err, retry_count + 1);
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
