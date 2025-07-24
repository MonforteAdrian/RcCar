use embassy_nrf::{bind_interrupts, peripherals::TWISPI0, twim};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};

bind_interrupts!(pub struct Irqs {
    TWISPI0 => twim::InterruptHandler<TWISPI0>;
});

pub static TWIN_CHANNEL: Channel<ThreadModeRawMutex, TwinCommand, 1> = Channel::new();

pub struct TwinCommand {
    channel: u8,
    value: u8,
}

impl TwinCommand {
    pub fn new(channel: u8, value: u8) -> Self {
        Self { channel, value }
    }

    pub fn channel(&self) -> u8 {
        self.channel
    }

    pub fn value(&self) -> u8 {
        self.value
    }
}
