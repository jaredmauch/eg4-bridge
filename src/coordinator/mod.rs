pub mod commands;

use crate::prelude::*;
use crate::eg4::packet::{Register, RegisterBit};
use crate::command::Command;
use crate::datalog_writer::DatalogWriter;

use crate::eg4::{
    packet::{DeviceFunction, TranslatedData, Packet},
};

use commands::time_register_ops::{Action, ReadTimeRegister};

use std::sync::{Arc, Mutex};

// Sleep durations - keeping only the ones actively used
const RETRY_DELAY_MS: u64 = 1000;    // 1 second

#[derive(Eq, PartialEq, Debug, Clone)]
pub enum ChannelData {
    Shutdown,
    Packet(crate::eg4::packet::Packet),
    SendPacket(crate::eg4::packet::Packet),
}

pub type InputsStore = std::collections::HashMap<Serial, crate::eg4::packet::ReadInputs>;

#[derive(Debug, Default)]
pub struct PacketStats {
    pub packets_received: u64,
    pub packets_sent: u64,
    // Received packet counters
    pub heartbeat_packets_received: u64,
    pub translated_data_packets_received: u64,
    pub read_param_packets_received: u64,
    pub write_param_packets_received: u64,
    // Sent packet counters
    pub heartbeat_packets_sent: u64,
    pub translated_data_packets_sent: u64,
    pub read_param_packets_sent: u64,
    pub write_param_packets_sent: u64,
    // Error counters
    pub modbus_errors: u64,
    pub mqtt_errors: u64,
    pub influx_errors: u64,
    pub database_errors: u64,
    pub register_cache_errors: u64,
    // Other stats
    pub mqtt_messages_sent: u64,
    pub influx_writes: u64,
    pub database_writes: u64,
    pub register_cache_writes: u64,
    // Connection stats
    pub inverter_disconnections: std::collections::HashMap<Serial, u64>,
    pub serial_mismatches: u64,
    // Last message received per inverter
    pub last_messages: std::collections::HashMap<Serial, String>,
}

impl PacketStats {
    pub fn print_summary(&self) {
        info!("Packet Statistics:");
        info!("  Total packets received: {}", self.packets_received);
        info!("  Total packets sent: {}", self.packets_sent);
        info!("  Received Packet Types:");
        info!("    Heartbeat packets: {}", self.heartbeat_packets_received);
        info!("    TranslatedData packets: {}", self.translated_data_packets_received);
        info!("    ReadParam packets: {}", self.read_param_packets_received);
        info!("    WriteParam packets: {}", self.write_param_packets_received);
        info!("  Sent Packet Types:");
        info!("    Heartbeat packets: {}", self.heartbeat_packets_sent);
        info!("    TranslatedData packets: {}", self.translated_data_packets_sent);
        info!("    ReadParam packets: {}", self.read_param_packets_sent);
        info!("    WriteParam packets: {}", self.write_param_packets_sent);
        info!("  Errors:");
        info!("    Modbus errors: {}", self.modbus_errors);
        info!("    MQTT errors: {}", self.mqtt_errors);
        info!("    InfluxDB errors: {}", self.influx_errors);
        info!("    Database errors: {}", self.database_errors);
        info!("    Register cache errors: {}", self.register_cache_errors);
        info!("  MQTT:");
        info!("    Messages sent: {}", self.mqtt_messages_sent);
        info!("  InfluxDB:");
        info!("    Writes: {}", self.influx_writes);
        info!("  Database:");
        info!("    Writes: {}", self.database_writes);
        info!("  Register Cache:");
        info!("    Writes: {}", self.register_cache_writes);
        info!("  Connection Stats:");
        info!("    Serial number mismatches: {}", self.serial_mismatches);
        info!("    Inverter disconnections by serial:");
        for (serial, count) in &self.inverter_disconnections {
            info!("      {}: {}", serial, count);
            if let Some(last_msg) = self.last_messages.get(serial) {
                info!("      Last message: {}", last_msg);
            }
        }
    }

    pub fn increment_serial_mismatches(&mut self) {
        self.serial_mismatches += 1;
    }

    pub fn increment_mqtt_errors(&mut self) {
        self.mqtt_errors += 1;
    }

    pub fn increment_cache_errors(&mut self) {
        self.register_cache_errors += 1;
    }

    pub fn copy_from(&mut self, other: &PacketStats) {
        self.packets_received = other.packets_received;
        self.packets_sent = other.packets_sent;
        self.heartbeat_packets_received = other.heartbeat_packets_received;
        self.translated_data_packets_received = other.translated_data_packets_received;
        self.read_param_packets_received = other.read_param_packets_received;
        self.write_param_packets_received = other.write_param_packets_received;
        self.heartbeat_packets_sent = other.heartbeat_packets_sent;
        self.translated_data_packets_sent = other.translated_data_packets_sent;
        self.read_param_packets_sent = other.read_param_packets_sent;
        self.write_param_packets_sent = other.write_param_packets_sent;
        self.modbus_errors = other.modbus_errors;
        self.mqtt_errors = other.mqtt_errors;
        self.influx_errors = other.influx_errors;
        self.database_errors = other.database_errors;
        self.register_cache_errors = other.register_cache_errors;
        self.mqtt_messages_sent = other.mqtt_messages_sent;
        self.influx_writes = other.influx_writes;
        self.database_writes = other.database_writes;
        self.register_cache_writes = other.register_cache_writes;
        self.inverter_disconnections = other.inverter_disconnections.clone();
        self.serial_mismatches = other.serial_mismatches;
        self.last_messages = other.last_messages.clone();
    }
}

#[derive(Clone)]
pub struct Coordinator {
    config: Arc<ConfigWrapper>,
    channels: Channels,
    shared_stats: Arc<Mutex<PacketStats>>,
    datalog_writer: Option<Arc<DatalogWriter>>,
    influx: Option<Arc<Influx>>,
    mqtt: Option<Arc<Mqtt>>,
    databases: Vec<Arc<Database>>,
    register_cache: Option<Arc<RegisterCache>>,
}

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
        info!("Stopping all components...");
        
        // Stop coordinator first to prevent new command processing
        self.coordinator.stop();

        // Stop optional components if they exist
        if let Some(influx) = &self.influx {
            influx.stop();
        }
        if let Some(mqtt) = &mut self.mqtt {
            let _ = mqtt.stop().await;
        }
        for database in &self.databases {
            database.stop();
        }
        if let Some(writer) = &self.datalog_writer {
            let _ = writer.stop();
        }

        info!("Shutdown complete");
    }
}

impl Coordinator {
    pub fn new(config: Arc<ConfigWrapper>, channels: Channels) -> Self {
        let shared_stats = Arc::new(Mutex::new(PacketStats::default()));
        Self {
            config,
            channels,
            shared_stats,
            datalog_writer: None,
            influx: None,
            mqtt: None,
            databases: Vec::new(),
            register_cache: None,
        }
    }

    pub fn stop(&self) {
        info!("Stopping coordinator...");

        // Send shutdown signals through channels first
        let _ = self.channels.to_inverter.send(crate::eg4::inverter::ChannelData::Shutdown);
        let _ = self.channels.to_mqtt.send(mqtt::ChannelData::Shutdown);
        let _ = self.channels.to_influx.send(influx::ChannelData::Shutdown);
        let _ = self.channels.to_database.send(database::ChannelData::Shutdown);
        let _ = self.channels.to_register_cache.send(register_cache::ChannelData::Shutdown);
    }

    async fn start(&mut self) -> Result<()> {
        // Initialize all components in dependency order
        info!("Initializing components...");
        
        // Start with RegisterCache as it's a dependency for other components
        info!("  Creating RegisterCache...");
        let register_cache = Arc::new(RegisterCache::new(self.channels.clone()));
        self.register_cache = Some(register_cache.clone());
        
        // Spawn the register cache task
        tokio::spawn(async move {
            if let Err(e) = register_cache.start().await {
                error!("Register cache task failed: {}", e);
            }
        });
        
        // Initialize datalog writer if configured
        if let Some(path) = self.config.datalog_file() {
            info!("Creating datalog writer with path: {}", path);
            let writer = DatalogWriter::new(&path, Arc::new(self.channels.clone()))?;
            let writer_arc = Arc::new(writer);
            self.datalog_writer = Some(writer_arc.clone());
            
            // Spawn the datalog writer task
            tokio::spawn(async move {
                if let Err(e) = writer_arc.start().await {
                    error!("Datalog writer task failed: {}", e);
                }
            });
            info!("Datalog writer initialized successfully");
        }
        
        // Initialize MQTT client if enabled
        if self.config.mqtt().enabled() {
            info!("Initializing MQTT");
            let mqtt = Arc::new(Mqtt::new((*self.config).clone(), self.channels.clone(), self.shared_stats.clone()));
            let mqtt_clone = mqtt.clone();
            self.mqtt = Some(mqtt);
            
            // Spawn the MQTT task
            tokio::spawn(async move {
                if let Err(e) = mqtt_clone.start().await {
                    error!("MQTT task failed: {}", e);
                }
            });
        }
        
        // Initialize InfluxDB client if enabled
        if self.config.influx().enabled() {
            info!("Initializing InfluxDB");
            let influx = Arc::new(Influx::new((*self.config).clone(), self.channels.clone(), self.shared_stats.clone()));
            
            // Start the InfluxDB client and wait for it to initialize
            match influx.start().await {
                Ok(_) => {
                    info!("InfluxDB client started successfully");
                    self.influx = Some(influx.clone());
                }
                Err(e) => {
                    error!("Failed to start InfluxDB client: {}", e);
                    return Err(anyhow!("Failed to start InfluxDB client: {}", e));
                }
            }
        }
        
        // Initialize databases
        self.databases = self.config.databases()
            .iter()
            .filter(|db| db.enabled())
            .map(|db| {
                info!("Initializing database {}", db.url());
                Arc::new(Database::new(db.clone(), self.channels.clone(), self.shared_stats.clone()))
            })
            .collect();
        
        // Start database tasks
        for database in &self.databases {
            let database_clone = database.clone();
            tokio::spawn(async move {
                if let Err(e) = database_clone.start().await {
                    error!("Database task failed: {}", e);
                }
            });
        }
        
        // Verify subscribers are ready
        info!("Verifying subscribers...");
        
        // Check datalog writer subscriber if configured
        if let Some(_) = &self.datalog_writer {
            let receiver = self.channels.from_inverter.subscribe();
            if receiver.is_closed() {
                error!("Datalog writer channel is closed - this is a fatal error");
                bail!("Datalog writer channel is closed");
            }
            info!("Datalog writer subscriber is ready");
        } else {
            info!("Datalog writer not configured, skipping verification");
        }

        // Check InfluxDB subscriber if enabled
        if self.config.influx().enabled() {
            let receiver = self.channels.to_influx.subscribe();
            if receiver.is_closed() {
                error!("InfluxDB channel is closed - this is a fatal error");
                bail!("InfluxDB channel is closed");
            }
            info!("InfluxDB subscriber is ready");
        } else {
            info!("InfluxDB not configured, skipping verification");
        }

        // Check MQTT subscriber if enabled
        if self.config.mqtt().enabled() {
            let receiver = self.channels.to_mqtt.subscribe();
            if receiver.is_closed() {
                error!("MQTT channel is closed - this is a fatal error");
                bail!("MQTT channel is closed");
            }
            info!("MQTT subscriber is ready");
        } else {
            info!("MQTT not configured, skipping verification");
        }

        // Check database subscribers if configured
        if !self.databases.is_empty() {
            let receiver = self.channels.to_database.subscribe();
            if receiver.is_closed() {
                error!("Database channel is closed - this is a fatal error");
                bail!("Database channel is closed");
            }
            info!("Database subscribers are ready");
        } else {
            info!("No databases configured, skipping verification");
        }

        info!("All required subscribers are ready");

        // Create and start inverters
        info!("Creating and starting inverters...");
        let inverters: Vec<_> = self.config
            .enabled_inverters()
            .into_iter()
            .map(|inverter| Inverter::new((*self.config).clone(), &inverter, self.channels.clone()))
            .collect();
        
        // Start each inverter
        for inverter in inverters {
            if let Err(e) = inverter.start().await {
                error!("Failed to start inverter: {}", e);
                continue;
            }
        }
        info!("All inverters started successfully");

        // Start the main loop to process inverter data
        let mut from_inverter_rx = self.channels.from_inverter.subscribe();
        let mut to_coordinator_rx = self.channels.to_coordinator.subscribe();

        info!("Starting main coordinator loop");
        loop {
            tokio::select! {
                // Process data from inverters
                msg = from_inverter_rx.recv() => {
                    match msg {
                        Ok(eg4::inverter::ChannelData::Packet(packet)) => {
                            if let Err(e) = self.process_packet(packet).await {
                                error!("Failed to process packet: {}", e);
                            }
                        }
                        Ok(eg4::inverter::ChannelData::Connected(datalog)) => {
                            if let Err(e) = self.inverter_connected(datalog).await {
                                error!("Failed to handle inverter connection: {}", e);
                            }
                        }
                        Ok(eg4::inverter::ChannelData::Disconnect(datalog)) => {
                            info!("Inverter {} disconnected", datalog);
                        }
                        Ok(eg4::inverter::ChannelData::Shutdown) => {
                            info!("Received shutdown signal from inverter");
                            break;
                        }
                        Ok(eg4::inverter::ChannelData::Heartbeat(packet)) => {
                            debug!("Received heartbeat packet: {:?}", packet);
                        }
                        Ok(eg4::inverter::ChannelData::ModbusError(inverter, code, error)) => {
                            error!("Modbus error from inverter {}: code {}, error: {:?}", 
                                inverter.datalog().map(|s| s.to_string()).unwrap_or_default(),
                                code,
                                error
                            );
                        }
                        Ok(eg4::inverter::ChannelData::SerialMismatch(inverter, expected, actual)) => {
                            error!("Serial mismatch for inverter {}: expected {}, got {}", 
                                inverter.datalog().map(|s| s.to_string()).unwrap_or_default(),
                                expected,
                                actual
                            );
                        }
                        Err(e) => {
                            error!("Error receiving from inverter channel: {}", e);
                            break;
                        }
                    }
                }

                // Process commands from coordinator
                msg = to_coordinator_rx.recv() => {
                    match msg {
                        Ok(ChannelData::SendPacket(packet)) => {
                            if let Err(e) = self.channels.to_inverter.send(eg4::inverter::ChannelData::Packet(packet)) {
                                error!("Failed to send packet to inverter: {}", e);
                            }
                        }
                        Ok(ChannelData::Packet(packet)) => {
                            if let Err(e) = self.process_packet(packet).await {
                                error!("Failed to process packet: {}", e);
                            }
                        }
                        Ok(ChannelData::Shutdown) => {
                            info!("Received shutdown signal");
                            break;
                        }
                        Err(e) => {
                            error!("Error receiving from coordinator channel: {}", e);
                            break;
                        }
                    }
                }
            }
        }

        info!("Coordinator main loop exiting");
        Ok(())
    }

    async fn verify_subscribers(&self) -> Result<()> {
        info!("Verifying subscribers...");

        // Check datalog writer subscriber if configured
        if let Some(writer) = &self.datalog_writer {
            let receiver = self.channels.from_inverter.subscribe();
            if receiver.is_closed() {
                error!("Datalog writer channel is closed - this is a fatal error");
                bail!("Datalog writer channel is closed");
            }
            info!("Datalog writer subscriber is ready");
        } else {
            info!("Datalog writer not configured, skipping verification");
        }

        // Check InfluxDB subscriber if enabled
        if self.config.influx().enabled() {
            let receiver = self.channels.to_influx.subscribe();
            if receiver.is_closed() {
                error!("InfluxDB channel is closed - this is a fatal error");
                bail!("InfluxDB channel is closed");
            }
            info!("InfluxDB subscriber is ready");
        } else {
            info!("InfluxDB not configured, skipping verification");
        }

        // Check MQTT subscriber if enabled
        if self.config.mqtt().enabled() {
            let receiver = self.channels.to_mqtt.subscribe();
            if receiver.is_closed() {
                error!("MQTT channel is closed - this is a fatal error");
                bail!("MQTT channel is closed");
            }
            info!("MQTT subscriber is ready");
        } else {
            info!("MQTT not configured, skipping verification");
        }

        // Check database subscribers if configured
        if !self.databases.is_empty() {
            let receiver = self.channels.to_database.subscribe();
            if receiver.is_closed() {
                error!("Database channel is closed - this is a fatal error");
                bail!("Database channel is closed");
            }
            info!("Database subscribers are ready");
        } else {
            info!("No databases configured, skipping verification");
        }

        info!("All required subscribers are ready");
        Ok(())
    }

    fn start_datalog_writer(&mut self) -> Result<()> {
        if let Some(path) = self.config.datalog_file() {
            info!("Creating datalog writer with path: {}", path);
            let writer = DatalogWriter::new(&path, Arc::new(self.channels.clone()))?;
            self.datalog_writer = Some(Arc::new(writer));
            info!("Datalog writer initialized successfully");
        }
        Ok(())
    }

    async fn process_packet(&self, packet: Packet) -> Result<()> {
        // Update shared stats for received packets
        if let Ok(mut stats) = self.shared_stats.lock() {
            stats.packets_received += 1;
            match &packet {
                Packet::TranslatedData(_) => stats.translated_data_packets_received += 1,
                Packet::ReadParam(_rp) => stats.read_param_packets_received += 1,
                Packet::WriteParam(_wp) => stats.write_param_packets_received += 1,
                Packet::Heartbeat(_) => stats.heartbeat_packets_received += 1,
            }
        }

        let datalog = match &packet {
            Packet::TranslatedData(td) => td.datalog,
            Packet::ReadParam(rp) => rp.datalog,
            Packet::WriteParam(wp) => wp.datalog,
            Packet::Heartbeat(hb) => hb.datalog,
        };

        // Log the type of packet received
        match &packet {
            Packet::TranslatedData(td) => {
                info!("Received TranslatedData packet - datalog: {}, inverter: {}, function: {:?}, register: {}", 
                    td.datalog, td.inverter, td.device_function, td.register);
            }
            Packet::ReadParam(rp) => {
                info!("Received ReadParam packet - datalog: {}, register: {}", 
                    rp.datalog, rp.register);
            }
            Packet::WriteParam(wp) => {
                info!("Received WriteParam packet - datalog: {}, register: {}", 
                    wp.datalog, wp.register);
            }
            Packet::Heartbeat(hb) => {
                info!("Received Heartbeat packet - datalog: {}", 
                    hb.datalog);
            }
        }

        // Check for datalog mismatch and update if needed
        if let Some(inverter) = self.config.enabled_inverter_with_datalog(datalog) {
            // Datalog matches, check serial for TranslatedData packets
            if let Packet::TranslatedData(td) = &packet {
                if let Some(current_serial) = inverter.serial() {
                    if current_serial != td.inverter {
                        info!("Updating inverter serial from {} to {}", current_serial, td.inverter);
                        if let Err(e) = self.config.update_inverter_serial(current_serial, td.inverter) {
                            error!("Failed to update inverter serial: {}", e);
                        }
                    }
                }
            }
        } else {
            // Datalog doesn't match, update configuration
            info!("Updating inverter datalog to {}", datalog);
            if let Err(e) = self.config.update_inverter_datalog(datalog, datalog) {
                error!("Failed to update inverter datalog: {}", e);
            }
        }

        // Skip processing if we're shutting down
        if self.is_shutting_down().await {
            return Ok(());
        }

        // Process the packet
        match packet {
            Packet::TranslatedData(td) => {
                // Skip heartbeat packets for InfluxDB
                if !matches!(td.device_function, DeviceFunction::WriteSingle | DeviceFunction::WriteMulti) {
                    // Send to InfluxDB
                    if let Err(e) = self.send_to_influx(&td).await {
                        error!("Failed to send data to InfluxDB: {}", e);
                    }
                }

                // Cache register values
                if let Err(e) = self.cache_register(td.register, td.values.clone()) {
                    error!("Failed to cache register {}: {}", td.register, e);
                }

                // Send to MQTT
                if let Err(e) = self.send_to_mqtt(&td).await {
                    error!("Failed to send data to MQTT: {}", e);
                }
            }
            Packet::ReadParam(rp) => {
                // Cache register values
                if let Err(e) = self.cache_register(rp.register, rp.values.clone()) {
                    error!("Failed to cache register {}: {}", rp.register, e);
                }
            }
            Packet::WriteParam(wp) => {
                // Check if we're in read-only mode
                if let Some(inverter) = self.config.enabled_inverter_with_datalog(wp.datalog) {
                    if self.config.read_only() || inverter.read_only() {
                        error!("Received write parameter packet but inverter is in read-only mode - datalog: {}, register: {}, value: {:?}", 
                            wp.datalog, wp.register, wp.values);
                        return Ok(());
                    }
                }

                // Cache register values
                if let Err(e) = self.cache_register(wp.register, wp.values.clone()) {
                    error!("Failed to cache register {}: {}", wp.register, e);
                }
            }
            Packet::Heartbeat(_) => {
                // Heartbeat packets are handled in the main loop and don't need to be sent to InfluxDB
            }
        }

        Ok(())
    }

    async fn is_shutting_down(&self) -> bool {
        // Check if any of the channels are closed by trying to subscribe
        self.channels.to_influx.subscribe().is_closed() || 
        self.channels.to_mqtt.subscribe().is_closed() || 
        self.channels.to_database.subscribe().is_closed() ||
        self.channels.read_register_cache.subscribe().is_closed()
    }

    async fn mqtt_receiver(&self) -> Result<()> {
        let mut receiver = self.channels.from_mqtt.subscribe();

        while let mqtt::ChannelData::Message(message) = receiver.recv().await? {
            let _ = self.process_message(message).await;
        }

        Ok(())
    }

    async fn process_message(&self, message: mqtt::Message) -> Result<()> {
        // If MQTT is disabled, don't process any messages
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        for inverter in self.config.inverters_for_message(&message)? {
            match message.to_command(inverter) {
                Ok(command) => {
                    info!("parsed command {:?}", command);
                    let result = self.process_command(command.clone()).await;
                    if result.is_err() {
                    let topic_reply = command.to_result_topic();
                    let reply = mqtt::ChannelData::Message(mqtt::Message {
                        topic: topic_reply,
                        retain: false,
                            payload: "FAIL".to_string(),
                    });
                    if self.channels.to_mqtt.send(reply).is_err() {
                        bail!("send(to_mqtt) failed - channel closed?");
                        }
                    }
                }
                Err(err) => {
                    error!("{:?}", err);
                }
            }
        }

        Ok(())
    }

    /// Process a command received from MQTT or other sources
    /// This function routes commands to appropriate read/write handlers
    async fn process_command(&self, command: Command) -> Result<()> {
        let inverter = match &command {
            Command::ChargeRate(inv, _) |
            Command::DischargeRate(inv, _) |
            Command::AcChargeRate(inv, _) |
            Command::AcChargeSocLimit(inv, _) |
            Command::DischargeCutoffSocLimit(inv, _) |
            Command::SetHold(inv, _, _) |
            Command::WriteParam(inv, _, _) |
            Command::SetAcChargeTime(inv, _, _) |
            Command::SetAcFirstTime(inv, _, _) |
            Command::SetChargePriorityTime(inv, _, _) |
            Command::SetForcedDischargeTime(inv, _, _) |
            Command::ReadInputs(inv, _) |
            Command::ReadInput(inv, _, _) |
            Command::ReadHold(inv, _, _) |
            Command::ReadParam(inv, _) |
            Command::ReadAcChargeTime(inv, _) |
            Command::ReadAcFirstTime(inv, _) |
            Command::ReadChargePriorityTime(inv, _) |
            Command::ReadForcedDischargeTime(inv, _) |
            Command::AcCharge(inv, _) |
            Command::ChargePriority(inv, _) |
            Command::ForcedDischarge(inv, _) => inv.clone(),
        };

        let write_inverter = commands::write_inverter::WriteInverter::new(
            self.channels.clone(),
            inverter.clone(),
            (*self.config).clone(),
        );

        match command {
            // Write operations - these are blocked by read_only mode
            Command::ChargeRate(_, value) => write_inverter.set_charge_rate(value).await,
            Command::DischargeRate(_, value) => write_inverter.set_discharge_rate(value).await,
            Command::AcChargeRate(_, value) => write_inverter.set_ac_charge_rate(value).await,
            Command::AcChargeSocLimit(_, value) => write_inverter.set_ac_charge_soc_limit(value).await,
            Command::DischargeCutoffSocLimit(_, value) => write_inverter.set_discharge_cutoff_soc_limit(value).await,
            Command::SetHold(_, register, value) => write_inverter.set_hold(register, value).await,
            Command::WriteParam(_, register, value) => write_inverter.set_param(register, value).await,
            Command::SetAcChargeTime(_, _, values) => write_inverter.set_ac_charge_time(values).await,
            Command::SetAcFirstTime(_, _, values) => write_inverter.set_ac_first_time(values).await,
            Command::SetChargePriorityTime(_, _, values) => write_inverter.set_charge_priority_time(values).await,
            Command::SetForcedDischargeTime(_, _, values) => write_inverter.set_forced_discharge_time(values).await,
            
            // Read operations - these are always allowed regardless of read_only mode
            Command::ReadInputs(_, block) => self.read_input_block(&inverter, block * 40, inverter.register_block_size()).await,
            Command::ReadInput(_, register, count) => self.read_input_registers(&inverter, register, count).await,
            Command::ReadHold(_, register, count) => self.read_hold_registers(&inverter, register, count).await,
            Command::ReadParam(_, register) => self.read_param_register(&inverter, register).await,
            Command::ReadAcChargeTime(_, num) => self.read_ac_charge_time(&inverter, num).await,
            Command::ReadAcFirstTime(_, num) => self.read_ac_first_time(&inverter, num).await,
            Command::ReadChargePriorityTime(_, num) => self.read_charge_priority_time(&inverter, num).await,
            Command::ReadForcedDischargeTime(_, num) => self.read_forced_discharge_time(&inverter, num).await,
            
            // Enable/Disable operations - these are blocked by read_only mode
            Command::AcCharge(_, enable) => {
                self.update_hold(
                    inverter,
                    Register::Register21,
                    RegisterBit::AcChargeEnable,
                    enable,
                ).await
            },
            Command::ChargePriority(_, enable) => {
                self.update_hold(
                    inverter,
                    Register::Register21,
                    RegisterBit::ChargePriorityEnable,
                    enable,
                ).await
            },
            Command::ForcedDischarge(_, enable) => {
                self.update_hold(
                    inverter,
                    Register::Register21,
                    RegisterBit::ForcedDischargeEnable,
                    enable,
                ).await
            },
        }
    }

    /// Read a block of input registers from the inverter
    /// This operation is always allowed regardless of read_only mode
    async fn read_input_block<U>(
        &self,
        inverter: &config::Inverter,
        register: U,
        count: u16,
    ) -> Result<()>
    where
        U: Into<u16>,
    {
        commands::read_inputs::ReadInputs::new(self.channels.clone(), inverter.clone(), register, count)
        .run()
        .await?;

        // Add delay between reads if configured
        if inverter.delay_ms().unwrap_or(0) > 0 {
            info!("read_input_block sleeping {} ms", inverter.delay_ms().unwrap_or(0));
            tokio::time::sleep(std::time::Duration::from_millis(inverter.delay_ms().unwrap_or(0))).await;
        }
        Ok(())
    }

    /// Read specific input registers from the inverter
    /// This operation is always allowed regardless of read_only mode
    async fn read_input_registers<U>(&self, inverter: &config::Inverter, register: U, count: u16) -> Result<()>
    where
        U: Into<u16>,
    {
        commands::read_inputs::ReadInputs::new(self.channels.clone(), inverter.clone(), register, count)
        .run()
        .await?;

        // Add delay between reads if configured
        if inverter.delay_ms().unwrap_or(0) > 0 {
            info!("read_input_registers sleeping {} ms", inverter.delay_ms().unwrap_or(0));
            tokio::time::sleep(std::time::Duration::from_millis(inverter.delay_ms().unwrap_or(0))).await;
        }
        Ok(())
    }

    /// Read holding registers from the inverter
    /// This operation is always allowed regardless of read_only mode
    async fn read_hold_registers<U>(&self, inverter: &config::Inverter, register: U, count: u16) -> Result<()>
    where
        U: Into<u16>,
    {
        commands::read_hold::ReadHold::new(self.channels.clone(), inverter.clone(), register, count)
        .run()
        .await?;

        // Add delay between reads if configured
        if inverter.delay_ms().unwrap_or(0) > 0 {
            info!("read_hold_registers sleeping {} ms", inverter.delay_ms().unwrap_or(0));
            tokio::time::sleep(std::time::Duration::from_millis(inverter.delay_ms().unwrap_or(0))).await;
        }
        Ok(())
    }

    /// Read a parameter register from the inverter
    /// This operation is always allowed regardless of read_only mode
    async fn read_param_register<U>(&self, inverter: &config::Inverter, register: U) -> Result<()>
    where
        U: Into<u16>,
    {
        commands::read_param::ReadParam::new(self.channels.clone(), inverter.clone(), register)
            .run()
            .await?;

        // Add delay between reads if configured
        if inverter.delay_ms().unwrap_or(0) > 0 {
            info!("read_param_register sleeping {} ms", inverter.delay_ms().unwrap_or(0));
            tokio::time::sleep(std::time::Duration::from_millis(inverter.delay_ms().unwrap_or(0))).await;
        }
        Ok(())
    }

    /// Read AC charge time settings from the inverter
    /// This operation is always allowed regardless of read_only mode
    async fn read_ac_charge_time(
        &self,
        inverter: &config::Inverter,
        num: u16,
    ) -> Result<()> {
        self.read_time_register(inverter, Action::AcCharge(num)).await
    }

    /// Read AC first time settings from the inverter
    /// This operation is always allowed regardless of read_only mode
    async fn read_ac_first_time(
        &self,
        inverter: &config::Inverter,
        num: u16,
    ) -> Result<()> {
        self.read_time_register(inverter, Action::AcFirst(num)).await
    }

    /// Read charge priority time settings from the inverter
    /// This operation is always allowed regardless of read_only mode
    async fn read_charge_priority_time(
        &self,
        inverter: &config::Inverter,
        num: u16,
    ) -> Result<()> {
        self.read_time_register(inverter, Action::ChargePriority(num)).await
    }

    async fn inverter_connected(&mut self, datalog: Serial) -> Result<()> {
        info!("Inverter {} connected", datalog);
        Ok(())
    }

    async fn send_to_influx(&self, data: &TranslatedData) -> Result<()> {
        if self.influx.is_none() {
            debug!("InfluxDB client not initialized, skipping send");
            return Ok(());
        }

        info!("Preparing to send data to InfluxDB: {:?}", data);
        
        // Convert to JSON and add serial number and timestamp
        let mut json = serde_json::to_value(data)?;
        if let Some(obj) = json.as_object_mut() {
            obj.insert("serial".to_string(), serde_json::Value::String(data.inverter.to_string()));
            // Add current timestamp in seconds since epoch
            obj.insert("time".to_string(), serde_json::Value::Number(serde_json::Number::from(chrono::Utc::now().timestamp())));
            
            // Add raw_data field with register values
            let mut raw_data = serde_json::Map::new();
            for (i, value) in data.values.iter().enumerate() {
                let reg = data.register + i as u16;
                raw_data.insert(reg.to_string(), serde_json::Value::String(format!("{:04x}", value)));
            }
            obj.insert("raw_data".to_string(), serde_json::Value::Object(raw_data));
            
            info!("Added serial number and raw data to InfluxDB data: {}", data.inverter);
        }

        // Send to InfluxDB channel
        match self.channels.to_influx.send(crate::influx::ChannelData::InputData(json)) {
            Ok(_) => {
                info!("Successfully sent data to InfluxDB channel");
                Ok(())
            }
            Err(e) => {
                error!("Failed to send data to InfluxDB channel: {}", e);
                Err(anyhow!("Failed to send data to InfluxDB channel: {}", e))
            }
        }
    }

    fn cache_register(&self, register: u16, values: Vec<u8>) -> Result<()> {
        // Convert Vec<u8> to Vec<u16>
        let values_u16: Vec<u16> = values.chunks(2)
            .map(|chunk| {
                if chunk.len() == 2 {
                    ((chunk[0] as u16) << 8) | (chunk[1] as u16)
                } else {
                    chunk[0] as u16
                }
            })
            .collect();

        // Send each value to the register cache
        for (i, value) in values_u16.into_iter().enumerate() {
            let reg = register + i as u16;
            self.channels.to_register_cache.send(register_cache::ChannelData::RegisterData(reg, value))?;
        }
        Ok(())
    }

    async fn send_to_mqtt(&self, data: &TranslatedData) -> Result<()> {
        if let Some(mqtt) = &self.mqtt {
            let messages = mqtt::Message::for_input(data.clone(), true)?;
            for message in messages {
                self.channels.to_mqtt.send(mqtt::ChannelData::Message(message))?;
            }
        }
        Ok(())
    }

    async fn read_forced_discharge_time(&self, inverter: &config::Inverter, num: u16) -> Result<()> {
        self.read_time_register(inverter, Action::ForcedDischarge(num)).await
    }

    async fn update_hold(&self, inverter: config::Inverter, register: Register, bit: RegisterBit, enable: bool) -> Result<()> {
        let write_inverter = commands::write_inverter::WriteInverter::new(
            self.channels.clone(),
            inverter,
            (*self.config).clone(),
        );
        let value = if enable { 1 } else { 0 };
        write_inverter.set_hold(register, value).await
    }

    async fn read_time_register(&self, inverter: &config::Inverter, action: Action) -> Result<()> {
        ReadTimeRegister::new(
            self.channels.clone(),
            inverter.clone(),
            (*self.config).clone(),
            action,
        )
        .run()
        .await
    }

    pub async fn app(shutdown_rx: broadcast::Receiver<()>, config: Arc<ConfigWrapper>) -> Result<()> {
        let channels = Channels::new();
        let mut coordinator = Self::new(config, channels);
        coordinator.start().await
    }
}
