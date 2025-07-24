use crate::twim::{TWIN_CHANNEL, TwinCommand};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};

pub static BIG_LEDS_CHANNEL: Channel<ThreadModeRawMutex, BigLedCommand, 1> = Channel::new();

pub enum BigLedCommand {
    Toggle,
}

pub struct BigLed {
    channel: u8,
    value: u8,
}

impl BigLed {
    const LEFT_LED: Self = Self::new(0x09, 0x00);
    const RIGHT_LED: Self = Self::new(0x0A, 0x00);

    const fn new(channel: u8, value: u8) -> Self {
        Self { channel, value }
    }
    pub const fn all_leds() -> [Self; 2] {
        [Self::LEFT_LED, Self::RIGHT_LED]
    }

    pub async fn set_value(&mut self, value: u8) {
        self.value = value;
        TWIN_CHANNEL
            .send(TwinCommand::new(self.channel, value))
            .await;
    }
}
