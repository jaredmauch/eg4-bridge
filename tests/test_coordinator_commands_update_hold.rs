mod common;

use common::*;
use eg4_bridge::coordinator;
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

    let register = eg4::packet::Register::Register21 as u16;
    let bit = eg4::packet::RegisterBit::AcChargeEnable;
    let enable = true;

    let subject = coordinator::commands::update_hold::UpdateHold::new(
        channels.clone(),
        inverter.clone(),
        register,
        bit,
        enable,
    );

    let sf = async {
        subject.run().await?;
        Ok::<(), anyhow::Error>(())
    };

    let tf = async {
        let mut to_inverter = channels.to_inverter.subscribe();

        assert_eq!(
            unwrap_inverter_channeldata_packet(to_inverter.recv().await?),
            Packet::TranslatedData(eg4::packet::TranslatedData {
                datalog: inverter.datalog().unwrap(),
                device_function: eg4::packet::DeviceFunction::ReadHold,
                inverter: inverter.serial().unwrap(),
                register: 21,
                values: vec![1, 0]
            })
        );

        let reply = Packet::TranslatedData(eg4::packet::TranslatedData {
            datalog: inverter.datalog().unwrap(),
            device_function: eg4::packet::DeviceFunction::ReadHold,
            inverter: inverter.serial().unwrap(),
            register: 21,
            values: vec![2, 0],
        });
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(reply))?;

        assert_eq!(
            unwrap_inverter_channeldata_packet(to_inverter.recv().await?),
            Packet::TranslatedData(eg4::packet::TranslatedData {
                datalog: inverter.datalog().unwrap(),
                device_function: eg4::packet::DeviceFunction::WriteSingle,
                inverter: inverter.serial().unwrap(),
                register: 21,
                values: vec![130, 0]
            })
        );

        let reply = Packet::TranslatedData(eg4::packet::TranslatedData {
            datalog: inverter.datalog().unwrap(),
            device_function: eg4::packet::DeviceFunction::WriteSingle,
            inverter: inverter.serial().unwrap(),
            register: 21,
            values: vec![130, 0],
        });
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(reply))?;

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

    let register = eg4::packet::Register::Register21 as u16;
    let bit = eg4::packet::RegisterBit::AcChargeEnable;
    let enable = true;

    let subject = coordinator::commands::update_hold::UpdateHold::new(
        channels.clone(),
        inverter.clone(),
        register,
        bit,
        enable,
    );

    let sf = async {
        let result = subject.run().await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Timeout waiting for reply to TranslatedData") && err.contains("ReadHold"),
            "{}",
            err
        );
        Ok::<(), anyhow::Error>(())
    };

    let tf = async {
        assert_eq!(
            unwrap_inverter_channeldata_packet(channels.to_inverter.subscribe().recv().await?),
            Packet::TranslatedData(eg4::packet::TranslatedData {
                datalog: inverter.datalog().unwrap(),
                device_function: eg4::packet::DeviceFunction::ReadHold,
                inverter: inverter.serial().unwrap(),
                register: 21,
                values: vec![1, 0]
            })
        );

        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(tf, sf).unwrap();
}
