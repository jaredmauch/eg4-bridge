use crate::prelude::*;

pub mod commands;

use std::sync::{Arc, Mutex};
use lxp::packet::{DeviceFunction, ReadInput, TranslatedData, Packet, ReadInputAll, ReadInput1, ReadInput2, ReadInput3, ReadInput4, ReadInput5, ReadInput6};
use lxp::inverter;

// Sleep durations - keeping only the ones actively used
const RETRY_DELAY_MS: u64 = 1000;    // 1 second

#[derive(Eq, PartialEq, Debug, Clone)]
pub enum ChannelData {
    Shutdown,
    Packet(lxp::packet::Packet),
}

pub type InputsStore = std::collections::HashMap<Serial, lxp::packet::ReadInputs>;

#[derive(Default)]
pub struct PacketStats {
    packets_received: u64,
    packets_sent: u64,
    // Received packet counters
    heartbeat_packets_received: u64,
    translated_data_packets_received: u64,
    read_param_packets_received: u64,
    write_param_packets_received: u64,
    // Sent packet counters
    heartbeat_packets_sent: u64,
    translated_data_packets_sent: u64,
    read_param_packets_sent: u64,
    write_param_packets_sent: u64,
    // Error counters
    modbus_errors: u64,
    mqtt_errors: u64,
    influx_errors: u64,
    database_errors: u64,
    register_cache_errors: u64,
    // Other stats
    mqtt_messages_sent: u64,
    influx_writes: u64,
    database_writes: u64,
    register_cache_writes: u64,
    // Connection stats
    inverter_disconnections: std::collections::HashMap<Serial, u64>,
    serial_mismatches: u64,
    // Last message received per inverter
    last_messages: std::collections::HashMap<Serial, String>,
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
}

#[derive(Clone)]
pub struct Coordinator {
    config: ConfigWrapper,
    channels: Channels,
    pub stats: Arc<Mutex<PacketStats>>,
}

impl Coordinator {
    pub fn new(config: ConfigWrapper, channels: Channels) -> Self {
        Self { 
            config, 
            channels,
            stats: Arc::new(Mutex::new(PacketStats::default())),
        }
    }

    pub async fn start(&self) -> Result<()> {
        if self.config.mqtt().enabled() {
            futures::try_join!(self.inverter_receiver(), self.mqtt_receiver())?;
        } else {
            self.inverter_receiver().await?;
        }

        Ok(())
    }

    pub fn stop(&self) {
        // Send shutdown signals to channels
        let _ = self
            .channels
            .from_inverter
            .send(lxp::inverter::ChannelData::Shutdown);

        if self.config.mqtt().enabled() {
            let _ = self.channels.from_mqtt.send(mqtt::ChannelData::Shutdown);
        }
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

    fn increment_packets_sent(&self, packet: &Packet) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.packets_sent += 1;
            
            // Increment counter for specific sent packet type
            match packet {
                Packet::Heartbeat(_) => stats.heartbeat_packets_sent += 1,
                Packet::TranslatedData(_) => stats.translated_data_packets_sent += 1,
                Packet::ReadParam(_) => stats.read_param_packets_sent += 1,
                Packet::WriteParam(_) => stats.write_param_packets_sent += 1,
            }
        }
    }

    async fn process_command(&self, command: Command) -> Result<()> {
        use commands::time_register_ops::Action;
        use lxp::packet::{Register, RegisterBit};
        use Command::*;

        match command {
            ReadInputs(inverter, 1) => self.read_inputs(inverter.clone(), 0_u16, inverter.register_block_size()).await?,
            ReadInputs(inverter, 2) => self.read_inputs(inverter.clone(), 40_u16, inverter.register_block_size()).await?,
            ReadInputs(inverter, 3) => self.read_inputs(inverter.clone(), 80_u16, inverter.register_block_size()).await?,
            ReadInputs(inverter, 4) => self.read_inputs(inverter.clone(), 120_u16, inverter.register_block_size()).await?,
            ReadInputs(inverter, 5) => self.read_inputs(inverter.clone(), 160_u16, inverter.register_block_size()).await?,
            ReadInputs(inverter, 6) => self.read_inputs(inverter.clone(), 200_u16, inverter.register_block_size()).await?,
            ReadInputs(_, n) => bail!("Invalid input register block number: {}", n),
            ReadInput(inverter, register, count) => {
                self.read_inputs(inverter.clone(), register, count).await?
            }
            ReadHold(inverter, register, count) => {
                self.read_hold(inverter.clone(), register, count).await?
            }
            ReadParam(inverter, register) => {
                self.read_param(inverter.clone(), register).await?
            }
            ReadAcChargeTime(inverter, num) => {
                self.read_time_register(inverter.clone(), Action::AcCharge(num))
                    .await?
            }
            ReadAcFirstTime(inverter, num) => {
                self.read_time_register(inverter.clone(), Action::AcFirst(num))
                    .await?
            }
            ReadChargePriorityTime(inverter, num) => {
                self.read_time_register(inverter.clone(), Action::ChargePriority(num))
                    .await?
            }
            ReadForcedDischargeTime(inverter, num) => {
                self.read_time_register(inverter.clone(), Action::ForcedDischarge(num))
                    .await?
            }
            SetHold(inverter, register, value) => {
                self.set_hold(inverter.clone(), register, value).await?
            }
            WriteParam(inverter, register, value) => {
                self.write_param(inverter.clone(), register, value).await?
            }
            SetAcChargeTime(inverter, num, values) => {
                self.set_time_register(inverter.clone(), Action::AcCharge(num), values)
                    .await?
            }
            SetAcFirstTime(inverter, num, values) => {
                self.set_time_register(inverter.clone(), Action::AcFirst(num), values)
                    .await?
            }
            SetChargePriorityTime(inverter, num, values) => {
                self.set_time_register(inverter.clone(), Action::ChargePriority(num), values)
                    .await?
            }
            SetForcedDischargeTime(inverter, num, values) => {
                self.set_time_register(inverter.clone(), Action::ForcedDischarge(num), values)
                    .await?
            }
            AcCharge(inverter, enable) => {
                self.update_hold(
                    inverter.clone(),
                    Register::Register21,
                    RegisterBit::AcChargeEnable,
                    enable,
                )
                .await?
            }
            ChargePriority(inverter, enable) => {
                self.update_hold(
                    inverter.clone(),
                    Register::Register21,
                    RegisterBit::ChargePriorityEnable,
                    enable,
                )
                .await?
            }
            ForcedDischarge(inverter, enable) => {
                self.update_hold(
                    inverter.clone(),
                    Register::Register21,
                    RegisterBit::ForcedDischargeEnable,
                    enable,
                )
                .await?
            }
            ChargeRate(inverter, pct) => {
                self.set_hold(inverter.clone(), Register::ChargePowerPercentCmd, pct)
                    .await?
            }
            DischargeRate(inverter, pct) => {
                self.set_hold(inverter.clone(), Register::DischgPowerPercentCmd, pct)
                    .await?
            }
            AcChargeRate(inverter, pct) => {
                self.set_hold(inverter.clone(), Register::AcChargePowerCmd, pct)
                    .await?
            }
            AcChargeSocLimit(inverter, pct) => {
                self.set_hold(inverter.clone(), Register::AcChargeSocLimit, pct)
                    .await?
            }
            DischargeCutoffSocLimit(inverter, pct) => {
                self.set_hold(inverter.clone(), Register::DischgCutOffSocEod, pct)
                    .await?
            }
        }
        Ok(())
    }

    async fn read_inputs<U>(
        &self,
        inverter: config::Inverter,
        register: U,
        count: u16,
    ) -> Result<()>
    where
        U: Into<u16>,
    {
        commands::read_inputs::ReadInputs::new(self.channels.clone(), inverter.clone(), register, count)
            .run()
            .await?;
        
        // Add delay after read operation
        tokio::time::sleep(std::time::Duration::from_millis(inverter.delay_ms())).await;
        Ok(())
    }

    async fn read_hold<U>(&self, inverter: config::Inverter, register: U, count: u16) -> Result<()>
    where
        U: Into<u16>,
    {
        commands::read_hold::ReadHold::new(self.channels.clone(), inverter.clone(), register, count)
            .run()
            .await?;
        
        // Add delay after read operation
        tokio::time::sleep(std::time::Duration::from_millis(inverter.delay_ms())).await;
        Ok(())
    }

    async fn read_param<U>(&self, inverter: config::Inverter, register: U) -> Result<()>
    where
        U: Into<u16>,
    {
        commands::read_param::ReadParam::new(self.channels.clone(), inverter.clone(), register)
            .run()
            .await?;
        
        // Add delay after read operation
        tokio::time::sleep(std::time::Duration::from_millis(inverter.delay_ms())).await;
        Ok(())
    }

    async fn read_time_register(
        &self,
        inverter: config::Inverter,
        action: commands::time_register_ops::Action,
    ) -> Result<()> {
        commands::time_register_ops::ReadTimeRegister::new(
            self.channels.clone(),
            inverter.clone(),
            self.config.clone(),
            action,
        )
        .run()
        .await?;
        
        // Add delay after read operation
        tokio::time::sleep(std::time::Duration::from_millis(inverter.delay_ms())).await;
        Ok(())
    }

    async fn write_param<U>(
        &self,
        inverter: config::Inverter,
        register: U,
        value: u16,
    ) -> Result<()>
    where
        U: Into<u16>,
    {
        commands::write_param::WriteParam::new(
            self.channels.clone(),
            inverter.clone(),
            register,
            value,
        )
        .run()
        .await?;

        Ok(())
    }

    async fn set_time_register(
        &self,
        inverter: config::Inverter,
        action: commands::time_register_ops::Action,
        values: [u8; 4],
    ) -> Result<()> {
        commands::time_register_ops::SetTimeRegister::new(
            self.channels.clone(),
            inverter.clone(),
            self.config.clone(),
            action,
            values,
        )
        .run()
        .await
    }

    async fn set_hold<U>(&self, inverter: config::Inverter, register: U, value: u16) -> Result<()>
    where
        U: Into<u16>,
    {
        commands::set_hold::SetHold::new(self.channels.clone(), inverter.clone(), register, value)
            .run()
            .await?;

        Ok(())
    }

    async fn update_hold<U>(
        &self,
        inverter: config::Inverter,
        register: U,
        bit: lxp::packet::RegisterBit,
        enable: bool,
    ) -> Result<()>
    where
        U: Into<u16>,
    {
        commands::update_hold::UpdateHold::new(
            self.channels.clone(),
            inverter.clone(),
            register,
            bit,
            enable,
        )
        .run()
        .await?;

        Ok(())
    }

    async fn process_inverter_packet(&self, packet: Packet, inverter: &config::Inverter) -> Result<()> {
        if let Packet::TranslatedData(td) = packet {
            // Check for Modbus error response
            if td.values.len() >= 1 {
                let first_byte = td.values[0];
                if first_byte & 0x80 != 0 {  // Check if MSB is set (error response)
                    let error_code = first_byte & 0x7F;  // Remove MSB to get error code
                    if let Some(error) = lxp::packet::ModbusError::from_code(error_code) {
                        error!("Modbus error from inverter {}: {} (code: {:#04x})", 
                            inverter.datalog(), error.description(), error_code);
                        if let Ok(mut stats) = self.stats.lock() {
                            stats.modbus_errors += 1;
                        }
                        return Ok(());  // Return early as this is an error response
                    }
                }
            }

            // Validate serial number format
            if let Some(serial) = td.inverter() {
                if !serial.to_string().chars().all(|c| c.is_ascii_alphanumeric()) {
                    warn!("Invalid serial number format: {}", serial);
                    if let Ok(mut stats) = self.stats.lock() {
                        stats.serial_mismatches += 1;
                    }
                    return Ok(());
                }
            }

            // Log TCP function for debugging
            debug!("Processing TCP function: {:?}", td.tcp_function());

            // Check if serial matches configured inverter
            if td.inverter() != Some(inverter.serial) {
                warn!(
                    "Serial mismatch - got {:?}, expected {}",
                    td.inverter(),
                    inverter.serial
                );
                if let Ok(mut stats) = self.stats.lock() {
                    stats.serial_mismatches += 1;
                    
                    // Track disconnection for this serial
                    let count = stats.inverter_disconnections
                        .entry(inverter.serial)
                        .or_insert(0);
                    *count += 1;
                    
                    // Store last message for debugging
                    stats.last_messages.insert(
                        inverter.serial,
                        format!("Serial mismatch - got {:?}, expected {}", td.inverter(), inverter.serial)
                    );
                }

                // Try to recover by requesting a new connection
                if let Err(e) = self.channels.from_inverter.send(inverter::ChannelData::Disconnect(inverter.serial)) {
                    error!("Failed to request disconnect after serial mismatch: {}", e);
                }
                
                return Ok(());
            }

            match td.device_function {
                DeviceFunction::ReadInput => {
                    debug!("Processing ReadInput packet");
                    let read_input = td.read_input()?;
                    match read_input {
                        ReadInput::ReadInputAll(input_all) => {
                            debug!("Processing ReadInputAll");
                            if let Err(e) = self.publish_raw_input_messages(&input_all, inverter).await {
                                error!("Failed to publish raw input messages: {}", e);
                                if let Ok(mut stats) = self.stats.lock() {
                                    stats.mqtt_errors += 1;
                                }
                            }
                        }
                        ReadInput::ReadInput1(input_1) => {
                            debug!("Processing ReadInput1");
                            if let Err(e) = self.publish_raw_input_messages_1(&input_1, inverter).await {
                                error!("Failed to publish raw input messages: {}", e);
                                if let Ok(mut stats) = self.stats.lock() {
                                    stats.mqtt_errors += 1;
                                }
                            }
                        }
                        ReadInput::ReadInput2(input_2) => {
                            debug!("Processing ReadInput2");
                            if let Err(e) = self.publish_raw_input_messages_2(&input_2, inverter).await {
                                error!("Failed to publish raw input messages: {}", e);
                                if let Ok(mut stats) = self.stats.lock() {
                                    stats.mqtt_errors += 1;
                                }
                            }
                        }
                        ReadInput::ReadInput3(input_3) => {
                            debug!("Processing ReadInput3");
                            if let Err(e) = self.publish_raw_input_messages_3(&input_3, inverter).await {
                                error!("Failed to publish raw input messages: {}", e);
                                if let Ok(mut stats) = self.stats.lock() {
                                    stats.mqtt_errors += 1;
                                }
                            }
                        }
                        ReadInput::ReadInput4(input_4) => {
                            debug!("Processing ReadInput4");
                            if let Err(e) = self.publish_raw_input_messages_4(&input_4, inverter).await {
                                error!("Failed to publish raw input messages: {}", e);
                                if let Ok(mut stats) = self.stats.lock() {
                                    stats.mqtt_errors += 1;
                                }
                            }
                        }
                        ReadInput::ReadInput5(input_5) => {
                            debug!("Processing ReadInput5");
                            if let Err(e) = self.publish_raw_input_messages_5(&input_5, inverter).await {
                                error!("Failed to publish raw input messages: {}", e);
                                if let Ok(mut stats) = self.stats.lock() {
                                    stats.mqtt_errors += 1;
                                }
                            }
                        }
                        ReadInput::ReadInput6(input_6) => {
                            debug!("Processing ReadInput6");
                            if let Err(e) = self.publish_raw_input_messages_6(&input_6, inverter).await {
                                error!("Failed to publish raw input messages: {}", e);
                                if let Ok(mut stats) = self.stats.lock() {
                                    stats.mqtt_errors += 1;
                                }
                            }
                        }
                    }
                }
                DeviceFunction::ReadHold => {
                    debug!("Processing ReadHold packet");
                    let register = td.register();
                    let pairs = td.pairs();
                    
                    // Log all register values
                    info!("Hold Register Values:");
                    for (reg, value) in &pairs {
                        // Cache the register value
                        if let Err(e) = self.channels.to_register_cache.send(register_cache::ChannelData::RegisterData(*reg, *value)) {
                            error!("Failed to cache register {}: {}", reg, e);
                            if let Ok(mut stats) = self.stats.lock() {
                                stats.register_cache_errors += 1;
                            }
                        }
                        
                        // Log register value with description if known
                        match *reg {
                            // System Information
                            0 => {
                                let lithium_type = (value >> 12) & 0xF;
                                let power_rating = (value >> 8) & 0xF;
                                let lead_acid_type = (value >> 4) & 0xF;
                                let battery_type = value & 0xF;
                                info!("  Register {:3} (Model Info): {:#06x}", reg, value);
                                info!("    Lithium Type: {}", lithium_type);
                                info!("    Power Rating: {}", power_rating);
                                info!("    Lead Acid Type: {}", lead_acid_type);
                                info!("    Battery Type: {}", battery_type);
                            },
                            2..=6 => info!("  Register {:3} (Serial Number Part {}): {:#06x}", reg, reg - 1, value),
                            7 => info!("  Register {:3} (Firmware Code): {}", reg, value),
                            9 => info!("  Register {:3} (Slave Firmware Code): {}", reg, value),
                            10 => info!("  Register {:3} (Control CPU Firmware Code): {}", reg, value),
                            11 => {
                                info!("  Register {:3} (Status Flags): {:#018b} - Binary format showing all status bits", reg, value);
                                info!("    Status Flag Details:");
                                if value & (1 << 0) != 0 { info!("    Bit  0: Grid Connected"); }
                                if value & (1 << 1) != 0 { info!("    Bit  1: Grid Synchronization"); }
                                if value & (1 << 2) != 0 { info!("    Bit  2: Soft Start"); }
                                if value & (1 << 3) != 0 { info!("    Bit  3: PV Input"); }
                                if value & (1 << 4) != 0 { info!("    Bit  4: Battery Charging"); }
                                if value & (1 << 5) != 0 { info!("    Bit  5: Battery Discharging"); }
                                if value & (1 << 6) != 0 { info!("    Bit  6: EPS Mode"); }
                                if value & (1 << 7) != 0 { info!("    Bit  7: Fault Present"); }
                                if value & (1 << 8) != 0 { info!("    Bit  8: Charging from Grid"); }
                                if value & (1 << 9) != 0 { info!("    Bit  9: Charge Priority Active"); }
                                if value & (1 << 10) != 0 { info!("    Bit 10: Forced Discharge Active"); }
                                if value & (1 << 11) != 0 { info!("    Bit 11: AC Charge Active"); }
                                if value & (1 << 12) != 0 { info!("    Bit 12: Fault Lock"); }
                                if value & (1 << 13) != 0 { info!("    Bit 13: Battery Full"); }
                                if value & (1 << 14) != 0 { info!("    Bit 14: Battery Empty"); }
                                if value & (1 << 15) != 0 { info!("    Bit 15: Battery Standby"); }
                            },
                            12 => {
                                let month = value >> 8;
                                let year = value & 0xFF;
                                info!("  Register {:3} (Date): Month={}, Year=20{:02}", reg, month, year);
                            },
                            13 => {
                                let hour = value >> 8;
                                let day = value & 0xFF;
                                info!("  Register {:3} (Time): Hour={}, Day={}", reg, hour, day);
                            },
                            14 => {
                                let second = value >> 8;
                                let minute = value & 0xFF;
                                info!("  Register {:3} (Time): Second={}, Minute={}", reg, second, minute);
                            },
                            15 => info!("  Register {:3} (Communication Address): {}", reg, value),
                            16 => info!("  Register {:3} (Language): {}", reg, value),
                            19 => info!("  Register {:3} (Device Type): {}", reg, value),
                            20 => info!("  Register {:3} (PV Input Mode): {}", reg, value),
                            23 => info!("  Register {:3} (Connect Time): {} ms - Grid connection time", reg, value),
                            24 => info!("  Register {:3} (Reconnect Time): {} ms - Grid reconnection delay", reg, value),

                            // Table 8 - System Control
                            21 => {
                                let mut flags = Vec::new();
                                if value & (1 << 0) != 0 { flags.push("AC Charge Enabled"); }
                                if value & (1 << 1) != 0 { flags.push("Charge Priority Enabled"); }
                                if value & (1 << 2) != 0 { flags.push("Forced Discharge Enabled"); }
                                if value & (1 << 3) != 0 { flags.push("AC First Enabled"); }
                                info!("  Register {:3} (System Flags): {:#06x}", reg, value);
                                let flag_str = if flags.is_empty() { 
                                    "None".to_string() 
                                } else { 
                                    flags.join(", ") 
                                };
                                info!("    Active flags: {}", flag_str);
                            },
                            22 => info!("  Register {:3} (Start PV Voltage): {:.1} V - PV voltage threshold for inverter startup", reg, (*value as f64) / 10.0),
                            64 => info!("  Register {:3} (System Charge Rate): {}% - Overall system charging power limit", reg, value),
                            65 => info!("  Register {:3} (System Discharge Rate): {}% - Overall system discharging power limit", reg, value),
                            66 => info!("  Register {:3} (Grid Charge Power Rate): {}% - Maximum power allowed for grid charging", reg, value),
                            67 => info!("  Register {:3} (AC Charge SOC Limit): {}% - Battery level at which AC charging stops", reg, value),
                            74 => info!("  Register {:3} (Charge Priority Rate): {}% - Power limit during charge priority mode", reg, value),
                            75 => info!("  Register {:3} (Charge Priority SOC): {}% - Target SOC for charge priority mode", reg, value),
                            83 => info!("  Register {:3} (Forced Discharge SOC): {}% - Target SOC for forced discharge mode", reg, value),
                            105 => info!("  Register {:3} (Discharge cut-off SOC): {}% - Minimum battery level for normal operation", reg, value),
                            125 => info!("  Register {:3} (EPS Discharge cut-off): {}% - Minimum battery level in EPS mode", reg, value),
                            160 => info!("  Register {:3} (AC Charge Start SOC): {}% - Battery level to begin AC charging", reg, value),
                            161 => info!("  Register {:3} (AC Charge End SOC): {}% - Battery level to stop AC charging", reg, value),
                            
                            // Table 7 - System Status
                            110 => info!("  Register {:3} (Battery Capacity): {} Ah - Rated capacity of the battery", reg, value),
                            111 => info!("  Register {:3} (Battery Voltage): {:.1} V - Current battery voltage", reg, (*value as f64) / 10.0),
                            112 => info!("  Register {:3} (Battery Current): {:.1} A - Current battery current", reg, (*value as f64) / 10.0),
                            113 => info!("  Register {:3} (Battery Power): {} W - Current battery power", reg, value),
                            114 => info!("  Register {:3} (Battery Temperature): {:.1} °C - Current battery temperature", reg, (*value as f64) / 10.0),
                            115 => info!("  Register {:3} (Battery SOC): {}% - Current state of charge", reg, value),
                            116 => info!("  Register {:3} (Battery SOH): {}% - Current state of health", reg, value),
                            117 => info!("  Register {:3} (Battery Cycles): {} - Total battery charge/discharge cycles", reg, value),
                            68 => info!("  Register {:3} (Grid Power Limit): {} W - Maximum grid power limit", reg, value),
                            69 => info!("  Register {:3} (Grid Connected Power): {} W - Current grid connected power", reg, value),
                            70 => info!("  Register {:3} (Grid Frequency): {:.2} Hz - Current grid frequency", reg, (*value as f64) / 100.0),
                            71 => info!("  Register {:3} (Grid Voltage): {:.1} V - Current grid voltage", reg, (*value as f64) / 10.0),
                            72 => info!("  Register {:3} (Grid Current): {:.1} A - Current grid current", reg, (*value as f64) / 10.0),
                            73 => info!("  Register {:3} (Grid Power): {} W - Current grid power", reg, value),
                            76 => info!("  Register {:3} (Load Power): {} W - Current load power consumption", reg, value),
                            77 => info!("  Register {:3} (Load Current): {:.1} A - Current load current", reg, (*value as f64) / 10.0),
                            78 => info!("  Register {:3} (AC Power L1): {} W - AC power on phase L1", reg, value),
                            79 => info!("  Register {:3} (AC Power L2): {} W - AC power on phase L2", reg, value),
                            80 => info!("  Register {:3} (Daily Grid Import): {:.1} kWh - Energy imported from grid today", reg, (*value as f64) / 10.0),
                            81 => info!("  Register {:3} (Daily Grid Export): {:.1} kWh - Energy exported to grid today", reg, (*value as f64) / 10.0),
                            82 => info!("  Register {:3} (Daily Load Energy): {:.1} kWh - Energy consumed by load today", reg, (*value as f64) / 10.0),
                            84 => {
                                let mut flags = Vec::new();
                                if value & (1 << 0) != 0 { flags.push("AC Charge Time Enabled"); }
                                if value & (1 << 1) != 0 { flags.push("Forced Discharge Time Enabled"); }
                                if value & (1 << 2) != 0 { flags.push("Charge Priority Time Enabled"); }
                                info!("  Register {:3} (Time Enable Flags): {:#06x}", reg, value);
                                let flag_str = if flags.is_empty() { 
                                    "None".to_string() 
                                } else { 
                                    flags.join(", ") 
                                };
                                info!("    Active time flags: {}", flag_str);
                            },
                            85 => info!("  Register {:3} (Current Time): Hour={}, Minute={} - System time", reg, value >> 8, value & 0xFF),
                            200 => {
                                let status = match value {
                                    0 => "Standby",
                                    1 => "Self Test",
                                    2 => "Normal",
                                    3 => "Alarm",
                                    4 => "Fault",
                                    _ => "Unknown",
                                };
                                info!("  Register {:3} (System Status): {} ({})", reg, value, status)
                            },
                            // Grid Connection Limits
                            25 => info!("  Register {:3} (Grid Connect Low Volt): {:.1} V - Lower voltage limit for grid connection", reg, (*value as f64) / 10.0),
                            26 => info!("  Register {:3} (Grid Connect High Volt): {:.1} V - Upper voltage limit for grid connection", reg, (*value as f64) / 10.0),
                            27 => info!("  Register {:3} (Grid Connect Low Freq): {:.2} Hz - Lower frequency limit for grid connection", reg, (*value as f64) / 100.0),
                            28 => info!("  Register {:3} (Grid Connect High Freq): {:.2} Hz - Upper frequency limit for grid connection", reg, (*value as f64) / 100.0),
                            
                            // Grid Voltage Protection Level 1
                            29 => info!("  Register {:3} (Grid Volt Limit 1 Low): {:.1} V - Level 1 lower voltage threshold", reg, (*value as f64) / 10.0),
                            30 => info!("  Register {:3} (Grid Volt Limit 1 High): {:.1} V - Level 1 upper voltage threshold", reg, (*value as f64) / 10.0),
                            31 => info!("  Register {:3} (Grid Volt Limit 1 Low Time): {} ms - Level 1 low voltage delay", reg, value),
                            32 => info!("  Register {:3} (Grid Volt Limit 1 High Time): {} ms - Level 1 high voltage delay", reg, value),
                            
                            // Grid Voltage Protection Level 2
                            33 => info!("  Register {:3} (Grid Volt Limit 2 Low): {:.1} V - Level 2 lower voltage threshold", reg, (*value as f64) / 10.0),
                            34 => info!("  Register {:3} (Grid Volt Limit 2 High): {:.1} V - Level 2 upper voltage threshold", reg, (*value as f64) / 10.0),
                            35 => info!("  Register {:3} (Grid Volt Limit 2 Low Time): {} ms - Level 2 low voltage delay", reg, value),
                            36 => info!("  Register {:3} (Grid Volt Limit 2 High Time): {} ms - Level 2 high voltage delay", reg, value),
                            
                            // Grid Voltage Protection Level 3
                            37 => info!("  Register {:3} (Grid Volt Limit 3 Low): {:.1} V - Level 3 lower voltage threshold", reg, (*value as f64) / 10.0),
                            38 => info!("  Register {:3} (Grid Volt Limit 3 High): {:.1} V - Level 3 upper voltage threshold", reg, (*value as f64) / 10.0),
                            39 => info!("  Register {:3} (Grid Volt Limit 3 Low Time): {} ms - Level 3 low voltage delay", reg, value),
                            40 => info!("  Register {:3} (Grid Volt Limit 3 High Time): {} ms - Level 3 high voltage delay", reg, value),
                            
                            // Additional Grid Protection
                            41 => info!("  Register {:3} (Grid Volt Moving Avg High): {:.1} V - Moving average high voltage limit", reg, (*value as f64) / 10.0),

                            // Grid Frequency Protection Level 1
                            42 => info!("  Register {:3} (Grid Freq Limit 1 Low): {:.2} Hz - Level 1 lower frequency threshold", reg, (*value as f64) / 100.0),
                            43 => info!("  Register {:3} (Grid Freq Limit 1 High): {:.2} Hz - Level 1 upper frequency threshold", reg, (*value as f64) / 100.0),
                            44 => info!("  Register {:3} (Grid Freq Limit 1 Low Time): {} ms - Level 1 low frequency delay", reg, value),
                            45 => info!("  Register {:3} (Grid Freq Limit 1 High Time): {} ms - Level 1 high frequency delay", reg, value),

                            // Grid Frequency Protection Level 2
                            46 => info!("  Register {:3} (Grid Freq Limit 2 Low): {:.2} Hz - Level 2 lower frequency threshold", reg, (*value as f64) / 100.0),
                            47 => info!("  Register {:3} (Grid Freq Limit 2 High): {:.2} Hz - Level 2 upper frequency threshold", reg, (*value as f64) / 100.0),
                            48 => info!("  Register {:3} (Grid Freq Limit 2 Low Time): {} ms - Level 2 low frequency delay", reg, value),
                            49 => info!("  Register {:3} (Grid Freq Limit 2 High Time): {} ms - Level 2 high frequency delay", reg, value),

                            // Grid Frequency Protection Level 3
                            50 => info!("  Register {:3} (Grid Freq Limit 3 Low): {:.2} Hz - Level 3 lower frequency threshold", reg, (*value as f64) / 100.0),
                            51 => info!("  Register {:3} (Grid Freq Limit 3 High): {:.2} Hz - Level 3 upper frequency threshold", reg, (*value as f64) / 100.0),
                            52 => info!("  Register {:3} (Grid Freq Limit 3 Low Time): {} ms - Level 3 low frequency delay", reg, value),
                            53 => info!("  Register {:3} (Grid Freq Limit 3 High Time): {} ms - Level 3 high frequency delay", reg, value),

                            // Power Quality Control
                            54 => info!("  Register {:3} (Max Q Percent for QV): {}% - Maximum reactive power percentage for Q(V) control", reg, value),
                            55 => info!("  Register {:3} (V1L): {:.1} V - Lower voltage threshold 1 for Q(V) curve", reg, (*value as f64) / 10.0),
                            56 => info!("  Register {:3} (V2L): {:.1} V - Lower voltage threshold 2 for Q(V) curve", reg, (*value as f64) / 10.0),
                            57 => info!("  Register {:3} (V1H): {:.1} V - Upper voltage threshold 1 for Q(V) curve", reg, (*value as f64) / 10.0),
                            58 => info!("  Register {:3} (V2H): {:.1} V - Upper voltage threshold 2 for Q(V) curve", reg, (*value as f64) / 10.0),
                            59 => info!("  Register {:3} (Reactive Power Cmd Type): {} - Reactive power command type", reg, value),
                            60 => info!("  Register {:3} (Active Power Percent): {}% - Active power percentage command", reg, value),
                            61 => info!("  Register {:3} (Reactive Power Percent): {}% - Reactive power percentage command", reg, value),
                            62 => info!("  Register {:3} (Power Factor Command): {:.3} - Power factor command", reg, (*value as f64) / 1000.0),
                            63 => info!("  Register {:3} (Power Soft Start Slope): {} - Power soft start slope", reg, value),
                            
                            // BMS registers (153-169)
                            153 => info!("  Register {:3} (BMS Status): {:#06x} - Battery management system status flags", reg, value),
                            154 => info!("  Register {:3} (BMS Pack Voltage): {:.2} V - Total battery pack voltage", reg, (*value as f64) / 100.0),
                            155 => info!("  Register {:3} (BMS Pack Current): {:.2} A - Total battery pack current", reg, (*value as f64) / 100.0),
                            156 => info!("  Register {:3} (BMS SOC): {}% - Battery state of charge from BMS", reg, value),
                            157 => info!("  Register {:3} (BMS Cell Temperature 1): {:.1} °C - Temperature of cell group 1", reg, (*value as f64) / 10.0),
                            158 => info!("  Register {:3} (BMS Cell Temperature 2): {:.1} °C - Temperature of cell group 2", reg, (*value as f64) / 10.0),
                            159 => info!("  Register {:3} (BMS Cell Temperature 3): {:.1} °C - Temperature of cell group 3", reg, (*value as f64) / 10.0),
                            162 => info!("  Register {:3} (BMS Cell Voltage 2): {:.3} V - Voltage of cell 2", reg, (*value as f64) / 1000.0),
                            163 => info!("  Register {:3} (BMS Cell Voltage 3): {:.3} V - Voltage of cell 3", reg, (*value as f64) / 1000.0),
                            164 => info!("  Register {:3} (BMS Cell Voltage 4): {:.3} V - Voltage of cell 4", reg, (*value as f64) / 1000.0),
                            165 => info!("  Register {:3} (BMS Cell Voltage 5): {:.3} V - Voltage of cell 5", reg, (*value as f64) / 1000.0),
                            166 => info!("  Register {:3} (BMS Cell Voltage 6): {:.3} V - Voltage of cell 6", reg, (*value as f64) / 1000.0),
                            167 => info!("  Register {:3} (BMS Cell Voltage 7): {:.3} V - Voltage of cell 7", reg, (*value as f64) / 1000.0),
                            168 => info!("  Register {:3} (BMS Cell Voltage 8): {:.3} V - Voltage of cell 8", reg, (*value as f64) / 1000.0),
                            169 => info!("  Register {:3} (BMS Cell Voltage 9): {:.3} V - Voltage of cell 9", reg, (*value as f64) / 1000.0),

                            // AutoTest registers (170-175)
                            170 => info!("  Register {:3} (AutoTest Command): {} - Command register for AutoTest", reg, value),
                            171 => {
                                let status = (value >> 0) & 0xF;
                                let step = (value >> 4) & 0xF;
                                let _reserved = (value >> 8) & 0xF;
                                info!("  Register {:3} (AutoTest Status): {:#06x}", reg, value);
                                info!("    Status: {} - {}", status, match status {
                                    0 => "Waiting - Test not started",
                                    1 => "Testing - Test in progress",
                                    2 => "Test Failed - Last test failed",
                                    3 => "Voltage Test OK - Voltage tests passed",
                                    4 => "Frequency Test OK - Frequency tests passed",
                                    5 => "Test Passed - All tests completed successfully",
                                    _ => "Unknown status"
                                });
                                info!("    Step: {} - {}", step, match step {
                                    1 => "V1L Test - Testing lower voltage limit 1",
                                    2 => "V1H Test - Testing upper voltage limit 1",
                                    3 => "F1L Test - Testing lower frequency limit 1",
                                    4 => "F1H Test - Testing upper frequency limit 1",
                                    5 => "V2L Test - Testing lower voltage limit 2",
                                    6 => "V2H Test - Testing upper voltage limit 2",
                                    7 => "F2L Test - Testing lower frequency limit 2",
                                    8 => "F2H Test - Testing upper frequency limit 2",
                                    _ => "No Test Active"
                                });
                            },
                            172 => {
                                let value_f = (*value as f64) * if value & 0x8000 != 0 { -0.1 } else { 0.1 };
                                info!("  Register {:3} (AutoTest Limit): {:.1} {}", reg, value_f,
                                    if (*reg >= 171 && *reg <= 172) || (*reg >= 175 && *reg <= 176) { "V" } else { "Hz" });
                            },
                            173 => info!("  Register {:3} (AutoTest Default Time): {} ms - Default test duration", reg, value),
                            174 => {
                                let value_f = (*value as f64) * if value & 0x8000 != 0 { -0.1 } else { 0.1 };
                                info!("  Register {:3} (AutoTest Trip Value): {:.1} {}", reg, value_f,
                                    if (*reg >= 171 && *reg <= 172) || (*reg >= 175 && *reg <= 176) { "V" } else { "Hz" });
                            },
                            175 => info!("  Register {:3} (AutoTest Trip Time): {} ms - Actual time until trip occurred", reg, value),

                            // BMS registers (176-198)
                            176 => info!("  Register {:3} (BMS Cell Voltage 16): {:.3} V - Voltage of cell 16", reg, (*value as f64) / 1000.0),
                            177 => info!("  Register {:3} (BMS Cell Resistance 1): {:.3} mΩ - Internal resistance of cell 1", reg, (*value as f64) / 1000.0),
                            178 => info!("  Register {:3} (BMS Cell Resistance 2): {:.3} mΩ - Internal resistance of cell 2", reg, (*value as f64) / 1000.0),
                            179 => info!("  Register {:3} (BMS Cell Resistance 3): {:.3} mΩ - Internal resistance of cell 3", reg, (*value as f64) / 1000.0),
                            180 => info!("  Register {:3} (BMS Cell Resistance 4): {:.3} mΩ - Internal resistance of cell 4", reg, (*value as f64) / 1000.0),
                            181 => info!("  Register {:3} (BMS Cell Resistance 5): {:.3} mΩ - Internal resistance of cell 5", reg, (*value as f64) / 1000.0),
                            182 => info!("  Register {:3} (BMS Cell Resistance 6): {:.3} mΩ - Internal resistance of cell 6", reg, (*value as f64) / 1000.0),
                            183 => info!("  Register {:3} (BMS Cell Resistance 7): {:.3} mΩ - Internal resistance of cell 7", reg, (*value as f64) / 1000.0),
                            184 => info!("  Register {:3} (BMS Cell Resistance 8): {:.3} mΩ - Internal resistance of cell 8", reg, (*value as f64) / 1000.0),
                            185 => info!("  Register {:3} (BMS Cell Resistance 9): {:.3} mΩ - Internal resistance of cell 9", reg, (*value as f64) / 1000.0),
                            186 => info!("  Register {:3} (BMS Cell Resistance 10): {:.3} mΩ - Internal resistance of cell 10", reg, (*value as f64) / 1000.0),
                            187 => info!("  Register {:3} (BMS Cell Resistance 11): {:.3} mΩ - Internal resistance of cell 11", reg, (*value as f64) / 1000.0),
                            188 => info!("  Register {:3} (BMS Cell Resistance 12): {:.3} mΩ - Internal resistance of cell 12", reg, (*value as f64) / 1000.0),
                            189 => info!("  Register {:3} (BMS Cell Resistance 13): {:.3} mΩ - Internal resistance of cell 13", reg, (*value as f64) / 1000.0),
                            190 => info!("  Register {:3} (BMS Cell Resistance 14): {:.3} mΩ - Internal resistance of cell 14", reg, (*value as f64) / 1000.0),
                            191 => info!("  Register {:3} (BMS Cell Resistance 15): {:.3} mΩ - Internal resistance of cell 15", reg, (*value as f64) / 1000.0),
                            192 => info!("  Register {:3} (BMS Cell Resistance 16): {:.3} mΩ - Internal resistance of cell 16", reg, (*value as f64) / 1000.0),
                            193 => {
                                let mut flags = Vec::new();
                                if value & (1 << 0) != 0 { flags.push("Cell Overvoltage"); }
                                if value & (1 << 1) != 0 { flags.push("Cell Undervoltage"); }
                                if value & (1 << 2) != 0 { flags.push("Pack Overvoltage"); }
                                if value & (1 << 3) != 0 { flags.push("Pack Undervoltage"); }
                                if value & (1 << 4) != 0 { flags.push("Charging Overcurrent"); }
                                if value & (1 << 5) != 0 { flags.push("Discharging Overcurrent"); }
                                if value & (1 << 6) != 0 { flags.push("Cell Temperature High"); }
                                if value & (1 << 7) != 0 { flags.push("Cell Temperature Low"); }
                                if value & (1 << 8) != 0 { flags.push("Cell Imbalance"); }
                                if value & (1 << 9) != 0 { flags.push("Internal Error"); }
                                info!("  Register {:3} (BMS Alarm Status): {:#06x}", reg, value);
                                let flag_str = if flags.is_empty() { 
                                    "None".to_string() 
                                } else { 
                                    flags.join(", ") 
                                };
                                info!("    Active alarms: {}", flag_str);
                            },
                            194 => info!("  Register {:3} (BMS Warning Status): {:#06x} - Battery warning status flags", reg, value),
                            195 => info!("  Register {:3} (BMS Protection Status): {:#06x} - Battery protection status flags", reg, value),
                            196 => {
                                let major = value >> 8;
                                let minor = value & 0xFF;
                                info!("  Register {:3} (BMS Software Version): {}.{} - BMS firmware version", reg, major, minor);
                            },
                            197 => {
                                let major = value >> 8;
                                let minor = value & 0xFF;
                                info!("  Register {:3} (BMS Hardware Version): {}.{} - BMS hardware version", reg, major, minor);
                            },
                            198 => info!("  Register {:3} (BMS Maximum Cell Difference): {:.3} V - Maximum voltage difference between cells", reg, (*value as f64) / 1000.0),

                            // Default case for unknown registers
                            _ => info!("  Register {:3}: {}", reg, value),
                        }
                    }
                    
                    if let Err(e) = self.publish_hold_message(register, pairs, inverter).await {
                        error!("Failed to publish hold message: {}", e);
                        if let Ok(mut stats) = self.stats.lock() {
                            stats.mqtt_errors += 1;
                        }
                    }
                }
                DeviceFunction::WriteSingle => {
                    debug!("Processing WriteSingle packet");
                    let register = td.register();
                    let value = td.value();
                    if let Err(e) = self.channels.to_register_cache.send(register_cache::ChannelData::RegisterData(register, value)) {
                        error!("Failed to cache register {}: {}", register, e);
                        if let Ok(mut stats) = self.stats.lock() {
                            stats.register_cache_errors += 1;
                        }
                    }
                    if let Err(e) = self.publish_write_confirmation(register, value, inverter).await {
                        error!("Failed to publish write confirmation: {}", e);
                        if let Ok(mut stats) = self.stats.lock() {
                            stats.mqtt_errors += 1;
                        }
                    }
                }
                DeviceFunction::WriteMulti => {
                    debug!("Processing WriteMulti packet");
                    let pairs = td.pairs();
                    for (register, value) in &pairs {
                        if let Err(e) = self.channels.to_register_cache.send(register_cache::ChannelData::RegisterData(*register, *value)) {
                            error!("Failed to cache register {}: {}", register, e);
                            if let Ok(mut stats) = self.stats.lock() {
                                stats.register_cache_errors += 1;
                            }
                        }
                    }
                    if let Err(e) = self.publish_write_multi_confirmation(pairs, inverter).await {
                        error!("Failed to publish write multi confirmation: {}", e);
                        if let Ok(mut stats) = self.stats.lock() {
                            stats.mqtt_errors += 1;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn inverter_receiver(&self) -> Result<()> {
        let mut receiver = self.channels.from_inverter.subscribe();
        let mut buffer_size = 0;
        const BUFFER_CLEAR_THRESHOLD: usize = 1024; // 1KB threshold for buffer clearing

        while let Ok(message) = receiver.recv().await {
            match message {
                inverter::ChannelData::Packet(packet) => {
                    // Track buffer size
                    buffer_size += std::mem::size_of_val(&packet);
                    
                    // Clear buffer if threshold exceeded
                    if buffer_size >= BUFFER_CLEAR_THRESHOLD {
                        debug!("Clearing receiver buffer (size: {} bytes)", buffer_size);
                        while let Ok(inverter::ChannelData::Packet(_)) = receiver.try_recv() {
                            // Drain excess messages
                        }
                        buffer_size = 0;
                    }

                    // Process packet based on type first to avoid unnecessary cloning
                    if let Packet::TranslatedData(ref td) = packet {
                        // Find the inverter for this packet
                        if let Some(inverter) = self.config.enabled_inverter_with_datalog(td.datalog()) {
                            // Validate serial number format before proceeding
                            if let Some(serial) = td.inverter() {
                                if !serial.to_string().chars().all(|c| c.is_ascii_alphanumeric()) {
                                    warn!("Invalid serial number format: {}", serial);
                                    if let Ok(mut stats) = self.stats.lock() {
                                        stats.serial_mismatches += 1;
                                    }
                                    continue;
                                }
                            }

                            // Update packet stats after validation
                            if let Ok(mut stats) = self.stats.lock() {
                                stats.packets_received += 1;
                                stats.last_messages.insert(td.datalog(), format!("{:?}", packet));
                                stats.translated_data_packets_received += 1;
                            }

                            if let Err(e) = self.process_inverter_packet(packet.clone(), &inverter).await {
                                warn!("Failed to process packet: {}", e);
                            }
                        } else {
                            warn!("No enabled inverter found for datalog {}", td.datalog());
                        }
                    } else {
                        // Handle non-TranslatedData packets
                        if let Ok(mut stats) = self.stats.lock() {
                            stats.packets_received += 1;
                            match &packet {
                                Packet::Heartbeat(_) => stats.heartbeat_packets_received += 1,
                                Packet::ReadParam(_) => stats.read_param_packets_received += 1,
                                Packet::WriteParam(_) => stats.write_param_packets_received += 1,
                                _ => {}
                            }
                        }
                    }
                }
                inverter::ChannelData::Connected(datalog) => {
                    info!("Inverter connected: {}", datalog);
                    if let Err(e) = self.inverter_connected(datalog).await {
                        error!("Failed to process inverter connection: {}", e);
                    }
                }
                inverter::ChannelData::Disconnect(serial) => {
                    warn!("Inverter disconnected: {}", serial);
                    if let Ok(mut stats) = self.stats.lock() {
                        let count = stats.inverter_disconnections
                            .entry(serial)
                            .or_insert(0);
                        *count += 1;
                    }
                }
                inverter::ChannelData::Shutdown => {
                    info!("Received shutdown signal");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn inverter_connected(&self, datalog: Serial) -> Result<()> {
        let inverter = match self.config.enabled_inverter_with_datalog(datalog) {
            Some(inverter) => inverter,
            None => {
                warn!("Unknown inverter datalog connected: {}, will continue processing its data", datalog);
                return Ok(());
            }
        };

        if !inverter.publish_holdings_on_connect() {
            return Ok(());
        }

        info!("Reading all registers for inverter {}", datalog);

        // Create a packet for stats tracking
        let packet = Packet::TranslatedData(TranslatedData {
            datalog: Serial::default(),
            device_function: DeviceFunction::ReadHold,
            inverter: Serial::default(),
            register: 0,
            values: vec![],
        });

        let block_size = inverter.register_block_size();

        // Read all holding register blocks
        for start_register in (0..=240).step_by(block_size as usize) {
            self.increment_packets_sent(&packet);
            self.read_hold(inverter.clone(), start_register as u16, block_size).await?;
        }

        // Read all input register blocks
        for start_register in (0..=200).step_by(block_size as usize) {
            self.increment_packets_sent(&packet);
            self.read_inputs(inverter.clone(), start_register as u16, block_size).await?;
        }

        // Read time registers
        for num in &[1, 2, 3] {
            self.increment_packets_sent(&packet);
            self.read_time_register(
                inverter.clone(),
                commands::time_register_ops::Action::AcCharge(*num),
            ).await?;

            self.increment_packets_sent(&packet);
            self.read_time_register(
                inverter.clone(),
                commands::time_register_ops::Action::ChargePriority(*num),
            ).await?;

            self.increment_packets_sent(&packet);
            self.read_time_register(
                inverter.clone(),
                commands::time_register_ops::Action::ForcedDischarge(*num),
            ).await?;

            self.increment_packets_sent(&packet);
            self.read_time_register(
                inverter.clone(),
                commands::time_register_ops::Action::AcFirst(*num),
            ).await?;
        }

        Ok(())
    }

    async fn publish_message(&self, topic: String, payload: String, retain: bool) -> Result<()> {
        let m = mqtt::Message {
            topic,
            payload,
            retain,
        };
        let channel_data = mqtt::ChannelData::Message(m);
        
        // Try sending with retries
        let mut retries = 3;
        while retries > 0 {
            match self.channels.to_mqtt.send(channel_data.clone()) {
                Ok(_) => {
                    if let Ok(mut stats) = self.stats.lock() {
                        stats.mqtt_messages_sent += 1;
                    }
                    return Ok(());
                }
                Err(e) => {
                    if retries > 1 {
                        warn!("Failed to send MQTT message, retrying... ({} attempts left): {}", retries - 1, e);
                        tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    }
                    retries -= 1;
                }
            }
        }

        if let Ok(mut stats) = self.stats.lock() {
            stats.mqtt_errors += 1;
        }
        bail!("send(to_mqtt) failed after retries - channel closed?");
    }

    async fn publish_raw_input_messages(&self, input_all: &ReadInputAll, inverter: &config::Inverter) -> Result<()> {
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        // Publish raw values
        let topic = format!("{}/inputs/all", inverter.datalog);
        if let Err(e) = self.publish_message(topic, serde_json::to_string(input_all)?, false).await {
            error!("Failed to publish raw input messages: {}", e);
            if let Ok(mut stats) = self.stats.lock() {
                stats.mqtt_errors += 1;
            }
        }

        Ok(())
    }

    async fn publish_raw_input_messages_1(&self, input_1: &ReadInput1, inverter: &config::Inverter) -> Result<()> {
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        // Publish raw values
        let topic = format!("{}/inputs/1", inverter.datalog);
        if let Err(e) = self.publish_message(topic, serde_json::to_string(input_1)?, false).await {
            error!("Failed to publish raw input messages: {}", e);
            if let Ok(mut stats) = self.stats.lock() {
                stats.mqtt_errors += 1;
            }
        }

        Ok(())
    }

    async fn publish_raw_input_messages_2(&self, input_2: &ReadInput2, inverter: &config::Inverter) -> Result<()> {
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        // Publish raw values
        let topic = format!("{}/inputs/2", inverter.datalog);
        debug!("Publishing ReadInput2: bat_brand={}, bat_com_type={}", input_2.bat_brand, input_2.bat_com_type);
        match serde_json::to_string(input_2) {
            Ok(json) => {
                if let Err(e) = self.publish_message(topic, json, false).await {
                    error!("Failed to publish raw input messages: {}", e);
                    if let Ok(mut stats) = self.stats.lock() {
                        stats.mqtt_errors += 1;
                    }
                }
            }
            Err(e) => {
                error!("Failed to serialize ReadInput2: {}", e);
                if let Ok(mut stats) = self.stats.lock() {
                    stats.mqtt_errors += 1;
                }
            }
        }

        Ok(())
    }

    async fn publish_raw_input_messages_3(&self, input_3: &ReadInput3, inverter: &config::Inverter) -> Result<()> {
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        // Publish raw values
        let topic = format!("{}/inputs/3", inverter.datalog);
        if let Err(e) = self.publish_message(topic, serde_json::to_string(input_3)?, false).await {
            error!("Failed to publish raw input messages: {}", e);
            if let Ok(mut stats) = self.stats.lock() {
                stats.mqtt_errors += 1;
            }
        }

        Ok(())
    }

    async fn publish_raw_input_messages_4(&self, input_4: &ReadInput4, inverter: &config::Inverter) -> Result<()> {
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        // Publish raw values
        let topic = format!("{}/inputs/4", inverter.datalog);
        if let Err(e) = self.publish_message(topic, serde_json::to_string(input_4)?, false).await {
            error!("Failed to publish raw input messages: {}", e);
            if let Ok(mut stats) = self.stats.lock() {
                stats.mqtt_errors += 1;
            }
        }

        Ok(())
    }

    async fn publish_raw_input_messages_5(&self, input_5: &ReadInput5, inverter: &config::Inverter) -> Result<()> {
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        // Publish raw values
        let topic = format!("{}/inputs/5", inverter.datalog);
        if let Err(e) = self.publish_message(topic, serde_json::to_string(input_5)?, false).await {
            error!("Failed to publish raw input messages: {}", e);
            if let Ok(mut stats) = self.stats.lock() {
                stats.mqtt_errors += 1;
            }
        }

        Ok(())
    }

    async fn publish_raw_input_messages_6(&self, input_6: &ReadInput6, inverter: &config::Inverter) -> Result<()> {
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        // Publish raw values
        let topic = format!("{}/inputs/6", inverter.datalog);
        if let Err(e) = self.publish_message(topic, serde_json::to_string(input_6)?, false).await {
            error!("Failed to publish raw input messages: {}", e);
            if let Ok(mut stats) = self.stats.lock() {
                stats.mqtt_errors += 1;
            }
        }

        Ok(())
    }

    async fn publish_hold_message(&self, _register: u16, pairs: Vec<(u16, u16)>, inverter: &config::Inverter) -> Result<()> {
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        // Publish raw values
        for (reg, value) in pairs {
            let topic = format!("{}/hold/{}", inverter.datalog, reg);
            if let Err(e) = self.publish_message(topic, value.to_string(), true).await {
                error!("Failed to publish hold message: {}", e);
                if let Ok(mut stats) = self.stats.lock() {
                    stats.mqtt_errors += 1;
                }
            }
        }

        Ok(())
    }

    async fn publish_write_confirmation(&self, register: u16, value: u16, inverter: &config::Inverter) -> Result<()> {
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        let topic = format!("{}/write/status", inverter.datalog);
        if let Err(e) = self.publish_message(topic, format!("OK: {} = {}", register, value), false).await {
            error!("Failed to publish write confirmation: {}", e);
            if let Ok(mut stats) = self.stats.lock() {
                stats.mqtt_errors += 1;
            }
        }

        Ok(())
    }

    async fn publish_write_multi_confirmation(&self, pairs: Vec<(u16, u16)>, inverter: &config::Inverter) -> Result<()> {
        if !self.config.mqtt().enabled() {
            return Ok(());
        }

        let topic = format!("{}/write_multi/status", inverter.datalog);
        if let Err(e) = self.publish_message(topic, format!("OK: {:?}", pairs), false).await {
            error!("Failed to publish write multi confirmation: {}", e);
            if let Ok(mut stats) = self.stats.lock() {
                stats.mqtt_errors += 1;
            }
        }

        Ok(())
    }
}
