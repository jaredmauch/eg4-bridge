use crate::prelude::*;

use eg4::{
    inverter::WaitForReply,
    packet::{DeviceFunction, TranslatedData},
};

use crate::coordinator::Channels;
use crate::config;

pub struct ReadInputs {
    channels: Channels,
    inverter: config::Inverter,
    register: u16,
    count: u16,
}

impl ReadInputs {
    pub fn new<U>(channels: Channels, inverter: config::Inverter, register: U, count: u16) -> Self
    where
        U: Into<u16>,
    {
        Self {
            channels,
            inverter,
            register: register.into(),
            count,
        }
    }

    pub async fn run(&self) -> Result<Packet> {
        let packet = Packet::TranslatedData(TranslatedData {
            datalog: self.inverter.datalog().expect("datalog must be set for read_inputs command"),
            device_function: DeviceFunction::ReadInput,
            inverter: self.inverter.serial().expect("serial must be set for read_inputs command"),
            register: self.register,
            values: vec![self.count as u8, 0],
        });

        let mut receiver = self.channels.from_inverter.subscribe();

        if let Err(e) = self.channels.to_coordinator.send(crate::coordinator::ChannelData::SendPacket(packet.clone())) {
            bail!("Failed to send packet to coordinator: {}", e);
        }

        let packet = receiver.wait_for_reply(&packet).await?;
        Ok(packet)
    }
}
