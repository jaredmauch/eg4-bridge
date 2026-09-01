use crate::prelude::*;
use std::sync::{Arc, Mutex};

// this just needs to be bigger than the max register we'll see
const REGISTER_COUNT: usize = 512;

#[derive(Clone, Debug)]
pub enum ChannelData {
    ReadRegister(u16, Arc<Mutex<Option<oneshot::Sender<u16>>>>),
    RegisterData(u16, u16),
    Shutdown,
}

pub struct RegisterCache {
    channels: Channels,
    register_data: Arc<Mutex<[u16; REGISTER_COUNT]>>,
}

impl RegisterCache {
    pub fn new(channels: Channels) -> Self {
        let register_data = Arc::new(Mutex::new([0; REGISTER_COUNT]));

        Self {
            channels,
            register_data,
        }
    }

    pub async fn start(&self) -> Result<()> {
        futures::try_join!(self.cache_getter(), self.cache_setter())?;

        Ok(())
    }

    // external helper method to simplify access to the cache, use like so:
    //
    //   RegisterCache::get(&self.channels, 1);
    //
    pub async fn get(channels: &Channels, register: u16) -> u16 {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let channel_data = ChannelData::ReadRegister(register, tx);
        debug!("Reading register {} from cache", register);
        let _ = channels.read_register_cache.send(channel_data);
        rx.await
            .expect("unexpected error reading from register cache")
    }

    async fn cache_getter(&self) -> Result<()> {
        let mut receiver = self.channels.read_register_cache.subscribe();

        debug!("register_cache getter starting");

        loop {
            match receiver.recv().await {
                Ok(ChannelData::ReadRegister(register, tx)) => {
                    let value = self.register_data.lock().unwrap()[register as usize];
                    debug!("Cache hit for register {}: value = {}", register, value);
                    if let Ok(mut tx) = tx.lock() {
                        if let Some(tx) = tx.take() {
                            let _ = tx.send(value);
                        }
                    }
                }
                Ok(ChannelData::Shutdown) => break,
                Ok(_) => (),
                Err(e) => {
                    if !crate::channels::broadcast_recv_continue(e, "register_cache getter") {
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn cache_setter(&self) -> Result<()> {
        let mut receiver = self.channels.to_register_cache.subscribe();

        debug!("register_cache setter starting");

        loop {
            match receiver.recv().await {
                Ok(ChannelData::RegisterData(register, value)) => {
                    debug!("Caching register {} with value {}", register, value);
                    self.register_data.lock().unwrap()[register as usize] = value;
                }
                Ok(ChannelData::Shutdown) => break,
                Ok(_) => (),
                Err(e) => {
                    if !crate::channels::broadcast_recv_continue(e, "register_cache setter") {
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}
