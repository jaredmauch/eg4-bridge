//! Coordinator integration tests: packet fan-out (register cache, MQTT, optional Influx) and the
//! MQTT command → inverter path. Inverters are disabled unless a test needs a TCP listener stub.
//!
//! ## MQTT broker (default: **not** required)
//!
//! Each test uses [`SkipMqttBrokerGuard`], which sets `EG4_TEST_SKIP_MQTT_BROKER=1`. The coordinator
//! then skips spawning [`eg4_bridge::mqtt::Mqtt::start`] (no TCP connection to `mqtt.host:port`),
//! while still driving the same publish paths through the in-process `to_mqtt` broadcast channel.
//! So **`cargo test --test test_coordinator` does not need mosquitto or any MQTT daemon.**
//!
//! ## Running against a real broker (optional)
//!
//! If you remove the guard for an experiment and leave `EG4_TEST_SKIP_MQTT_BROKER` unset, the
//! coordinator will try to connect to whatever `mqtt.host` / `mqtt.port` are in your config
//! (often `localhost:1883` from `config.yaml.example`). **Start a broker first**, for example:
//!
//! ```text
//! mosquitto -p 1883
//! # or: docker run --rm -p 1883:1883 eclipse-mosquitto:2
//! ```

mod common;

use common::*;
use eg4_bridge::eg4::packet::{DeviceFunction, Packet, TranslatedData};
use eg4_bridge::prelude::*;
use eg4_bridge::{config, database, eg4, mqtt};
use mockito::Matcher;
use serde_json::json;
use std::sync::Arc;
use std::sync::Once;
use tokio::net::TcpListener;
use tokio::sync::broadcast::error::TryRecvError;

/// Shared baseline: no file-backed registers, no datalog writer, no DB, inverters off (no TCP).
fn quiet_bridge_config() -> config::Config {
    let mut c = Factory::example_config();
    c.datalog_file = None;
    c.register_file = None;
    for db in &mut c.databases {
        db.enabled = false;
    }
    for inv in &mut c.inverters {
        inv.enabled = false;
    }
    c
}

fn arc_config(c: config::Config) -> Arc<ConfigWrapper> {
    Arc::new(ConfigWrapper::from_config(c))
}

struct SkipMqttBrokerGuard;
static SKIP_MQTT_BROKER_ONCE: Once = Once::new();

impl SkipMqttBrokerGuard {
    fn set() -> Self {
        SKIP_MQTT_BROKER_ONCE.call_once(|| unsafe {
            std::env::set_var("EG4_TEST_SKIP_MQTT_BROKER", "1");
        });
        Self
    }
}

#[tokio::test]
async fn publishes_read_hold_mqtt() {
    let _mqtt_guard = SkipMqttBrokerGuard::set();
    common_setup();

    let mut c = quiet_bridge_config();
    c.influx.enabled = false;
    c.mqtt.enabled = true;

    let config = arc_config(c);
    let inverter = config.inverters()[0].clone();
    let datalog = inverter.datalog().expect("example inverter has datalog");
    let channels = Channels::new();
    let mut coordinator = Coordinator::new(config, channels.clone());
    let coord_stop = coordinator.clone();

    let tf = async move {
        let mut to_influx = channels.to_influx.subscribe();
        let mut to_mqtt = channels.to_mqtt.subscribe();
        let mut to_db = channels.to_database.subscribe();
        let mut to_register_cache = channels.to_register_cache.subscribe();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let packet = Packet::TranslatedData(TranslatedData {
            datalog,
            device_function: DeviceFunction::ReadHold,
            inverter: inverter.serial().expect("example inverter has serial"),
            register: 12,
            values: vec![22, 6],
        });
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(packet.clone()))?;

        let register_cache::ChannelData::RegisterData(a, b) = to_register_cache.recv().await?
        else {
            unreachable!("coordinator sends RegisterData for TranslatedData packets")
        };
        assert_eq!(a, 12);
        assert_eq!(b, 1558);

        assert_eq!(
            to_mqtt.recv().await?,
            mqtt::ChannelData::Message(mqtt::Message {
                topic: format!("{}/hold/12", datalog),
                retain: true,
                payload: "1558".to_owned()
            })
        );
        assert_eq!(to_influx.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(to_db.try_recv(), Err(TryRecvError::Empty));

        coord_stop.stop();

        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(coordinator.start(), tf).unwrap();
}

#[tokio::test]
async fn forwards_read_input_all_to_mqtt_and_influx() {
    let _mqtt_guard = SkipMqttBrokerGuard::set();
    common_setup();

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/write")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("db".to_owned(), "eg4".to_owned()),
            Matcher::UrlEncoded("precision".to_owned(), "s".to_owned()),
        ]))
        .match_body(Matcher::Any)
        .with_status(204)
        .expect(2)
        .create();

    let mut c = quiet_bridge_config();
    c.influx.enabled = true;
    c.influx.url = server.url();
    c.influx.username = None;
    c.influx.password = None;
    c.mqtt.enabled = true;

    let config = arc_config(c);
    let inverter = config.inverters()[0].clone();
    let datalog = inverter.datalog().expect("example inverter has datalog");
    let channels = Channels::new();
    let mut coordinator = Coordinator::new(config, channels.clone());
    let coord_stop = coordinator.clone();

    let tf = async move {
        let mut to_influx = channels.to_influx.subscribe();
        let mut to_mqtt = channels.to_mqtt.subscribe();

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let packet = Packet::TranslatedData(TranslatedData {
            datalog,
            device_function: DeviceFunction::ReadInput,
            inverter: inverter.serial().expect("example inverter has serial"),
            register: 0,
            values: vec![1; 254],
        });
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(packet.clone()))?;

        let mqtt::ChannelData::Message(msg) = to_mqtt.recv().await? else {
            panic!("expected MQTT message");
        };
        assert_eq!(msg.topic, format!("{}/inputs/all", datalog));
        let payload: serde_json::Value = serde_json::from_str(&msg.payload)?;
        assert_eq!(payload["soc"], json!(1));
        assert_eq!(payload["v_pv_1"], json!(25.7));

        let d = unwrap_influx_channeldata_input_data(to_influx.recv().await?);
        assert_eq!(d["register"], 0);
        assert_eq!(d["device_function"], "ReadInput");
        assert_eq!(d["raw_data"]["0"], "0001");

        coord_stop.stop();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        mock.assert();

        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(coordinator.start(), tf).unwrap();
}

#[tokio::test]
async fn forwards_read_input_all_to_influx_and_database_channels() {
    let _mqtt_guard = SkipMqttBrokerGuard::set();
    common_setup();

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/write")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("db".to_owned(), "eg4".to_owned()),
            Matcher::UrlEncoded("precision".to_owned(), "s".to_owned()),
        ]))
        .match_body(Matcher::Any)
        .with_status(204)
        .expect(2)
        .create();

    let mut c = quiet_bridge_config();
    c.influx.enabled = true;
    c.influx.url = server.url();
    c.influx.username = None;
    c.influx.password = None;
    c.mqtt.enabled = false;
    c.databases = vec![config::Database {
        enabled: true,
        // Intentionally invalid target for this test; we only assert coordinator fan-out.
        url: "postgres://eg4:eg4@127.0.0.1:1/eg4".to_owned(),
    }];

    let config = arc_config(c);
    let inverter = config.inverters()[0].clone();
    let datalog = inverter.datalog().expect("example inverter has datalog");
    let channels = Channels::new();
    let mut coordinator = Coordinator::new(config, channels.clone());
    let coord_stop = coordinator.clone();

    let tf = async move {
        let mut to_influx = channels.to_influx.subscribe();
        let mut to_db = channels.to_database.subscribe();

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let packet = Packet::TranslatedData(TranslatedData {
            datalog,
            device_function: DeviceFunction::ReadInput,
            inverter: inverter.serial().expect("example inverter has serial"),
            register: 0,
            values: vec![1; 254],
        });
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(packet.clone()))?;

        let d = unwrap_influx_channeldata_input_data(to_influx.recv().await?);
        assert_eq!(d["register"], 0);
        assert_eq!(d["device_function"], "ReadInput");

        let db_msg = to_db.recv().await?;
        let database::ChannelData::ReadInputAll(input_all) = db_msg else {
            panic!("expected database ReadInputAll message");
        };
        assert_eq!(input_all.soc, 1);

        coord_stop.stop();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        mock.assert();

        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(coordinator.start(), tf).unwrap();
}

#[tokio::test]
async fn mqtt_read_hold_command_reaches_inverter_and_publishes_hold() {
    let _mqtt_guard = SkipMqttBrokerGuard::set();
    common_setup();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let mut c = quiet_bridge_config();
    c.influx.enabled = false;
    c.mqtt.enabled = true;
    c.inverters[0].enabled = true;
    c.inverters[0].host = "127.0.0.1".to_owned();
    c.inverters[0].port = addr.port();

    let config = arc_config(c);
    let inverter = config.inverters()[0].clone();
    let datalog = inverter.datalog().expect("example inverter has datalog");

    let channels = Channels::new();
    let mut coordinator = Coordinator::new(config, channels.clone());
    let coord_stop = coordinator.clone();

    let accept_task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((_sock, _)) => continue,
                Err(_) => break,
            }
        }
    });

    let tf = async move {
        let mut to_inverter = channels.to_inverter.subscribe();
        let mut to_mqtt = channels.to_mqtt.subscribe();
        let mut to_influx = channels.to_influx.subscribe();
        let mut to_db = channels.to_database.subscribe();

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let message = mqtt::Message {
            topic: "cmd/all/read/hold/12".to_owned(),
            retain: false,
            payload: "".to_owned(),
        };
        channels
            .from_mqtt
            .send(mqtt::ChannelData::Message(message))
            .unwrap();

        let packet_out = Packet::TranslatedData(TranslatedData {
            datalog,
            device_function: DeviceFunction::ReadHold,
            inverter: inverter.serial().expect("serial"),
            register: 12,
            values: vec![1, 0],
        });
        assert_eq!(
            to_inverter.recv().await?,
            eg4::inverter::ChannelData::Packet(packet_out),
        );

        let reply = Packet::TranslatedData(TranslatedData {
            datalog,
            device_function: DeviceFunction::ReadHold,
            inverter: inverter.serial().expect("serial"),
            register: 12,
            values: vec![22, 6],
        });
        channels
            .from_inverter
            .send(eg4::inverter::ChannelData::Packet(reply))
            .unwrap();

        assert_eq!(
            to_mqtt.recv().await?,
            mqtt::ChannelData::Message(mqtt::Message {
                topic: format!("{}/hold/12", datalog),
                retain: true,
                payload: "1558".to_owned()
            })
        );

        assert_eq!(to_influx.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(to_db.try_recv(), Err(TryRecvError::Empty));

        coord_stop.stop();
        accept_task.abort();

        Ok::<(), anyhow::Error>(())
    };

    futures::try_join!(coordinator.start(), tf).unwrap();
}

