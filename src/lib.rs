// Module declarations for the application's core components
pub mod channels;      // Inter-component communication channels
pub mod command;       // Command processing and handling
pub mod config;        // Configuration management
pub mod coordinator;   // Main application coordinator
pub mod database;      // Database operations and storage
pub mod datalog_writer; // Data logging functionality
pub mod home_assistant; // Home Assistant integration
pub mod influx;        // InfluxDB integration
pub mod mqtt;          // MQTT client and messaging
pub mod options;       // Command line options parsing
pub mod prelude;       // Common imports and types
pub mod register_cache; // Register value caching
pub mod scheduler;     // Task scheduling
pub mod unixtime;      // Unix timestamp handling
pub mod utils;         // Utility functions
pub mod eg4;           // EG4 inverter protocol implementation
pub mod error;         // Error handling and types
pub mod register;      // Register definitions and parsing

use crate::prelude::*;
use std::sync::Arc;
use crate::coordinator::Coordinator;
use crate::scheduler::Scheduler;
use crate::mqtt::Mqtt;
use crate::influx::Influx;
use crate::database::Database;
use crate::datalog_writer::DatalogWriter;
use crate::prelude::Channels;

/// Manages all application components and their lifecycle
/// 
/// This struct holds references to all major components of the application
/// and provides methods to coordinate their startup and shutdown.
#[derive(Clone)]
pub struct Components {
    pub coordinator: Coordinator,      // Main application coordinator
    pub scheduler: Scheduler,          // Task scheduler
    pub mqtt: Option<Mqtt>,           // Optional MQTT client
    pub influx: Option<Influx>,       // Optional InfluxDB client
    pub databases: Vec<Database>,     // List of configured databases
    pub datalog_writer: Option<DatalogWriter>, // Optional data logger
    pub channels: Channels,           // Inter-component communication channels
}

impl Components {
    /// Creates a new Components instance with all required components
    pub fn new(
        coordinator: Coordinator,
        scheduler: Scheduler,
        mqtt: Option<Mqtt>,
        influx: Option<Influx>,
        databases: Vec<Database>,
        datalog_writer: Option<DatalogWriter>,
        channels: Channels,
    ) -> Self {
        trace!("Creating new Components instance");
        trace!("  - MQTT enabled: {}", mqtt.is_some());
        trace!("  - InfluxDB enabled: {}", influx.is_some());
        trace!("  - Databases: {}", databases.len());
        trace!("  - Datalog writer enabled: {}", datalog_writer.is_some());
        
        Self {
            coordinator,
            scheduler,
            mqtt,
            influx,
            databases,
            datalog_writer,
            channels,
        }
    }

    /// Gracefully stops all components in the correct order
    /// 
    /// The shutdown sequence is:
    /// 1. Coordinator (to stop processing new commands)
    /// 2. InfluxDB (to stop data collection)
    /// 3. MQTT (to stop message publishing)
    /// 4. Databases (to stop data storage)
    /// 5. Datalog writer (to stop logging)
    pub async fn stop(&mut self) {
        info!("Initiating component shutdown sequence");
        trace!("Stopping components in order: Coordinator -> InfluxDB -> MQTT -> Databases -> DatalogWriter");
        
        // Stop coordinator first to prevent new command processing
        trace!("Stopping Coordinator...");
        self.coordinator.stop();
        trace!("Coordinator stopped");

        // Stop optional components if they exist
        if let Some(influx) = &self.influx {
            trace!("Stopping InfluxDB...");
            influx.stop();
            trace!("InfluxDB stopped");
        }
        if let Some(mqtt) = &mut self.mqtt {
            trace!("Stopping MQTT client...");
            let _ = mqtt.stop().await;
            trace!("MQTT client stopped");
        }
        for (i, database) in self.databases.iter().enumerate() {
            trace!("Stopping database {}/{}...", i + 1, self.databases.len());
            database.stop();
            trace!("Database {}/{} stopped", i + 1, self.databases.len());
        }
        if let Some(writer) = &self.datalog_writer {
            trace!("Stopping datalog writer...");
            let _ = writer.stop();
            trace!("Datalog writer stopped");
        }

        info!("Component shutdown sequence completed");
    }
}

/// Application entry point
/// 
/// This function is the main entry point for the application.
/// It initializes the configuration and starts the main application loop.
pub async fn run(config: Config) -> Result<()> {
    info!("Starting application initialization");
    trace!("Creating shutdown channel");
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel(1);
    let config = Arc::new(ConfigWrapper::from_config(config));
    trace!("Configuration loaded and wrapped in Arc");

    // Set up signal handlers for graceful shutdown
    trace!("Setting up signal handlers");
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!("Failed to listen for ctrl+c: {}", e);
        }
        info!("Ctrl+C signal received, initiating shutdown");
        let _ = shutdown_tx.send(()).await;
    });

    // Run the main application
    info!("Starting main application loop");
    trace!("Calling Coordinator::app with shutdown receiver and config");
    Coordinator::app(shutdown_rx, config).await.map_err(|e| {
        error!("Application error: {}", e);
        anyhow::anyhow!("{}", e)
    })?;

    info!("Application shutdown complete");
    Ok(())
}
