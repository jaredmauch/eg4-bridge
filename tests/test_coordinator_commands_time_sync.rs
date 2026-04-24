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
#[cfg_attr(not(feature = "mocks"), ignore)]
async fn update_time() {
    common_setup();

    let inverter = Factory::inverter();
    let channels = Channels::new();
    spawn_coordinator_forwarder(&channels);

    let subject =
        coordinator::commands::timesync::TimeSync::new(channels.clone(), inverter.clone());

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
                register: 12,
                values: vec![3, 0]
            })
        );

        let inverter_time_packet = Packet::TranslatedData(eg4::packet::TranslatedData {
            datalog: inverter.datalog().unwrap(),
            device_function: eg4::packet::DeviceFunction::ReadHold,
            inverter: inverter.serial().unwrap(),
            register: 12,
            values: vec![22, 6, 18, 21, 03, 10],
        });
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(inverter_time_packet))?;

        assert_eq!(
            unwrap_inverter_channeldata_packet(to_inverter.recv().await?),
            Packet::TranslatedData(eg4::packet::TranslatedData {
                datalog: inverter.datalog().unwrap(),
                device_function: eg4::packet::DeviceFunction::WriteMulti,
                inverter: inverter.serial().unwrap(),
                register: 12,
                values: vec![22, 3, 4, 5, 6, 7]
            })
        );

        let inverter_ok_packet = Packet::TranslatedData(eg4::packet::TranslatedData {
            datalog: inverter.datalog().unwrap(),
            device_function: eg4::packet::DeviceFunction::WriteMulti,
            inverter: inverter.serial().unwrap(),
            register: 12,
            values: vec![3, 0],
        });
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(inverter_ok_packet))?;

        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(tf, sf).unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "mocks"), ignore)]
async fn time_already_correct() {
    common_setup();

    let inverter = Factory::inverter();
    let channels = Channels::new();
    spawn_coordinator_forwarder(&channels);

    let subject =
        coordinator::commands::timesync::TimeSync::new(channels.clone(), inverter.clone());

    let sf = async {
        subject.run().await?;
        Ok::<(), anyhow::Error>(())
    };

    let tf = async {
        assert_eq!(
            unwrap_inverter_channeldata_packet(channels.to_inverter.subscribe().recv().await?),
            Packet::TranslatedData(eg4::packet::TranslatedData {
                datalog: inverter.datalog().unwrap(),
                device_function: eg4::packet::DeviceFunction::ReadHold,
                inverter: inverter.serial().unwrap(),
                register: 12,
                values: vec![3, 0]
            })
        );

        let inverter_time_packet = Packet::TranslatedData(eg4::packet::TranslatedData {
            datalog: inverter.datalog().unwrap(),
            device_function: eg4::packet::DeviceFunction::ReadHold,
            inverter: inverter.serial().unwrap(),
            register: 12,
            values: vec![22, 3, 4, 5, 6, 7],
        });
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(inverter_time_packet))?;

        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(tf, sf).unwrap();
}
