use crate::prelude::*;
use crate::eg4::packet::BatteryStatusString;
use crate::coordinator::PacketStats;

use rumqttc::{AsyncClient, Event, EventLoop, Incoming, LastWill, MqttOptions, Publish, QoS};
use std::sync::{Arc, Mutex};

// Message {{{
#[derive(Eq, PartialEq, Debug, Clone)]
pub struct Message {
    pub topic: String,
    pub retain: bool,
    pub payload: String,
}

pub enum TargetInverter {
    Serial(Serial),
    All,
}

impl Message {
    pub fn for_param(rp: crate::eg4::packet::ReadParam) -> Result<Vec<Message>> {
        let mut r = Vec::new();

        for (register, value) in rp.pairs() {
            r.push(mqtt::Message {
                topic: format!("{}/param/{}", rp.datalog, register),
                retain: true,
                payload: serde_json::to_string(&value)?,
            });
        }

        Ok(r)
    }

    pub fn for_hold(td: crate::eg4::packet::TranslatedData) -> Result<Vec<Message>> {
        let mut r = Vec::new();

        for (register, value) in td.pairs() {
            r.push(mqtt::Message {
                topic: format!("{}/hold/{}", td.datalog, register),
                retain: true,
                payload: serde_json::to_string(&value)?,
            });

            if register == 21 {
                let bits = crate::eg4::packet::Register21Bits::new(value);
                r.push(mqtt::Message {
                    topic: format!("{}/hold/{}/bits", td.datalog, register),
                    retain: true,
                    payload: serde_json::to_string(&bits)?,
                });
            }

            if register == 110 {
                let bits = crate::eg4::packet::Register110Bits::new(value);
                r.push(mqtt::Message {
                    topic: format!("{}/hold/{}/bits", td.datalog, register),
                    retain: true,
                    payload: serde_json::to_string(&bits)?,
                });
            }
        }

        Ok(r)
    }

    pub fn for_input_all(
        inputs: &crate::eg4::packet::ReadInputAll,
        datalog: crate::eg4::inverter::Serial,
    ) -> Result<Message> {
        Ok(mqtt::Message {
            topic: format!("{}/inputs/all", datalog),
            retain: false,
            payload: serde_json::to_string(&inputs)?,
        })
    }

    pub fn for_input_with_parsed(
        td: crate::eg4::packet::TranslatedData,
        publish_individual: bool,
        parsed_input: Option<crate::eg4::packet::ReadInput>,
    ) -> Result<Vec<Message>> {
        use crate::eg4::packet::ReadInput;

        let mut r = Vec::new();

        if publish_individual {
            let mut fault_code_registers_seen = false;
            let mut fault_code = 0;
            let mut warning_code_registers_seen = false;
            let mut warning_code = 0;

            for (register, value) in td.pairs() {
                r.push(mqtt::Message {
                    topic: format!("{}/input/{}", td.datalog, register),
                    retain: false,
                    payload: serde_json::to_string(&value)?,
                });

                if register == 0 {
                    r.push(mqtt::Message {
                        topic: format!("{}/input/{}/parsed", td.datalog, register),
                        retain: false,
                        payload: crate::eg4::packet::StatusString::from_value(value).to_owned(),
                    });
                }

                if register == 60 {
                    fault_code |= value as u32;
                    fault_code_registers_seen = true;
                }
                if register == 61 {
                    fault_code |= (value as u32) << 16;
                    fault_code_registers_seen = true;
                }

                if register == 62 {
                    warning_code |= value as u32;
                    warning_code_registers_seen = true;
                }
                if register == 63 {
                    warning_code |= (value as u32) << 16;
                    warning_code_registers_seen = true;
                }
            }

            if warning_code_registers_seen {
                r.push(mqtt::Message {
                    topic: format!("{}/input/warning_code/parsed", td.datalog),
                    retain: false,
                    payload: crate::eg4::packet::WarningCodeString::from_value(warning_code).to_owned(),
                });
            }

            if fault_code_registers_seen {
                r.push(mqtt::Message {
                    topic: format!("{}/input/fault_code/parsed", td.datalog),
                    retain: false,
                    payload: crate::eg4::packet::FaultCodeString::from_value(fault_code).to_owned(),
                });
            }
        }

        match parsed_input {
            Some(ReadInput::ReadInputAll(r_all)) => r.push(mqtt::Message {
                topic: format!("{}/inputs/all", td.datalog),
                retain: false,
                payload: serde_json::to_string(&r_all)?,
            }),
            Some(ReadInput::ReadInput1(r1)) => r.push(mqtt::Message {
                topic: format!("{}/inputs/1", td.datalog),
                retain: false,
                payload: serde_json::to_string(&r1)?,
            }),
            Some(ReadInput::ReadInput2(r2)) => {
                // Create the main message with all data
                r.push(mqtt::Message {
                    topic: format!("{}/inputs/2", td.datalog),
                    retain: false,
                    payload: serde_json::to_string(&r2)?,
                });
                
                // Add human-readable battery information
                let bat_brand_str = match r2.bat_brand {
                    0 => "Unknown/Not configured",
                    1 => "Pylon",
                    2 => "Dyness",
                    _ => "Unknown",
                };
                
                let bat_com_type_str = match r2.bat_com_type {
                    0 => "Unknown/Not configured",
                    1 => "RS485",
                    2 => "CAN",
                    _ => "Unknown",
                };
                
                // Add decoded battery information messages
                r.push(mqtt::Message {
                    topic: format!("{}/inputs/2/bat_brand_decoded", td.datalog),
                    retain: false,
                    payload: bat_brand_str.to_string(),
                });
                
                r.push(mqtt::Message {
                    topic: format!("{}/inputs/2/bat_com_type_decoded", td.datalog),
                    retain: false,
                    payload: bat_com_type_str.to_string(),
                });
            },
            Some(ReadInput::ReadInput3(r3)) => {
                // Create the main message with all data
                r.push(mqtt::Message {
                    topic: format!("{}/inputs/3", td.datalog),
                    retain: false,
                    payload: serde_json::to_string(&r3)?,
                });

                // Add decoded battery status messages
                let status_9_decoded = BatteryStatusString::decode_status_9(r3.bat_status_9);
                let status_inv_decoded = BatteryStatusString::decode_status_inv(r3.bat_status_inv);

                r.push(mqtt::Message {
                    topic: format!("{}/inputs/3/bat_status_9_decoded", td.datalog),
                    retain: false,
                    payload: status_9_decoded.join(", "),
                });

                r.push(mqtt::Message {
                    topic: format!("{}/inputs/3/bat_status_inv_decoded", td.datalog),
                    retain: false,
                    payload: status_inv_decoded.join(", "),
                });
            },
            Some(ReadInput::ReadInput4(r4)) => r.push(mqtt::Message {
                topic: format!("{}/inputs/4", td.datalog),
                retain: false,
                payload: serde_json::to_string(&r4)?,
            }),
            Some(ReadInput::ReadInput5(r5)) => r.push(mqtt::Message {
                topic: format!("{}/inputs/5", td.datalog),
                retain: false,
                payload: serde_json::to_string(&r5)?,
            }),
            Some(ReadInput::ReadInput6(r6)) => r.push(mqtt::Message {
                topic: format!("{}/inputs/6", td.datalog),
                retain: false,
                payload: serde_json::to_string(&r6)?,
            }),
            None => {}
        }

        Ok(r)
    }

    pub fn for_input(
        td: crate::eg4::packet::TranslatedData,
        publish_individual: bool,
    ) -> Result<Vec<Message>> {
        let parsed_input = match td.read_input() {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                warn!("ignoring {:?}", err);
                None
            }
        };
        Self::for_input_with_parsed(td, publish_individual, parsed_input)
    }

    pub fn to_command(&self, inverter: config::Inverter) -> Result<Command> {
        use Command::*;

        let (_datalog, parts) = self.split_cmd_topic()?;

        let r = match parts[..] {
            ["read", "inputs", "1"] => ReadInputs(inverter, 1),
            ["read", "inputs", "2"] => ReadInputs(inverter, 2),
            ["read", "inputs", "3"] => ReadInputs(inverter, 3),
            ["read", "inputs", "4"] => ReadInputs(inverter, 4),
            ["read", "inputs", "5"] => ReadInputs(inverter, 5),
            ["read", "inputs", "6"] => ReadInputs(inverter, 6),
            ["read", "input", register] => {
                ReadInput(inverter, register.parse()?, self.payload_int_or_1()?)
            }
            ["read", "hold", register] => {
                ReadHold(inverter, register.parse()?, self.payload_int_or_1()?)
            }
            ["read", "param", register] => ReadParam(inverter, register.parse()?),
            ["read", "ac_charge", num] => ReadAcChargeTime(inverter, num.parse()?),
            ["read", "ac_first", num] => ReadAcFirstTime(inverter, num.parse()?),
            ["read", "charge_priority", num] => ReadChargePriorityTime(inverter, num.parse()?),
            ["read", "forced_discharge", num] => ReadForcedDischargeTime(inverter, num.parse()?),
            ["set", "hold", register] => SetHold(inverter, register.parse()?, self.payload_int()?),
            ["set", "param", register] => {
                WriteParam(inverter, register.parse()?, self.payload_int()?)
            }
            ["set", "ac_charge"] => AcCharge(inverter, self.payload_bool()),
            ["set", "ac_charge", num] => {
                SetAcChargeTime(inverter, num.parse()?, self.payload_start_end_time()?)
            }
            ["set", "ac_first", num] => {
                SetAcFirstTime(inverter, num.parse()?, self.payload_start_end_time()?)
            }
            ["set", "charge_priority"] => ChargePriority(inverter, self.payload_bool()),
            ["set", "charge_priority", num] => {
                SetChargePriorityTime(inverter, num.parse()?, self.payload_start_end_time()?)
            }
            ["set", "forced_discharge"] => ForcedDischarge(inverter, self.payload_bool()),
            ["set", "forced_discharge", num] => {
                SetForcedDischargeTime(inverter, num.parse()?, self.payload_start_end_time()?)
            }
            ["set", "charge_rate_pct"] => ChargeRate(inverter, self.payload_int()?),
            ["set", "discharge_rate_pct"] => DischargeRate(inverter, self.payload_int()?),
            ["set", "ac_charge_rate_pct"] => AcChargeRate(inverter, self.payload_int()?),
            ["set", "ac_charge_soc_limit_pct"] => AcChargeSocLimit(inverter, self.payload_int()?),
            ["set", "discharge_cutoff_soc_limit_pct"] => {
                DischargeCutoffSocLimit(inverter, self.payload_int()?)
            }
            [..] => bail!("unhandled: {:?}", self),
        };

        Ok(r)
    }

    // given a cmd Message, return the datalog it is intended for.
    //
    // eg cmd/AB12345678/set/ac_charge => (AB12345678, ['set', 'ac_charge'])
    pub fn split_cmd_topic(&self) -> Result<(TargetInverter, Vec<&str>)> {
        let parts: Vec<&str> = self.topic.split('/').collect();

        // bail if the topic is too short to handle.
        // this *shouldn't* happen as our subscribe is for lxp/cmd/{datalog}/#
        if parts.len() < 2 {
            bail!("ignoring badly formed MQTT topic: {}", self.topic);
        }

        // parts[0] should be cmd
        let datalog = parts[1];
        let rest = parts[2..].to_vec();

        if datalog == "all" {
            Ok((TargetInverter::All, rest))
        } else {
            let serial = Serial::from_str(datalog)?;
            Ok((TargetInverter::Serial(serial), rest))
        }
    }

    // not entirely happy with this return type but it avoids needing to expose a struct for now
    fn payload_start_end_time(&self) -> Result<[u8; 4]> {
        use serde::Deserialize;
        #[derive(Deserialize)]
        struct StartEndTime {
            start: String,
            end: String,
        }

        // {"start":"20:00", "end":"21:00"} -> [20, 0, 21, 0]
        let t = serde_json::from_str::<StartEndTime>(&self.payload)?;
        // split on : then make u8s to return in an array
        let start: Vec<&str> = t.start.split(':').collect();
        let end: Vec<&str> = t.end.split(':').collect();
        if start.len() != 2 || end.len() != 2 {
            bail!("badly formatted time, use HH:MM")
        }
        Ok([
            start[0].parse()?,
            start[1].parse()?,
            end[0].parse()?,
            end[1].parse()?,
        ])
    }

    fn payload_int_or_1(&self) -> Result<u16> {
        self.payload_int().or(Ok(1))
    }

    fn payload_int(&self) -> Result<u16> {
        self.payload
            .parse()
            .map_err(|err| anyhow!("payload_int: {}", err))
    }

    fn payload_bool(&self) -> bool {
        matches!(
            self.payload.to_ascii_lowercase().as_str(),
            "1" | "t" | "true" | "on" | "y" | "yes"
        )
    }
} // }}}

#[derive(Eq, PartialEq, Debug, Clone)]
pub enum ChannelData {
    Message(Message),
    Shutdown,
}

pub type Sender = broadcast::Sender<ChannelData>;

#[derive(Clone)]
pub struct Mqtt {
    config: ConfigWrapper,
    shutdown: bool,
    channels: Channels,
    shared_stats: Arc<Mutex<PacketStats>>,
}

impl Mqtt {
    pub fn new(config: ConfigWrapper, channels: Channels, shared_stats: Arc<Mutex<PacketStats>>) -> Self {
        Self {
            config,
            channels,
            shutdown: false,
            shared_stats,
        }
    }

    pub async fn start(&self) -> Result<()> {
        let c = &self.config;

        if !c.mqtt().enabled() {
            info!("mqtt disabled, skipping");
            return Ok(());
        }

        let mut options = MqttOptions::new("lxp-bridge", c.mqtt().host(), c.mqtt().port());

        let will = LastWill {
            topic: self.lwt_topic(),
            message: bytes::Bytes::from("offline"),
            qos: QoS::AtLeastOnce,
            retain: true,
        };
        options.set_last_will(will);

        options.set_keep_alive(std::time::Duration::from_secs(60));
        if let (Some(u), Some(p)) = (c.mqtt().username(), c.mqtt().password()) {
            options.set_credentials(u, p);
        }

        info!(
            "initializing mqtt at {}:{}",
            c.mqtt().host(),
            c.mqtt().port()
        );

        let (client, eventloop) = AsyncClient::new(options, 10);

        futures::try_join!(
            self.setup(client.clone()),
            self.receiver(eventloop),
            self.sender(client)
        )?;

        Ok(())
    }

    pub async fn stop(&mut self) {
        info!("Stopping MQTT client...");
        self.shutdown = true;

        // Send shutdown signal
        let _ = self.channels.from_mqtt.send(ChannelData::Shutdown);
        
        // Give tasks time to process shutdown
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    async fn setup(&self, client: AsyncClient) -> Result<()> {
        client
            .publish(self.lwt_topic(), QoS::AtLeastOnce, true, "online")
            .await?;

        client
            .subscribe(
                format!("{}/cmd/all/#", self.config.mqtt().namespace()),
                QoS::AtMostOnce,
            )
            .await?;

        for inverter in self.config.enabled_inverters() {
            client
                .subscribe(
                    format!(
                        "{}/cmd/{}/#",
                        self.config.mqtt().namespace(),
                        inverter.datalog().map(|s| s.to_string()).unwrap_or_default()
                    ),
                    QoS::AtMostOnce,
                )
                .await?;

            if self.config.homeassistant_enabled() && self.config.mqtt().homeassistant().enabled() {
                let ha = home_assistant::Config::new(&inverter, &self.config.mqtt(), &self.config);
                for msg in ha.all()?.into_iter() {
                    let _ = client
                        .publish(&msg.topic, QoS::AtLeastOnce, msg.retain, msg.payload)
                        .await;
                }
            }
        }

        Ok(())
    }

    // mqtt -> coordinator
    async fn receiver(&self, mut eventloop: EventLoop) -> Result<()> {
        loop {
            if self.shutdown {
                info!("MQTT receiver shutting down");
                break;
            }

            if let Ok(event) =
                tokio::time::timeout(std::time::Duration::from_secs(1), eventloop.poll()).await
            {
                match event {
                    Ok(Event::Incoming(Incoming::Publish(publish))) => {
                        self.handle_message(publish)?;
                    }
                    Err(e) => {
                        if !self.shutdown {
                            error!("{}", e);
                            info!("reconnecting in 5s");
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        }
                    }
                    _ => {} // keepalives etc
                }
            }
        }

        info!("MQTT receiver loop exiting");
        Ok(())
    }

    fn handle_message(&self, publish: Publish) -> Result<()> {
        // remove the namespace, including the first /
        // doing it this way means we don't break if namespace happens to contain a /
        let topic = publish.topic[self.config.mqtt().namespace().len() + 1..].to_owned();

        let message = Message {
            topic,
            retain: publish.retain,
            payload: String::from_utf8(publish.payload.to_vec())?,
        };
        debug!("RX: {:?}", message);
        if self
            .channels
            .from_mqtt
            .send(ChannelData::Message(message))
            .is_err()
        {
            bail!("send(from_mqtt) failed - channel closed?");
        }

        Ok(())
    }

    // coordinator -> mqtt
    async fn sender(&self, client: AsyncClient) -> Result<()> {
        use ChannelData::*;

        let mut receiver = self.channels.to_mqtt.subscribe();

        loop {
            match receiver.recv().await? {
                Shutdown => {
                    info!("MQTT sender received shutdown signal");
                    // Flush any remaining messages before exiting
                    let _ = client.disconnect().await;
                    break;
                }
                Message(message) => {
                    let topic = format!("{}/{}", self.config.mqtt().namespace(), message.topic);
                    info!("publishing: {} = {}", topic, message.payload);
                    let payload = message.payload.as_bytes().to_vec();
                    let mut retry_count = 0;
                    loop {
                        match client.publish(&topic, QoS::AtLeastOnce, message.retain, payload.as_slice()).await {
                            Ok(_) => {
                                info!("Successfully published message to topic: {}", topic);
                                // Increment stats after successful publish
                                if let Ok(mut stats) = self.shared_stats.lock() {
                                    stats.mqtt_messages_sent += 1;
                                    debug!("Incremented MQTT messages sent counter to {}", stats.mqtt_messages_sent);
                                }
                                break;
                            }
                            Err(err) => {
                                error!("MQTT publish failed: {:?} - retrying in 10s (attempt {}/3)", err, retry_count + 1);
                                if let Ok(mut stats) = self.shared_stats.lock() {
                                    stats.mqtt_errors += 1;
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                                retry_count += 1;
                            }
                        }
                    }
                }
            }
        }

        info!("MQTT sender loop exiting");
        Ok(())
    }

    fn lwt_topic(&self) -> String {
        format!("{}/LWT", self.config.mqtt().namespace())
    }
}
