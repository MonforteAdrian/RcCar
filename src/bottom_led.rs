use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};

// Low-level constants for WS2812B LED control
const T1H: u16 = 0x8000 | 13; // Duty = 13/20 ticks (0.8us/1.25us) for a 1
const T0H: u16 = 0x8000 | 7; // Duty 7/20 ticks (0.4us/1.25us) for a 0
const RES: u16 = 0x8000;

pub static BOTTOM_LEDS_CHANNEL: Channel<ThreadModeRawMutex, BottomLedCommand, 4> = Channel::new();

#[derive(Clone, Copy)]
pub enum BottomLedCommand {
    AllOff,
    AllOn,
    SetColor(usize, Color),   // Set a specific LED to a color
    SetAllColors([Color; 4]), // Set all LEDs at once
    Toggle(usize),            // Toggle a specific LED (on/off)
    Pattern([Color; 4]),      // Set a pattern of colors
}

#[derive(Clone, Copy)]
enum LedSide {
    Left,
    Right,
}

#[derive(Clone, Copy)]
enum LedPosition {
    Front,
    Back,
}

#[derive(Clone, Copy)]
pub struct Color {
    g: u8,
    r: u8,
    b: u8,
}

impl Color {
    pub fn encode(&self, buf: &mut [u16]) {
        let [g, r, b] = [self.g, self.r, self.b];
        for (i, &byte) in [g, r, b].iter().enumerate() {
            for bit in 0..8 {
                buf[i * 8 + bit] = if (byte << bit) & 0x80 != 0 { T1H } else { T0H };
            }
        }
    }
}

pub struct BottomLed {
    side: LedSide,
    position: LedPosition,
    color: Color,
}

impl BottomLed {
    pub const FRONT_LEFT: Self = Self::new(
        LedSide::Left,
        LedPosition::Front,
        Color { g: 0, r: 0, b: 0 },
    );
    pub const FRONT_RIGHT: Self = Self::new(
        LedSide::Right,
        LedPosition::Front,
        Color { g: 0, r: 0, b: 0 },
    );
    pub const BACK_LEFT: Self =
        Self::new(LedSide::Left, LedPosition::Back, Color { g: 0, r: 0, b: 0 });
    pub const BACK_RIGHT: Self = Self::new(
        LedSide::Right,
        LedPosition::Back,
        Color { g: 0, r: 0, b: 0 },
    );

    const fn new(side: LedSide, position: LedPosition, color: Color) -> Self {
        Self {
            side,
            position,
            color,
        }
    }

    pub const fn all_leds() -> [Self; 4] {
        [
            Self::FRONT_LEFT,
            Self::FRONT_RIGHT,
            Self::BACK_LEFT,
            Self::BACK_RIGHT,
        ]
    }

    pub const fn left_side_leds() -> [Self; 2] {
        [Self::FRONT_LEFT, Self::BACK_LEFT]
    }

    pub const fn right_side_leds() -> [Self; 2] {
        [Self::FRONT_RIGHT, Self::BACK_RIGHT]
    }

    pub const fn front_leds() -> [Self; 2] {
        [Self::FRONT_LEFT, Self::FRONT_RIGHT]
    }

    pub const fn back_leds() -> [Self; 2] {
        [Self::BACK_LEFT, Self::BACK_RIGHT]
    }
}
