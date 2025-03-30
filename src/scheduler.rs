use crate::prelude::*;
use std::time::Duration;

#[derive(Clone)]
pub struct Scheduler {
    config: ConfigWrapper,
    channels: Channels,
}

impl Scheduler {
    pub fn new(config: ConfigWrapper, channels: Channels) -> Self {
        Self { config, channels }
    }

    async fn read_input_registers(&self, inverter: &config::Inverter) -> Result<()> {
        let block_size = inverter.register_block_size();
        
        // Read all input register blocks
        for start_register in (0..=200).step_by(block_size as usize) {
            crate::coordinator::commands::read_inputs::ReadInputs::new(
                self.channels.clone(),
                inverter.clone(),
                start_register as u16,
                block_size,
            )
            .run()
            .await?;

            // Add delay between reads if configured
            if let Some(delay_ms) = inverter.delay_ms() {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        info!("Scheduler starting...");
        // Create intervals for time sync and register reading
        let mut timesync_interval = tokio::time::interval(Duration::from_secs(60));
        
        // Get the global register read interval
        let global_interval = self.config.register_read_interval().unwrap_or(60);
        let mut register_interval = tokio::time::interval(Duration::from_secs(global_interval));
        
        info!("Scheduler intervals created - timesync: 60s, register: {}s", global_interval);

        // Create a channel for shutdown notification
        let mut shutdown_rx = self.channels.from_coordinator.subscribe();

        loop {
            debug!("Scheduler waiting for interval tick or shutdown...");
            tokio::select! {
                _ = timesync_interval.tick() => {
                    debug!("Timesync interval ticked");
                    for inverter in self.config.enabled_inverters() {
                        if let Err(e) = crate::coordinator::commands::timesync::TimeSync::new(
                            self.channels.clone(),
                            inverter.clone(),
                        )
                        .run()
                        .await
                        {
                            error!("Failed to sync time for inverter {}: {}", inverter.serial().unwrap_or_default(), e);
                        }
                    }
                }
                _ = register_interval.tick() => {
                    debug!("Register interval ticked");
                    for inverter in self.config.enabled_inverters() {
                        debug!("Reading registers for inverter {}", inverter.serial().unwrap_or_default());
                        // Use inverter-specific interval if configured, otherwise use global
                        let _interval = inverter.register_read_interval.unwrap_or(global_interval);
                        
                        if let Err(e) = self.read_input_registers(&inverter).await {
                            error!("Failed to read registers for inverter {}: {}", inverter.serial().unwrap_or_default(), e);
                        }
                    }
                }
                msg = shutdown_rx.recv() => {
                    match msg {
                        Ok(crate::coordinator::ChannelData::Shutdown) => {
                            info!("Scheduler received shutdown signal");
                            return Ok(());
                        }
                        Err(e) => {
                            if matches!(e, tokio::sync::broadcast::error::RecvError::Closed) {
                                info!("Scheduler channel closed, shutting down scheduler");
                                return Ok(());
                            } else {
                                error!("Error receiving message from coordinator: {}", e);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}
