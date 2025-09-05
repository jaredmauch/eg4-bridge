use crate::prelude::*;
use crate::eg4::packet::{Packet, TcpFrameFactory, WriteParam, ReadParam};
use crate::eg4::packet_decoder::PacketDecoder;

use {
    async_trait::async_trait,
    serde::{Serialize, Serializer},
    tokio::io::{AsyncReadExt, AsyncWriteExt},
    std::time::Duration,
    socket2::Socket,
    std::sync::{Arc, Mutex},
    std::time::Instant,
    std::sync::atomic::{AtomicU64, Ordering},
};

use crate::coordinator::PacketStats;

#[derive(Eq, PartialEq, Debug, Clone)]
pub enum ChannelData {
    Connected(Serial),  // strictly speaking, these two only ever go
    Disconnect(Serial), // inverter->coordinator, but eh.
    Packet(Packet),     // this one goes both ways through the channel.
    Shutdown,
    Heartbeat(Packet),
    ModbusError(config::Inverter, u8, crate::eg4::packet::ModbusError),
    SerialMismatch(config::Inverter, Serial, Serial),
}
pub type Sender = broadcast::Sender<ChannelData>;
pub type Receiver = broadcast::Receiver<ChannelData>;

// WaitForReply {{{
#[async_trait]
pub trait WaitForReply {
    #[cfg(not(feature = "mocks"))]
    const TIMEOUT: u64 = 30;

    #[cfg(feature = "mocks")]
    const TIMEOUT: u64 = 0; // fail immediately in tests

    async fn wait_for_reply(&mut self, packet: &Packet) -> Result<Packet>;
}
#[async_trait]
impl WaitForReply for Receiver {
    async fn wait_for_reply(&mut self, packet: &Packet) -> Result<Packet> {
        let start = std::time::Instant::now();
        let timeout_duration = std::time::Duration::from_secs(Self::TIMEOUT);

        loop {
            if start.elapsed() >= timeout_duration {
                bail!("Timeout waiting for reply to {:?} after {} seconds", packet, Self::TIMEOUT);
            }

            match (packet, self.try_recv()) {
                (
                    Packet::TranslatedData(td),
                    Ok(ChannelData::Packet(Packet::TranslatedData(reply))),
                ) => {
                    if td.datalog == reply.datalog
                        && td.register == reply.register
                        && td.device_function == reply.device_function
                    {
                        return Ok(Packet::TranslatedData(reply));
                    }
                }
                (Packet::ReadParam(rp), Ok(ChannelData::Packet(Packet::ReadParam(reply)))) => {
                    if rp.datalog == reply.datalog && rp.register == reply.register {
                        return Ok(Packet::ReadParam(reply));
                    }
                }
                (Packet::WriteParam(wp), Ok(ChannelData::Packet(Packet::WriteParam(reply)))) => {
                    if wp.datalog == reply.datalog && wp.register == reply.register {
                        return Ok(Packet::WriteParam(reply));
                    }
                }
                (Packet::Heartbeat(hb), Ok(ChannelData::Packet(Packet::Heartbeat(reply)))) => {
                    if hb.datalog == reply.datalog {
                        return Ok(Packet::Heartbeat(reply));
                    }
                }
                (_, Ok(ChannelData::Packet(_))) => {} // Mismatched packet, continue waiting
                (_, Ok(ChannelData::Heartbeat(_))) => { info!("heartbeat_rx from") } // Heartbeat received, continue waiting
                (_, Ok(ChannelData::Connected(_))) => {} // Connection status update, continue waiting
                (_, Ok(ChannelData::Disconnect(inverter_datalog))) => {
                    if inverter_datalog == packet.datalog() {
                        bail!("Inverter {} disconnected while waiting for reply", inverter_datalog);
                    }
                }
                (_, Ok(ChannelData::Shutdown)) => bail!("Channel shutdown received while waiting for reply"),
                (_, Ok(ChannelData::ModbusError(_, _, _))) => {} // Modbus error, continue waiting
                (_, Ok(ChannelData::SerialMismatch(_, _, _))) => {} // Serial mismatch, continue waiting
                (_, Err(broadcast::error::TryRecvError::Empty)) => {
                    // Channel empty, sleep briefly before retrying
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                (_, Err(err)) => bail!("Channel error while waiting for reply: {:?}", err),
            }
        }
    }
} // }}}

// Serial {{{
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Serial([u8; 10]);

impl Serial {
    pub fn new(bytes: &[u8]) -> Result<Self> {
        Ok(Self(bytes.try_into()?))
    }

    pub fn data(&self) -> [u8; 10] {
        self.0
    }

    pub fn as_bytes(&self) -> &[u8; 10] {
        &self.0
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }
}

impl From<[u8; 10]> for Serial {
    fn from(bytes: [u8; 10]) -> Self {
        Self(bytes)
    }
}

impl From<&[u8]> for Serial {
    fn from(bytes: &[u8]) -> Self {
        let mut result = [0u8; 10];
        result.copy_from_slice(&bytes[..std::cmp::min(bytes.len(), 10)]);
        Self(result)
    }
}

impl From<&str> for Serial {
    fn from(s: &str) -> Self {
        let mut result = [0u8; 10];
        let bytes = s.as_bytes();
        result.copy_from_slice(&bytes[..std::cmp::min(bytes.len(), 10)]);
        Self(result)
    }
}

impl std::fmt::Display for Serial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.0))
    }
}

impl Serialize for Serial {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl std::str::FromStr for Serial {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 10 {
            return Err(anyhow!("inverter.rs:{} must be exactly 10 characters", s));
        }

        let mut r: [u8; 10] = Default::default();
        r.copy_from_slice(s.as_bytes());
        Ok(Self(r))
    }
}

impl std::fmt::Debug for Serial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.0))
    }
} // }}}

struct MessageTimestamps {
    start_time: Instant,
    last_sent: AtomicU64,
    last_received: AtomicU64,
}

impl MessageTimestamps {
    fn new() -> Self {
        Self {
            start_time: Instant::now(),
            last_sent: AtomicU64::new(0),
            last_received: AtomicU64::new(0),
        }
    }

    fn update_sent(&self) {
        self.last_sent.store(
            self.start_time.elapsed().as_secs(),
            Ordering::SeqCst
        );
    }

    fn update_received(&self) {
        self.last_received.store(
            self.start_time.elapsed().as_secs(),
            Ordering::SeqCst
        );
    }

    fn time_since_sent(&self) -> u64 {
        self.start_time.elapsed().as_secs() - self.last_sent.load(Ordering::SeqCst)
    }

    fn time_since_received(&self) -> u64 {
        self.start_time.elapsed().as_secs() - self.last_received.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
pub struct Inverter {
    config: ConfigWrapper,
    host: String,
    channels: Channels,
    shared_stats: Arc<Mutex<PacketStats>>,
    message_timestamps: Arc<MessageTimestamps>,
}

const READ_TIMEOUT_SECS: u64 = 1; // Multiplier for read_timeout from config
const WRITE_TIMEOUT_SECS: u64 = 5; // Timeout for write operations
const RECONNECT_DELAY_SECS: u64 = 5; // Delay before reconnection attempts
const TCP_KEEPALIVE_SECS: u64 = 60; // TCP keepalive interval

impl Inverter {
    pub fn new(config: ConfigWrapper, inverter: &config::Inverter, channels: Channels) -> Self {
        let message_timestamps = Arc::new(MessageTimestamps::new());
        let timestamps_clone = Arc::clone(&message_timestamps);
        let datalog = inverter.datalog().map(|s| s.to_string()).unwrap_or_default();
        let channels_clone = channels.clone();
        
        // Start the timer thread
        tokio::spawn(async move {
            loop {
                let time_since_sent = timestamps_clone.time_since_sent();
                let time_since_received = timestamps_clone.time_since_received();
                
                if time_since_sent > 60 || time_since_received > 60 {
                    warn!(
                        "Inverter {}: No messages for {} seconds (sent) / {} seconds (received)",
                        datalog,
                        time_since_sent,
                        time_since_received
                    );

                    // If no messages received for 120 seconds, send a heartbeat
                    if time_since_received >= 120 {
                        info!("Inverter {}: Sending heartbeat after {} seconds of silence", datalog, time_since_received);
                        
                        let heartbeat = Packet::Heartbeat(crate::eg4::packet::Heartbeat {
                            datalog: datalog.parse().expect("datalog must be valid"),
                        });

                        if let Err(e) = channels_clone.to_inverter.send(ChannelData::Packet(heartbeat)) {
                            error!("Failed to send heartbeat to inverter {}: {}", datalog, e);
                        }
                    }
                }
                
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });

        Self {
            config: config.clone(),
            host: inverter.host().to_string(),
            channels,
            shared_stats: Arc::new(Mutex::new(PacketStats::default())),
            message_timestamps,
        }
    }

    pub fn new_with_stats(config: ConfigWrapper, inverter: &config::Inverter, channels: Channels, shared_stats: Arc<Mutex<PacketStats>>) -> Self {
        Self {
            config: config.clone(),
            host: inverter.host().to_string(),
            channels,
            shared_stats,
            message_timestamps: Arc::new(MessageTimestamps::new()),
        }
    }

    pub fn config(&self) -> config::Inverter {
        self.config
            .inverter_with_host(&self.host)
            .expect("can't find my inverter")
    }

    pub async fn start(&self) -> Result<()> {
        let config = self.config();
        let datalog = config.datalog().map(|s| s.to_string()).unwrap_or_default();
        let host = config.host();
        let port = config.port();
        debug!("Starting inverter {} at {}:{}", datalog, host, port);
        
        let mut attempt = 1;
        while let Err(e) = self.connect().await {
            error!("inverter {}: Connection attempt {} failed: {}", datalog, attempt, e);
            debug!(
                "inverter {}: Connection attempt {} failed with error: {:?}", 
                datalog, 
                attempt, 
                e
            );
            info!(
                "inverter {}: reconnecting in {}s (attempt {})", 
                datalog,
                RECONNECT_DELAY_SECS,
                attempt
            );
            tokio::time::sleep(std::time::Duration::from_secs(RECONNECT_DELAY_SECS)).await;
            attempt += 1;
        }

        debug!("inverter {}: Successfully established connection at {}:{}", datalog, host, port);
        info!("inverter {}: Successfully started and connected", datalog);
        Ok(())
    }

    pub async fn stop(&self) {
        info!("Stopping inverter {}...", self.config().datalog().map(|s| s.to_string()).unwrap_or_default());
        
        // Send shutdown signal
        let _ = self.channels.to_inverter.send(ChannelData::Shutdown);
        
        // Give tasks time to process shutdown
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    pub async fn connect(&self) -> Result<()> {
        debug!("Starting connect method for inverter at {}:{}", self.host, self.config().port());
        let inverter_config = self.config();
        debug!(
            "Starting connection process for inverter {} at {}:{}",
            inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default(),
            inverter_config.host(),
            inverter_config.port()
        );

        let inverter_hp = (inverter_config.host().to_owned(), inverter_config.port());
        debug!("Resolved host and port: {:?}", inverter_hp);

        // Attempt TCP connection with timeout
        debug!("Attempting TCP connection with {}s timeout", WRITE_TIMEOUT_SECS * 2);
        let stream = match tokio::time::timeout(
            Duration::from_secs(WRITE_TIMEOUT_SECS * 2),
            tokio::net::TcpStream::connect(inverter_hp)
        ).await {
            Ok(Ok(stream)) => {
                debug!("TCP connection successfully established to {}:{}", 
                    inverter_config.host(), 
                    inverter_config.port()
                );
                stream
            },
            Ok(Err(e)) => {
                error!("Failed to connect to inverter at {}:{}: {}", 
                    inverter_config.host(), 
                    inverter_config.port(), 
                    e
                );
                debug!("Detailed TCP connection error: {:?}", e);
                bail!("Failed to connect to inverter: {}", e);
            },
            Err(_) => {
                error!("Connection timeout after {} seconds when connecting to {}:{}", 
                    WRITE_TIMEOUT_SECS * 2,
                    inverter_config.host(),
                    inverter_config.port()
                );
                bail!("Connection timeout after {} seconds", WRITE_TIMEOUT_SECS * 2);
            },
        };

        // Configure TCP socket
        debug!("Configuring TCP socket options");
        let std_stream = stream.into_std()?;
        let socket = Socket::from(std_stream);
        
        // Set TCP keepalive
        if let Err(e) = socket.set_keepalive(true) {
            warn!("Failed to set TCP keepalive: {}", e);
            debug!("Detailed TCP keepalive error: {:?}", e);
        } else {
            debug!("TCP keepalive enabled");
        }
        
        // Set TCP_NODELAY based on configuration
        if inverter_config.use_tcp_nodelay() {
            if let Err(e) = socket.set_nodelay(true) {
                warn!("Failed to set TCP_NODELAY: {}", e);
                debug!("Detailed TCP_NODELAY error: {:?}", e);
            } else {
                debug!("TCP_NODELAY enabled");
            }
        }
        
        let stream = tokio::net::TcpStream::from_std(socket.into())?;

        debug!("TCP socket configuration complete");

        let (reader, writer) = stream.into_split();
        debug!("TCP stream split into reader and writer");

        // Clone necessary parts for the tasks
        let sender_config = self.config.clone();
        let receiver_config = self.config.clone();
        let sender_channels = self.channels.clone();
        let receiver_channels = self.channels.clone();
        let sender_stats = self.shared_stats.clone();
        let receiver_stats = self.shared_stats.clone();
        let sender_host = self.host.clone();
        let receiver_host = self.host.clone();
        let sender_timestamps = self.message_timestamps.clone();
        let receiver_timestamps = self.message_timestamps.clone();

        // Start sender and receiver tasks
        let _sender_handle = tokio::spawn(async move {
            let inverter = Inverter {
                config: sender_config,
                host: sender_host,
                channels: sender_channels,
                shared_stats: sender_stats,
                message_timestamps: sender_timestamps,
            };
            inverter.sender(writer).await
        });

        let _receiver_handle = tokio::spawn(async move {
            let inverter = Inverter {
                config: receiver_config,
                host: receiver_host,
                channels: receiver_channels,
                shared_stats: receiver_stats,
                message_timestamps: receiver_timestamps,
            };
            inverter.inverter_periodic_reader(reader).await
        });

        // Send Connected message after tasks are started
        let mut retries = 3;
        let mut connected = false;
        while retries > 0 && !connected {
            match self.channels.from_inverter.send(ChannelData::Connected(inverter_config.datalog().expect("datalog must be set"))) {
                Ok(_) => {
                    debug!("{}:sent Connected message", inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default());
                    connected = true;
                }
                Err(e) => {
                    warn!("{}:Failed to send Connected message (attempt {}): {}", 
                        inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default(),
                        retries,
                        e
                    );
                    debug!("Detailed Connected message error: {:?}", e);
                    if retries > 1 {
                        // Wait before retrying
                        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    }
                    retries -= 1;
                }
            }
        }

        if !connected {
            error!("{}:Failed to send Connected message after all retries - channel may be closed", 
                inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default());
        }

        // Store the task handles in the inverter for later use if needed
        debug!("Both sender and receiver tasks started successfully");
        Ok(())
    }

    async fn sender(&self, mut writer: tokio::net::tcp::OwnedWriteHalf) -> Result<()> {
        let mut receiver = self.channels.to_inverter.subscribe();
        let inverter_config = self.config();
        let frame_factory = TcpFrameFactory::new(inverter_config.datalog().expect("datalog must be set"));

        loop {
            match receiver.recv().await {
                Ok(data) => {
                    match data {
                        ChannelData::Packet(packet) => {
                            // Update timestamp when sending packet
                            self.message_timestamps.update_sent();
                            
                            let bytes = frame_factory.create_frame(&packet)?;

                            // Use timeout for write operations
                            match tokio::time::timeout(
                                Duration::from_secs(WRITE_TIMEOUT_SECS),
                                writer.write_all(&bytes)
                            ).await {
                                Ok(Ok(_)) => {
                                    // Log packet details only after successful write
                                    match &packet {
                                        Packet::Heartbeat(hb) => {
                                            info!("[sender] Sent Heartbeat packet to inverter with datalog {}", hb.datalog);
                                        }
                                        Packet::TranslatedData(td) => {
                                            info!("[sender] Sent TranslatedData packet to inverter - function: {:?}, register: {}, datalog: {}", 
                                                td.device_function, td.register, td.datalog);
                                        }
                                        Packet::ReadParam(rp) => {
                                            info!("[sender] Sent ReadParam packet to inverter - register: {}, datalog: {}", 
                                                rp.register, rp.datalog);
                                        }
                                        Packet::WriteParam(wp) => {
                                            info!("[sender] Sent WriteParam packet to inverter - register: {}, values: {:?}, datalog: {}", 
                                                wp.register, wp.values, wp.datalog);
                                        }
                                    }
                                    
                                    // Ensure data is actually sent
                                    if let Err(_e) = writer.flush().await {
                                        bail!("Failed to write to socket for {}", inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default());
                                    }
                                }
                                Ok(Err(_e)) => bail!("Failed to write packet for {}", inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default()),
                                Err(_) => bail!("Write timeout after {} seconds for {}", WRITE_TIMEOUT_SECS, inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default()),
                            }
                        }
                        ChannelData::Shutdown => {
                            info!("inverter {}: sender received shutdown signal", 
                                inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default());
                            break;
                        }
                        _ => {}
                    }
                }
                Err(_e) => {
                    warn!("eg4:inverter.rs {}: sender channel closed", 
                        inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default());
                    break;
                }
            }
        }

        info!("eg4:inverter.rs {}: sender exiting", 
            inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default());
        Ok(())
    }

    async fn inverter_periodic_reader(&self, mut socket: tokio::net::tcp::OwnedReadHalf) -> Result<()> {
        use std::time::Duration;
        use tokio::time::timeout;
        use {bytes::BytesMut, tokio_util::codec::Decoder};

        const MAX_BUFFER_SIZE: usize = 65536; // 64KB max buffer size
        let mut buf = BytesMut::with_capacity(MAX_BUFFER_SIZE); // Start with MAX_BUFFER_SIZE
        let mut decoder = PacketDecoder::new();
        let inverter_config = self.config();
        let mut to_inverter_rx = self.channels.to_inverter.subscribe();
        let config = self.config.clone();

        // Start a separate task for sending ReadParam requests
        let channels_clone = self.channels.clone();
        let inverter_config_clone = inverter_config.clone();
        tokio::spawn(async move {
            loop {
                // Wait 45 seconds before sending requests
                tokio::time::sleep(Duration::from_secs(45)).await;

                // Create and send ReadParam requests
                let read_input_packet = Packet::ReadParam(ReadParam {
                    datalog: inverter_config_clone.datalog().expect("datalog must be set"),
                    register: 0x0004,
                    values: vec![],
                });
                let read_hold_packet = Packet::ReadParam(ReadParam {
                    datalog: inverter_config_clone.datalog().expect("datalog must be set"),
                    register: 0x0003,
                    values: vec![],
                });

                if let Err(e) = channels_clone.to_inverter.send(ChannelData::Packet(read_input_packet)) {
                    warn!("Failed to send ReadInput request: {}", e);
                }
                if let Err(e) = channels_clone.to_inverter.send(ChannelData::Packet(read_hold_packet)) {
                    warn!("Failed to send ReadHold request: {}", e);
                }
            }
        });

        loop {
            // Check if we've exceeded the configured timeout
            let time_since_received = self.message_timestamps.time_since_received();
            if time_since_received >= config.inverter_timeout() {
                warn!("No data received from inverter {} for {} seconds, closing connection", 
                    inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default(),
                    time_since_received);
                break;
            }

            // Check buffer capacity and prevent potential memory issues
            if buf.len() >= MAX_BUFFER_SIZE {
                bail!("Buffer overflow: received data exceeds maximum size of {} bytes", MAX_BUFFER_SIZE);
            }

            // Use select! to efficiently wait for either data or shutdown
            tokio::select! {
                // Check for shutdown signal
                msg = to_inverter_rx.recv() => {
                    match msg {
                        Ok(ChannelData::Shutdown) => {
                            info!("Receiver received shutdown signal for {}", inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default());
                            // Process any remaining data in buffer before exiting
                            while let Some(packet) = decoder.decode_eof(&mut buf)? {
                                if let Err(_e) = self.handle_incoming_packet(packet) {
                                    warn!("Failed to handle final packet during shutdown");
                                }
                            }
                            break;
                        }
                        Ok(_) => continue,
                        Err(_e) => {
                            warn!("Error receiving from channel");
                            continue;
                        }
                    }
                }

                // Wait for socket data with timeout
                read_result = async {
                    if inverter_config.read_timeout() > 0 {
                        timeout(
                            Duration::from_secs(inverter_config.read_timeout() * READ_TIMEOUT_SECS),
                            socket.read_buf(&mut buf)
                        ).await
                    } else {
                        Ok(socket.read_buf(&mut buf).await)
                    }
                } => {
                    let len = match read_result {
                        Ok(Ok(n)) => n,
                        Ok(Err(_e)) => bail!("Read error"),
                        Err(_) => bail!("No data received for {} seconds", inverter_config.read_timeout() * READ_TIMEOUT_SECS),
                    };

                    if len == 0 {
                        // Try to process any remaining data before disconnecting
                        while let Some(packet) = decoder.decode_eof(&mut buf)? {
                            if let Err(_e) = self.handle_incoming_packet(packet) {
                                warn!("Failed to handle final packet");
                            }
                        }
                        bail!("Connection closed by peer");
                    }

                    // Process received data
                    while let Some(packet) = decoder.decode(&mut buf)? {
                        // Update timestamp when receiving packet
                        self.message_timestamps.update_received();
                        
                        let packet_clone = packet.clone();
                        info!("RX packet from {} - type: {:?}, function: {:?}", 
                            inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default(),
                            packet_clone,
                            match &packet_clone {
                                Packet::TranslatedData(td) => Some(td.device_function),
                                _ => None
                            }
                        );

                        // Handle configuration updates asynchronously
                        let config_updates = async {
                            let mut updates = Vec::new();

                            // Check datalog serial
                            if let Err(e) = self.compare_datalog(&packet) {
                                if let Some(new_datalog) = self.extract_datalog_serial(&packet) {
                                    updates.push(("datalog", new_datalog));
                                }
                            }

                            // Check inverter serial for TranslatedData packets
                            if let Packet::TranslatedData(_) = packet {
                                if let Err(e) = self.compare_inverter(&packet) {
                                    if let Some(new_serial) = self.extract_inverter_serial(&packet) {
                                        updates.push(("serial", new_serial));
                                    }
                                }
                            }

                            updates
                        };

                        // Process the packet immediately
                        if let Err(e) = self.handle_incoming_packet(packet_clone) {
                            warn!("Failed to handle packet: {}", e);
                            continue;
                        }

                        // Track received packet
                        if let Ok(mut stats) = self.shared_stats.lock() {
                            stats.packets_received += 1;
                            match &packet {
                                Packet::Heartbeat(_) => stats.heartbeat_packets_received += 1,
                                Packet::TranslatedData(_) => stats.translated_data_packets_received += 1,
                                Packet::ReadParam(_) => stats.read_param_packets_received += 1,
                                Packet::WriteParam(_) => stats.write_param_packets_received += 1,
                            }
                        }

                        // Apply configuration updates after packet processing
                        let updates = config_updates.await;
                        for (field, value) in updates {
                            match field {
                                "datalog" => {
                                    info!("Updating datalog serial from {} to {}", 
                                        inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default(),
                                        value);
                                    self.update_datalog_serial(&value).await?;
                                },
                                "serial" => {
                                    info!("Updating inverter serial from {} to {}", 
                                        inverter_config.serial().map(|s| s.to_string()).unwrap_or_default(),
                                        value);
                                    self.update_inverter_serial(&value).await?;
                                },
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        info!("inverter {}: receiver exiting", inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default());
        Ok(())
    }

    fn handle_incoming_packet(&self, packet: Packet) -> Result<()> {
        let inverter_config = self.config();
        
        // Try to send the packet and handle the result
        match self.channels.from_inverter.send(ChannelData::Packet(packet.clone())) {
            Ok(_) => {
                debug!("Successfully forwarded packet from inverter {} to coordinator", 
                    inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default());
                Ok(())
            }
            Err(e) => {
                let packet_info = match &packet {
                    Packet::TranslatedData(td) => format!("TranslatedData(register={:?}, datalog={})", td.register, td.datalog),
                    Packet::ReadParam(rp) => format!("ReadParam(register={:?}, datalog={})", rp.register, rp.datalog),
                    Packet::WriteParam(wp) => format!("WriteParam(register={:?}, datalog={})", wp.register, wp.datalog),
                    Packet::Heartbeat(hb) => format!("Heartbeat(datalog={})", hb.datalog),
                };
                
                // Log the specific error
                match e {
                    broadcast::error::SendError(_) => {
                        error!("Failed to forward packet from inverter {} ({}) - channel is closed", 
                            inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default(),
                            packet_info
                        );
                    }
                    _ => {
                        error!("Failed to forward packet from inverter {} ({}) - unexpected error: {}", 
                            inverter_config.datalog().map(|s| s.to_string()).unwrap_or_default(),
                            packet_info,
                            e
                        );
                    }
                }
                Ok(())
            }
        }
    }

    pub fn compare_datalog(&self, packet: &Packet) -> Result<()> {
        if packet.datalog() != self.config().datalog().expect("datalog must be set") {
            warn!(
                "Datalog serial mismatch: packet={}, config={}. {}",
                packet.datalog(),
                self.config().datalog().map(|s| s.to_string()).unwrap_or_default(),
                if self.config.strict_data_check() {
                    "Configuration updates are disabled due to strict_data_check=true"
                } else {
                    "Updating configuration with new datalog serial"
                }
            );
            if !self.config.strict_data_check() {
                if let Err(e) = self.config.update_inverter_datalog(
                    self.config().datalog().expect("datalog must be set"),
                    packet.datalog(),
                ) {
                    error!("Failed to update datalog serial in config: {}", e);
                }
            }
        }
        Ok(())
    }

    pub fn compare_inverter(&self, packet: &Packet) -> Result<()> {
        if let Packet::TranslatedData(td) = packet {
            if td.inverter != self.config().serial().expect("serial must be set") {
                warn!(
                    "Inverter serial mismatch: packet={}, config={}. {}",
                    td.inverter,
                    self.config().serial().map(|s| s.to_string()).unwrap_or_default(),
                    if self.config.strict_data_check() {
                        "Configuration updates are disabled due to strict_data_check=true"
                    } else {
                        "Updating configuration with new inverter serial"
                    }
                );
                if !self.config.strict_data_check() {
                    if let Err(e) = self.config.update_inverter_serial(
                        self.config().serial().expect("serial must be set"),
                        td.inverter,
                    ) {
                        error!("Failed to update inverter serial in config: {}", e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Set the inverter's output power limit
    pub async fn set_output_power_limit(&self, power_limit: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0001,
            values: vec![(power_limit >> 8) as u8, power_limit as u8],
        });
        info!("[set_output_power_limit] Sending WriteParam packet to inverter - register: 0x0001, values: {:?}, datalog: {}", 
            power_limit, packet.datalog());
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's grid-tie mode
    pub async fn set_grid_tie_mode(&self, mode: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0002,
            values: vec![(mode >> 8) as u8, mode as u8],
        });
        info!("[set_grid_tie_mode] Sending WriteParam packet to inverter - register: 0x0002, values: {:?}, datalog: {}", 
            mode, packet.datalog());
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's battery charge current
    pub async fn set_battery_charge_current(&self, current: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0003,
            values: vec![(current >> 8) as u8, current as u8],
        });
        info!("[set_battery_charge_current] Sending WriteParam packet to inverter - register: 0x0003, values: {:?}, datalog: {}", 
            current, packet.datalog());
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's battery discharge current
    pub async fn set_battery_discharge_current(&self, current: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0004,
            values: vec![(current >> 8) as u8, current as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's battery charge voltage
    pub async fn set_battery_charge_voltage(&self, voltage: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0005,
            values: vec![(voltage >> 8) as u8, voltage as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's battery discharge cutoff voltage
    pub async fn set_battery_discharge_cutoff_voltage(&self, voltage: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0006,
            values: vec![(voltage >> 8) as u8, voltage as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's AC charge current
    pub async fn set_ac_charge_current(&self, current: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0007,
            values: vec![(current >> 8) as u8, current as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's AC charge voltage
    pub async fn set_ac_charge_voltage(&self, voltage: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0008,
            values: vec![(voltage >> 8) as u8, voltage as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's AC charge frequency
    pub async fn set_ac_charge_frequency(&self, frequency: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0009,
            values: vec![(frequency >> 8) as u8, frequency as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's AC charge power factor
    pub async fn set_ac_charge_power_factor(&self, power_factor: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x000A,
            values: vec![(power_factor >> 8) as u8, power_factor as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's AC charge priority
    pub async fn set_ac_charge_priority(&self, priority: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x000B,
            values: vec![(priority >> 8) as u8, priority as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's AC charge time window
    pub async fn set_ac_charge_time(&self, start_hour: u8, start_minute: u8, end_hour: u8, end_minute: u8) -> Result<()> {
        let start_time = ((start_hour as u16) << 8) | (start_minute as u16);
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x000C,
            values: vec![(start_time >> 8) as u8, start_time as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;

        let end_time = ((end_hour as u16) << 8) | (end_minute as u16);
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x000D,
            values: vec![(end_time >> 8) as u8, end_time as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's forced discharge mode
    pub async fn set_forced_discharge_mode(&self, enabled: bool) -> Result<()> {
        let value = if enabled { 1u16 } else { 0u16 };
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x000E,
            values: vec![(value >> 8) as u8, value as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's forced discharge time window
    pub async fn set_forced_discharge_time(&self, start_hour: u8, start_minute: u8, end_hour: u8, end_minute: u8) -> Result<()> {
        let start_time = ((start_hour as u16) << 8) | (start_minute as u16);
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x000F,
            values: vec![(start_time >> 8) as u8, start_time as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;

        let end_time = ((end_hour as u16) << 8) | (end_minute as u16);
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0010,
            values: vec![(end_time >> 8) as u8, end_time as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's forced discharge power
    pub async fn set_forced_discharge_power(&self, power: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0011,
            values: vec![(power >> 8) as u8, power as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's grid-tie power limit
    pub async fn set_grid_tie_power_limit(&self, power_limit: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0012,
            values: vec![(power_limit >> 8) as u8, power_limit as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's grid-tie frequency
    pub async fn set_grid_tie_frequency(&self, frequency: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0013,
            values: vec![(frequency >> 8) as u8, frequency as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's grid-tie voltage
    pub async fn set_grid_tie_voltage(&self, voltage: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0014,
            values: vec![(voltage >> 8) as u8, voltage as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    /// Set the inverter's grid-tie power factor
    pub async fn set_grid_tie_power_factor(&self, power_factor: u16) -> Result<()> {
        let packet = Packet::WriteParam(WriteParam {
            datalog: self.config().datalog().expect("datalog must be set"),
            register: 0x0015,
            values: vec![(power_factor >> 8) as u8, power_factor as u8],
        });
        self.channels.to_inverter.send(ChannelData::Packet(packet))?;
        Ok(())
    }

    fn extract_datalog_serial(&self, packet: &Packet) -> Option<String> {
        match packet {
            Packet::TranslatedData(td) => Some(td.datalog.to_string()),
            Packet::ReadParam(rp) => Some(rp.datalog.to_string()),
            Packet::WriteParam(wp) => Some(wp.datalog.to_string()),
            Packet::Heartbeat(hb) => Some(hb.datalog.to_string()),
        }
    }

    fn extract_inverter_serial(&self, packet: &Packet) -> Option<String> {
        if let Packet::TranslatedData(td) = packet {
            Some(td.inverter.to_string())
        } else {
            None
        }
    }

    async fn update_datalog_serial(&self, new_serial: &str) -> Result<()> {
        if let Err(e) = self.config.update_inverter_datalog(
            self.config().datalog().expect("datalog must be set"),
            new_serial.into()
        ) {
            error!("Failed to update datalog serial in config: {}", e);
        }
        Ok(())
    }

    async fn update_inverter_serial(&self, new_serial: &str) -> Result<()> {
        if let Err(e) = self.config.update_inverter_serial(
            self.config().serial().expect("serial must be set"),
            new_serial.into()
        ) {
            error!("Failed to update inverter serial in config: {}", e);
        }
        Ok(())
    }
}
