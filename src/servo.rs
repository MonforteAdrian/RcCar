use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};

pub static SERVO_CHANNEL: Channel<ThreadModeRawMutex, ServoCommand, 1> = Channel::new();

#[derive(Clone, Copy)]
pub enum ServoCommand {
    SetDirection(ServoDirection),
    SetPosition(i8),
}

#[derive(Clone, Copy)]
pub enum ServoDirection {
    Right,
    RightFront,
    Front,
    LeftFront,
    Left,
}

impl ServoDirection {
    pub fn direction_to_duty(&self) -> u16 {
        2500 - match self {
            ServoDirection::Right => 0.55 / 0.008,     // 0 deg
            ServoDirection::RightFront => 1.0 / 0.008, // 45 deg
            ServoDirection::Front => 1.5 / 0.008,      // 90 deg
            ServoDirection::LeftFront => 2.0 / 0.008,  // 135 deg
            ServoDirection::Left => 2.45 / 0.008,      // 180 deg
        } as u16
    }
}

pub fn position_to_duty(position: i8) -> u16 {
    let position = position.clamp(-100, 100);
    let left = ServoDirection::Left.direction_to_duty();
    let front = ServoDirection::Front.direction_to_duty();
    let right = ServoDirection::Right.direction_to_duty();

    if position < 0 {
        front - ((front - left) * (-position) as u16 / 100)
    } else {
        front + ((right - front) * position as u16 / 100)
    }
}
