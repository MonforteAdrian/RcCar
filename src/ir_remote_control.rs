use defmt::debug;
use embassy_executor::Spawner;

use crate::{
    big_led::{BIG_LEDS_CHANNEL, BigLedCommand},
    motor::{MOTORS_CHANNEL, Motor, MotorCommand},
};

pub enum IrButton {
    Ok,
    Left,
    Up,
    Right,
    Down,
    Num(u8),
    Star,
    Hash,
    Unknown(u8),
}

impl IrButton {
    pub fn from_command(cmd: u8) -> Self {
        match cmd {
            0x40 => IrButton::Ok,
            0x44 => IrButton::Left,
            0x46 => IrButton::Up,
            0x43 => IrButton::Right,
            0x15 => IrButton::Down,
            0x16 => IrButton::Num(1),
            0x19 => IrButton::Num(2),
            0x0D => IrButton::Num(3),
            0x0C => IrButton::Num(4),
            0x18 => IrButton::Num(5),
            0x5E => IrButton::Num(6),
            0x08 => IrButton::Num(7),
            0x1C => IrButton::Num(8),
            0x5A => IrButton::Num(9),
            0x42 => IrButton::Star,
            0x52 => IrButton::Num(0),
            0x4A => IrButton::Hash,
            other => IrButton::Unknown(other),
        }
    }

    pub fn execute<T: IrButtonHandler>(&self, handler: &mut T) {
        match self {
            IrButton::Ok => handler.on_ok(),
            IrButton::Left => handler.on_left(),
            IrButton::Up => handler.on_up(),
            IrButton::Right => handler.on_right(),
            IrButton::Down => handler.on_down(),
            IrButton::Num(n) => handler.on_num(*n),
            IrButton::Star => handler.on_star(),
            IrButton::Hash => handler.on_hash(),
            IrButton::Unknown(cmd) => handler.on_unknown(*cmd),
        }
    }
}

pub trait IrButtonHandler {
    fn on_ok(&mut self);
    fn on_left(&mut self);
    fn on_up(&mut self);
    fn on_right(&mut self);
    fn on_down(&mut self);
    fn on_num(&mut self, n: u8);
    fn on_star(&mut self);
    fn on_hash(&mut self);
    fn on_unknown(&mut self, cmd: u8);
}

pub struct IrRemoteController;

impl IrButtonHandler for IrRemoteController {
    fn on_ok(&mut self) {
        let _ = MOTORS_CHANNEL.try_send(MotorCommand::Stop);
        debug!("Ok button pressed");
    }
    fn on_left(&mut self) {
        let _ = MOTORS_CHANNEL.try_send(MotorCommand::Left);
        debug!("Left button pressed: activate left motor");
    }
    fn on_up(&mut self) {
        let _ = MOTORS_CHANNEL.try_send(MotorCommand::Forward);
        debug!("Up button pressed: activate forward motor");
    }
    fn on_right(&mut self) {
        let _ = MOTORS_CHANNEL.try_send(MotorCommand::Right);
        debug!("Right button pressed: activate right motor");
    }
    fn on_down(&mut self) {
        let _ = MOTORS_CHANNEL.try_send(MotorCommand::Backward);
        debug!("Down button pressed: activate backward motor");
    }
    fn on_num(&mut self, n: u8) {
        debug!("Number button {} pressed", n);
    }
    fn on_star(&mut self) {
        let _ = BIG_LEDS_CHANNEL.try_send(BigLedCommand::Toggle);
        debug!("Star button pressed");
    }
    fn on_hash(&mut self) {
        debug!("Hash button pressed");
    }
    fn on_unknown(&mut self, cmd: u8) {
        debug!("Unknown button: 0x{:02X}", cmd);
    }
}

const NEC_LEADER_LOW_MIN: u32 = 8000;
const NEC_LEADER_LOW_MAX: u32 = 10000;
const NEC_LEADER_HIGH_MIN: u32 = 4000;
const NEC_LEADER_HIGH_MAX: u32 = 5000;
const NEC_REPEAT_HIGH_MIN: u32 = 2000;
const NEC_REPEAT_HIGH_MAX: u32 = 2500;
const NEC_BIT_THRESHOLD: u32 = 1000;

pub enum IrDecodeResult {
    Button(IrButton),
    Repeat,
    None,
}

pub fn decode_nec(timings: &[u32]) -> IrDecodeResult {
    if timings.len() < 2 + 2 * 32 {
        return IrDecodeResult::None;
    }
    if timings[0] > NEC_LEADER_LOW_MIN
        && timings[0] < NEC_LEADER_LOW_MAX
        && timings[1] > NEC_LEADER_HIGH_MIN
        && timings[1] < NEC_LEADER_HIGH_MAX
    {
        let mut data: u32 = 0;
        for j in 0..32 {
            let high = *timings.get(2 + j * 2 + 1).unwrap_or(&0);
            let bit = if high > NEC_BIT_THRESHOLD { 1 } else { 0 };
            data |= bit << j;
        }
        let command = ((data >> 16) & 0xFF) as u8;
        IrDecodeResult::Button(IrButton::from_command(command))
    } else if timings[0] > NEC_LEADER_LOW_MIN
        && timings[0] < NEC_LEADER_LOW_MAX
        && timings[1] > NEC_REPEAT_HIGH_MIN
        && timings[1] < NEC_REPEAT_HIGH_MAX
    {
        IrDecodeResult::Repeat
    } else {
        IrDecodeResult::None
    }
}
