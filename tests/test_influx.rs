use eg4_bridge::channels::Channels;
use eg4_bridge::coordinator::PacketStats;
use eg4_bridge::prelude::*;
use eg4_bridge::{config, influx};
use eg4_bridge::influx::ChannelData;
use mockito::Matcher;
use serde_json::json;
use std::sync::{Arc, Mutex};

fn setup_log() {
    let _ = env_logger::try_init();
}

#[tokio::test]
async fn sends_http_request() {
    setup_log();

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

    let mut cfg = config::Config::new("config.yaml.example".to_string()).unwrap();
    cfg.influx.url = server.url();
    cfg.register_file = None;
    let config = ConfigWrapper::from_config(cfg);
    let channels = Channels::new();
    let stats = Arc::new(Mutex::new(PacketStats::default()));

    let influx = influx::Influx::new(config, channels.clone(), stats);

    influx.start().await.unwrap();
    // `start` returns as soon as the sender task is spawned; give it time to call `subscribe`.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let json = json!({
        "time": 1000_i64,
        "serial": "5555555555",
        "datalog": "BA12345678",
        "raw_data": { "0": "00fa" }
    });
    channels
        .to_influx
        .send(ChannelData::InputData(json))
        .unwrap();
    channels.to_influx.send(ChannelData::Shutdown).unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    mock.assert();
}
