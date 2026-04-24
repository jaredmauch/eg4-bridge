mod common;

use common::*;
use eg4_bridge::coordinator;
use eg4_bridge::coordinator::commands::set_hold::SetHold;
use eg4_bridge::eg4;
use eg4_bridge::eg4::packet::Packet;
use eg4_bridge::prelude::*;

fn spawn_coordinator_forwarder(channels: &Channels) {
    let ch = channels.clone();
    let mut rx = ch.to_coordinator.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(coordinator::ChannelData::SendPacket(packet)) => {
                    let _ = ch
                        .to_inverter
                        .send(eg4::inverter::ChannelData::Packet(packet));
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });
}

#[tokio::test]
async fn happy_path() {
    common_setup();

    let inverter = Factory::inverter();
    let channels = Channels::new();
    spawn_coordinator_forwarder(&channels);

    let register = 5_u16;
    let value = 10_u16;

    let subject = SetHold::new(channels.clone(), inverter.clone(), register, value);

    let reply = Packet::TranslatedData(eg4::packet::TranslatedData {
        datalog: inverter.datalog().unwrap(),
        device_function: eg4::packet::DeviceFunction::WriteSingle,
        inverter: inverter.serial().unwrap(),
        register: 5,
        values: vec![10, 0],
    });

    let sf = async {
        let result = subject.run().await;
        assert_eq!(result?, reply.clone());
        Ok::<(), anyhow::Error>(())
    };

    let tf = async {
        channels.to_inverter.subscribe().recv().await?;
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(reply.clone()))?;
        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(tf, sf).unwrap();
}

#[tokio::test]
async fn bad_reply() {
    common_setup();

    let inverter = Factory::inverter();
    let channels = Channels::new();
    spawn_coordinator_forwarder(&channels);

    let register = 5_u16;
    let value = 10_u16;

    let subject = SetHold::new(channels.clone(), inverter.clone(), register, value);

    let reply = Packet::TranslatedData(eg4::packet::TranslatedData {
        datalog: inverter.datalog().unwrap(),
        device_function: eg4::packet::DeviceFunction::WriteSingle,
        inverter: inverter.serial().unwrap(),
        register: 5,
        values: vec![200, 0],
    });

    let sf = async {
        let result = subject.run().await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "failed to set register 5, got back value 200 (wanted 10)"
        );
        Ok::<(), anyhow::Error>(())
    };

    let tf = async {
        channels.to_inverter.subscribe().recv().await?;
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(reply.clone()))?;
        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(tf, sf).unwrap();
}

#[tokio::test]
async fn no_reply() {
    common_setup();

    let inverter = Factory::inverter();
    let channels = Channels::new();
    spawn_coordinator_forwarder(&channels);

    let register = 5_u16;
    let value = 10_u16;

    let subject = SetHold::new(channels.clone(), inverter.clone(), register, value);

    let sf = async {
        let result = subject.run().await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Timeout waiting for reply to TranslatedData")
                && err.contains("WriteSingle")
                && err.contains("register: 5"),
            "{}",
            err
        );
        Ok::<(), anyhow::Error>(())
    };

    let tf = async {
        channels.to_inverter.subscribe().recv().await?;
        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(tf, sf).unwrap();
}

#[tokio::test]
async fn inverter_not_receiving() {
    common_setup();

    let inverter = Factory::inverter();
    let channels = Channels::new();

    let register = 5_u16;
    let value = 10_u16;

    let subject = SetHold::new(channels.clone(), inverter.clone(), register, value);

    let sf = async {
        let result = subject.run().await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.starts_with("Failed to send packet to coordinator:"),
            "{}",
            err
        );
        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(sf).unwrap();
}
