use crate::prelude::*;
use crate::eg4::inverter::ChannelData;
use crate::coordinator::ChannelData as CoordinatorChannelData;

#[derive(Debug, Clone)]
pub struct Channels {
    pub from_inverter: broadcast::Sender<ChannelData>,
    pub to_inverter: broadcast::Sender<ChannelData>,
    pub from_mqtt: broadcast::Sender<crate::mqtt::ChannelData>,
    pub to_mqtt: broadcast::Sender<crate::mqtt::ChannelData>,
    pub to_influx: broadcast::Sender<crate::influx::ChannelData>,
    pub to_database: broadcast::Sender<database::ChannelData>,
    pub read_register_cache: broadcast::Sender<register_cache::ChannelData>,
    pub to_register_cache: broadcast::Sender<register_cache::ChannelData>,
    pub from_coordinator: broadcast::Sender<CoordinatorChannelData>,
    pub to_coordinator: broadcast::Sender<CoordinatorChannelData>,
}

impl Default for Channels {
    fn default() -> Self {
        Self::new()
    }
}

impl Channels {
    pub fn new() -> Self {
        Self {
            from_inverter: Self::channel(),
            to_inverter: Self::channel(),
            from_mqtt: Self::channel(),
            to_mqtt: Self::channel(),
            to_influx: Self::channel(),
            to_database: Self::channel(),
            read_register_cache: Self::channel(),
            to_register_cache: Self::channel(),
            from_coordinator: Self::channel(),
            to_coordinator: Self::channel(),
        }
    }

    fn channel<T: Clone>() -> broadcast::Sender<T> {
        broadcast::channel(2048).0
    }
}

/// Handle a broadcast receiver error. Returns `true` if the receive loop should continue,
/// `false` if it should exit (channel closed).
pub fn broadcast_recv_continue(err: broadcast::error::RecvError, context: &str) -> bool {
    match err {
        broadcast::error::RecvError::Closed => {
            info!("{} channel closed", context);
            false
        }
        broadcast::error::RecvError::Lagged(n) => {
            warn!("{} channel lagged, missed {} messages", context, n);
            true
        }
    }
}
