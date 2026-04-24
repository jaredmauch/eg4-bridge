mod common;

use common::*;
use eg4_bridge::coordinator;
use eg4_bridge::coordinator::commands::read_hold::ReadHold;
use eg4_bridge::eg4;
use eg4_bridge::eg4::packet::Packet;
use eg4_bridge::prelude::*;

/// Minimal stand-in for the coordinator `SendPacket` → `to_inverter` path used in production
/// ([`Coordinator`](eg4_bridge::coordinator::Coordinator) loop).
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

    let register = 0_u16;
    let count = 40_u16;

    let subject = ReadHold::new(channels.clone(), inverter.clone(), register, count);

    let reply = Packet::TranslatedData(eg4::packet::TranslatedData {
        datalog: inverter.datalog().unwrap(),
        device_function: eg4::packet::DeviceFunction::ReadHold,
        inverter: inverter.serial().unwrap(),
        register: 0,
        values: vec![0, 0],
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
async fn no_reply() {
    common_setup();

    let inverter = Factory::inverter();
    let channels = Channels::new();
    spawn_coordinator_forwarder(&channels);

    let register = 0_u16;
    let count = 40_u16;

    let subject = ReadHold::new(channels.clone(), inverter.clone(), register, count);

    let sf = async {
        let result = subject.run().await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Timeout waiting for reply to TranslatedData")
                && err.contains("values: [40, 0]"),
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

    let register = 0_u16;
    let count = 40_u16;

    let subject = ReadHold::new(channels.clone(), inverter.clone(), register, count);

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
