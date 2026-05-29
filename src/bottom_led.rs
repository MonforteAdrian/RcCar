use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};

pub static BOTTOM_LEDS_CHANNEL: Channel<ThreadModeRawMutex, BottomLedCommand, 4> = Channel::new();

#[derive(Clone, Copy)]
pub enum BottomLedCommand {
    AllOff,
    AllOn,
}
