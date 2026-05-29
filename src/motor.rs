use crate::twim::{TWIN_CHANNEL, TwinCommand};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};

pub static MOTORS_CHANNEL: Channel<ThreadModeRawMutex, MotorCommand, 1> = Channel::new();
const ANALOG_MOTOR_DEADBAND_PERCENT: i8 = 15;

#[derive(Clone, Copy)]
pub enum MotorCommand {
    Stop,
    Forward,
    Backward,
    Left,
    Right,
    Throttle(i8),
    Drive { throttle: i8, steering: i8 },
}

impl MotorCommand {
    pub async fn execute(&self) {
        match self {
            MotorCommand::Stop => {
                for mut motor in Motor::all_motors().into_iter() {
                    motor.set_power(MotorPower::Stop).await;
                }
            }
            MotorCommand::Forward => {
                for mut motor in Motor::all_motors().into_iter() {
                    motor.set_power(MotorPower::Forward(0xFF)).await;
                }
            }
            MotorCommand::Backward => {
                for mut motor in Motor::all_motors().into_iter() {
                    motor.set_power(MotorPower::Backward(0xFF)).await;
                }
            }
            MotorCommand::Left => {
                for mut motor in Motor::left_side_motors().into_iter() {
                    motor.set_power(MotorPower::Backward(0xFF)).await;
                }
                for mut motor in Motor::right_side_motors().into_iter() {
                    motor.set_power(MotorPower::Forward(0xFF)).await;
                }
            }
            MotorCommand::Right => {
                for mut motor in Motor::left_side_motors().into_iter() {
                    motor.set_power(MotorPower::Forward(0xFF)).await;
                }
                for mut motor in Motor::right_side_motors().into_iter() {
                    motor.set_power(MotorPower::Backward(0xFF)).await;
                }
            }
            MotorCommand::Throttle(throttle) => {
                let power = motor_power_from_throttle(*throttle);
                for mut motor in Motor::all_motors().into_iter() {
                    motor.set_power(power).await;
                }
            }
            MotorCommand::Drive { throttle, steering } => {
                let left_power = motor_power_from_throttle(mix_percent(*throttle, *steering));
                let right_power = motor_power_from_throttle(mix_percent(*throttle, -*steering));
                for mut motor in Motor::left_side_motors().into_iter() {
                    motor.set_power(left_power).await;
                }
                for mut motor in Motor::right_side_motors().into_iter() {
                    motor.set_power(right_power).await;
                }
            }
        }
    }
}

fn mix_percent(throttle: i8, steering: i8) -> i8 {
    (throttle as i16 + steering as i16).clamp(-100, 100) as i8
}

fn motor_power_from_throttle(throttle: i8) -> MotorPower {
    let throttle = throttle.clamp(-100, 100);
    if throttle.abs() <= ANALOG_MOTOR_DEADBAND_PERCENT {
        MotorPower::Stop
    } else if throttle > 0 {
        MotorPower::Forward(percent_to_pwm(throttle as u8))
    } else {
        MotorPower::Backward(percent_to_pwm((-throttle) as u8))
    }
}

fn percent_to_pwm(percent: u8) -> u8 {
    ((percent as u16 * u8::MAX as u16) / 100) as u8
}

#[derive(Clone, Copy)]
enum MotorSide {
    Left,
    Right,
}

#[derive(Clone, Copy)]
enum MotorPosition {
    Front,
    Back,
}

#[derive(Clone, Copy)]
pub enum MotorPower {
    Stop,
    Forward(u8),
    Backward(u8),
}

pub struct Motor {
    side: MotorSide,
    position: MotorPosition,
    power: MotorPower,
}

impl Motor {
    pub const FRONT_RIGHT: Self =
        Self::new(MotorSide::Right, MotorPosition::Front, MotorPower::Stop);
    pub const FRONT_LEFT: Self = Self::new(MotorSide::Left, MotorPosition::Front, MotorPower::Stop);
    pub const BACK_RIGHT: Self = Self::new(MotorSide::Right, MotorPosition::Back, MotorPower::Stop);
    pub const BACK_LEFT: Self = Self::new(MotorSide::Left, MotorPosition::Back, MotorPower::Stop);

    const fn new(side: MotorSide, position: MotorPosition, power: MotorPower) -> Self {
        Self {
            side,
            position,
            power,
        }
    }

    pub const fn all_motors() -> [Self; 4] {
        [
            Self::FRONT_RIGHT,
            Self::FRONT_LEFT,
            Self::BACK_RIGHT,
            Self::BACK_LEFT,
        ]
    }

    pub const fn left_side_motors() -> [Self; 2] {
        [Self::FRONT_LEFT, Self::BACK_LEFT]
    }

    pub const fn right_side_motors() -> [Self; 2] {
        [Self::FRONT_RIGHT, Self::BACK_RIGHT]
    }

    const fn channel(&self) -> (u8, u8) {
        match (self.position, self.side) {
            (MotorPosition::Front, MotorSide::Right) => (0x01, 0x02),
            (MotorPosition::Front, MotorSide::Left) => (0x03, 0x04),
            (MotorPosition::Back, MotorSide::Right) => (0x05, 0x06),
            (MotorPosition::Back, MotorSide::Left) => (0x07, 0x08),
        }
    }

    pub async fn set_power(&mut self, power: MotorPower) {
        self.power = power;
        let ((channel0, value0), (channel1, value1)) = match power {
            MotorPower::Stop => ((self.channel().0, 0x00), (self.channel().1, 0x00)),
            MotorPower::Forward(speed) => ((self.channel().0, 0x00), (self.channel().1, speed)),
            MotorPower::Backward(speed) => ((self.channel().0, speed), (self.channel().1, 0x00)),
        };
        TWIN_CHANNEL.send(TwinCommand::new(channel0, value0)).await;
        TWIN_CHANNEL.send(TwinCommand::new(channel1, value1)).await;
    }
}
