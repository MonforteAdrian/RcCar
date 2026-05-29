pub const ANALOG_OPCODE: u8 = 0x10;
pub const DRIVE_OPCODE: u8 = 0x11;
pub const ANALOG_MIN: i8 = -100;
pub const ANALOG_MAX: i8 = 100;

pub const COMMANDS: &str = "Commands:
  forward
  backward
  stop
  left
  right
  servo-left
  servo-left-front
  servo-front
  servo-right-front
  servo-right
  front-toggle
  bottom-on
  bottom-off
  drive <throttle -100..100> <steering -100..100>
  throttle <value -100..100>
  steer <value -100..100>
  drive gamepad
  drive chronos --device <path>
  chronos-probe --device <path>
  chronos-calibrate --device <path>
  status
  restart
  logs
  install

Advanced forms also work:
  motor forward
  servo left
  front-led toggle
  bottom-led on
";

pub const COMMAND_MENU: &str = "Commands:
   1  motor stop
   2  motor forward
   3  motor backward
   4  motor left
   5  motor right
   6  servo right
   7  servo right-front
   8  servo front
   9  servo left-front
  10  servo left
  11  front-led toggle
  12  bottom-led off
  13  bottom-led on

Analog commands:
  drive <throttle -100..100> <steering -100..100>
  throttle <value -100..100>
  steer <value -100..100>

Simple command aliases:
  stop, forward, backward, left, right
  servo-left, servo-front, servo-right
  front-toggle, bottom-on, bottom-off
  status";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Characteristic {
    Motor,
    Servo,
    FrontLed,
    BottomLed,
}

impl Characteristic {
    pub const fn uuid_str(self) -> &'static str {
        match self {
            Characteristic::Motor => "a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e501",
            Characteristic::Servo => "a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e502",
            Characteristic::FrontLed => "a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e503",
            Characteristic::BottomLed => "a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e504",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutoStopEffect {
    Arm,
    Disarm,
    NoChange,
}

impl AutoStopEffect {
    pub const fn merge(self, next: Self) -> Self {
        match next {
            AutoStopEffect::NoChange => self,
            _ => next,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BleWrite {
    pub characteristic: Characteristic,
    pub payload: Vec<u8>,
    pub description: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MotorAction {
    Stop,
    Forward,
    Backward,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServoPosition {
    Right,
    RightFront,
    Front,
    LeftFront,
    Left,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BottomLedAction {
    Off,
    On,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlCommand {
    Motor(MotorAction),
    Servo(ServoPosition),
    FrontLedToggle,
    BottomLed(BottomLedAction),
    Drive { throttle: i8, steering: i8 },
    Throttle(i8),
    Steering(i8),
}

impl ControlCommand {
    pub fn ble_writes(&self) -> Vec<BleWrite> {
        match *self {
            ControlCommand::Motor(action) => vec![legacy_motor_write(action)],
            ControlCommand::Servo(position) => vec![legacy_servo_write(position)],
            ControlCommand::FrontLedToggle => vec![BleWrite {
                characteristic: Characteristic::FrontLed,
                payload: vec![0x01],
                description: "front LEDs toggle".to_string(),
            }],
            ControlCommand::BottomLed(action) => vec![legacy_bottom_led_write(action)],
            ControlCommand::Drive { throttle, steering } => {
                vec![analog_drive_write(throttle, steering)]
            }
            ControlCommand::Throttle(throttle) => vec![analog_motor_write(throttle)],
            ControlCommand::Steering(steering) => vec![analog_servo_write(steering)],
        }
    }

    pub fn description(&self) -> String {
        match *self {
            ControlCommand::Motor(action) => legacy_motor_description(action).to_string(),
            ControlCommand::Servo(position) => legacy_servo_description(position).to_string(),
            ControlCommand::FrontLedToggle => "front LEDs toggle".to_string(),
            ControlCommand::BottomLed(action) => legacy_bottom_led_description(action).to_string(),
            ControlCommand::Drive { throttle, steering } => {
                format!("drive throttle {throttle} steering {steering}")
            }
            ControlCommand::Throttle(throttle) => format!("throttle {throttle}"),
            ControlCommand::Steering(steering) => format!("steering {steering}"),
        }
    }

    pub fn socket_line(&self) -> String {
        match *self {
            ControlCommand::Motor(action) => legacy_motor_description(action).to_string(),
            ControlCommand::Servo(position) => legacy_servo_description(position).to_string(),
            ControlCommand::FrontLedToggle => "front-toggle".to_string(),
            ControlCommand::BottomLed(BottomLedAction::Off) => "bottom-off".to_string(),
            ControlCommand::BottomLed(BottomLedAction::On) => "bottom-on".to_string(),
            ControlCommand::Drive { throttle, steering } => format!("drive {throttle} {steering}"),
            ControlCommand::Throttle(throttle) => format!("throttle {throttle}"),
            ControlCommand::Steering(steering) => format!("steer {steering}"),
        }
    }

    pub const fn auto_stop_effect(&self) -> AutoStopEffect {
        match *self {
            ControlCommand::Motor(MotorAction::Stop) => AutoStopEffect::Disarm,
            ControlCommand::Motor(_) => AutoStopEffect::Arm,
            ControlCommand::Drive {
                throttle: 0,
                steering: 0,
            }
            | ControlCommand::Throttle(0) => AutoStopEffect::Disarm,
            ControlCommand::Drive { .. } | ControlCommand::Throttle(_) => AutoStopEffect::Arm,
            ControlCommand::Servo(_)
            | ControlCommand::FrontLedToggle
            | ControlCommand::BottomLed(_)
            | ControlCommand::Steering(_) => AutoStopEffect::NoChange,
        }
    }
}

pub fn parse_control_command(line: &str) -> Result<ControlCommand, String> {
    match line.trim() {
        "1" => return Ok(ControlCommand::Motor(MotorAction::Stop)),
        "2" => return Ok(ControlCommand::Motor(MotorAction::Forward)),
        "3" => return Ok(ControlCommand::Motor(MotorAction::Backward)),
        "4" => return Ok(ControlCommand::Motor(MotorAction::Left)),
        "5" => return Ok(ControlCommand::Motor(MotorAction::Right)),
        "6" => return Ok(ControlCommand::Servo(ServoPosition::Right)),
        "7" => return Ok(ControlCommand::Servo(ServoPosition::RightFront)),
        "8" => return Ok(ControlCommand::Servo(ServoPosition::Front)),
        "9" => return Ok(ControlCommand::Servo(ServoPosition::LeftFront)),
        "10" => return Ok(ControlCommand::Servo(ServoPosition::Left)),
        "11" => return Ok(ControlCommand::FrontLedToggle),
        "12" => return Ok(ControlCommand::BottomLed(BottomLedAction::Off)),
        "13" => return Ok(ControlCommand::BottomLed(BottomLedAction::On)),
        "stop" => return Ok(ControlCommand::Motor(MotorAction::Stop)),
        "forward" => return Ok(ControlCommand::Motor(MotorAction::Forward)),
        "backward" | "reverse" => return Ok(ControlCommand::Motor(MotorAction::Backward)),
        "left" => return Ok(ControlCommand::Motor(MotorAction::Left)),
        "right" => return Ok(ControlCommand::Motor(MotorAction::Right)),
        "servo-right" => return Ok(ControlCommand::Servo(ServoPosition::Right)),
        "servo-right-front" => return Ok(ControlCommand::Servo(ServoPosition::RightFront)),
        "servo-front" | "servo-center" | "center" => {
            return Ok(ControlCommand::Servo(ServoPosition::Front));
        }
        "servo-left-front" => return Ok(ControlCommand::Servo(ServoPosition::LeftFront)),
        "servo-left" => return Ok(ControlCommand::Servo(ServoPosition::Left)),
        "front-toggle" | "front-led-toggle" => return Ok(ControlCommand::FrontLedToggle),
        "bottom-off" | "bottom-led-off" => {
            return Ok(ControlCommand::BottomLed(BottomLedAction::Off));
        }
        "bottom-on" | "bottom-green" | "bottom-led-on" => {
            return Ok(ControlCommand::BottomLed(BottomLedAction::On));
        }
        "" => return Err("missing command".to_string()),
        _ => {}
    }

    let parts = line.split_whitespace().collect::<Vec<_>>();
    match parts.as_slice() {
        ["drive", throttle, steering] => Ok(ControlCommand::Drive {
            throttle: parse_percent(throttle)?,
            steering: parse_percent(steering)?,
        }),
        ["throttle", value] => Ok(ControlCommand::Throttle(parse_percent(value)?)),
        ["steer" | "steering", value] => Ok(ControlCommand::Steering(parse_percent(value)?)),
        ["motor", action] => parse_motor_action(action).map(ControlCommand::Motor),
        ["motor", "throttle", value] => Ok(ControlCommand::Throttle(parse_percent(value)?)),
        ["servo", action] => parse_servo_position(action).map(ControlCommand::Servo),
        ["servo", "position", value] => Ok(ControlCommand::Steering(parse_percent(value)?)),
        ["front-led" | "front-leds", "toggle"] => Ok(ControlCommand::FrontLedToggle),
        ["bottom-led" | "bottom-leds", action] => {
            parse_bottom_led_action(action).map(ControlCommand::BottomLed)
        }
        [_] => Err("unknown command".to_string()),
        _ => Err("too many words".to_string()),
    }
}

pub fn parse_percent(value: &str) -> Result<i8, String> {
    let parsed = value
        .parse::<i16>()
        .map_err(|_| format!("invalid analog value: {value}"))?;

    if !(ANALOG_MIN as i16..=ANALOG_MAX as i16).contains(&parsed) {
        return Err(format!(
            "analog value {parsed} is out of range {ANALOG_MIN}..{ANALOG_MAX}"
        ));
    }

    Ok(parsed as i8)
}

pub fn axis_value_to_percent(value: f32, deadzone_percent: u8, invert: bool) -> i8 {
    let mut value = value.clamp(-1.0, 1.0);
    if invert {
        value = -value;
    }

    let deadzone = (deadzone_percent.min(99) as f32) / 100.0;
    let magnitude = value.abs();
    if magnitude <= deadzone {
        return 0;
    }

    let scaled = (magnitude - deadzone) / (1.0 - deadzone);
    (scaled * value.signum() * ANALOG_MAX as f32).round() as i8
}

pub fn parse_receiver_text_event(line: &str) -> Result<Option<ControlCommand>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }

    if let Ok(command) = parse_control_command(line) {
        return Ok(Some(command));
    }

    let normalized = line.replace(',', " ");
    let parts = normalized.split_whitespace().collect::<Vec<_>>();
    match parts.as_slice() {
        [throttle, steering] => Ok(Some(ControlCommand::Drive {
            throttle: parse_percent(throttle)?,
            steering: parse_percent(steering)?,
        })),
        _ => Err("unknown receiver event".to_string()),
    }
}

fn legacy_motor_write(action: MotorAction) -> BleWrite {
    let value = match action {
        MotorAction::Stop => 0x00,
        MotorAction::Forward => 0x01,
        MotorAction::Backward => 0x02,
        MotorAction::Left => 0x03,
        MotorAction::Right => 0x04,
    };

    BleWrite {
        characteristic: Characteristic::Motor,
        payload: vec![value],
        description: legacy_motor_description(action).to_string(),
    }
}

fn analog_motor_write(throttle: i8) -> BleWrite {
    BleWrite {
        characteristic: Characteristic::Motor,
        payload: vec![ANALOG_OPCODE, throttle as u8],
        description: format!("throttle {throttle}"),
    }
}

fn analog_drive_write(throttle: i8, steering: i8) -> BleWrite {
    BleWrite {
        characteristic: Characteristic::Motor,
        payload: vec![DRIVE_OPCODE, throttle as u8, steering as u8],
        description: format!("drive throttle {throttle} steering {steering}"),
    }
}

fn legacy_motor_description(action: MotorAction) -> &'static str {
    match action {
        MotorAction::Stop => "motor stop",
        MotorAction::Forward => "motor forward",
        MotorAction::Backward => "motor backward",
        MotorAction::Left => "motor left",
        MotorAction::Right => "motor right",
    }
}

fn legacy_servo_write(position: ServoPosition) -> BleWrite {
    let value = match position {
        ServoPosition::Right => 0x00,
        ServoPosition::RightFront => 0x01,
        ServoPosition::Front => 0x02,
        ServoPosition::LeftFront => 0x03,
        ServoPosition::Left => 0x04,
    };

    BleWrite {
        characteristic: Characteristic::Servo,
        payload: vec![value],
        description: legacy_servo_description(position).to_string(),
    }
}

fn analog_servo_write(steering: i8) -> BleWrite {
    BleWrite {
        characteristic: Characteristic::Servo,
        payload: vec![ANALOG_OPCODE, steering as u8],
        description: format!("steering {steering}"),
    }
}

fn legacy_servo_description(position: ServoPosition) -> &'static str {
    match position {
        ServoPosition::Right => "servo right",
        ServoPosition::RightFront => "servo right-front",
        ServoPosition::Front => "servo front",
        ServoPosition::LeftFront => "servo left-front",
        ServoPosition::Left => "servo left",
    }
}

fn legacy_bottom_led_write(action: BottomLedAction) -> BleWrite {
    let value = match action {
        BottomLedAction::Off => 0x00,
        BottomLedAction::On => 0x01,
    };

    BleWrite {
        characteristic: Characteristic::BottomLed,
        payload: vec![value],
        description: legacy_bottom_led_description(action).to_string(),
    }
}

fn legacy_bottom_led_description(action: BottomLedAction) -> &'static str {
    match action {
        BottomLedAction::Off => "bottom LEDs off",
        BottomLedAction::On => "bottom LEDs on",
    }
}

fn parse_motor_action(action: &str) -> Result<MotorAction, String> {
    match action {
        "stop" => Ok(MotorAction::Stop),
        "forward" => Ok(MotorAction::Forward),
        "backward" | "reverse" => Ok(MotorAction::Backward),
        "left" => Ok(MotorAction::Left),
        "right" => Ok(MotorAction::Right),
        _ => Err("unknown motor command".to_string()),
    }
}

fn parse_servo_position(action: &str) -> Result<ServoPosition, String> {
    match action {
        "right" => Ok(ServoPosition::Right),
        "right-front" => Ok(ServoPosition::RightFront),
        "front" | "center" => Ok(ServoPosition::Front),
        "left-front" => Ok(ServoPosition::LeftFront),
        "left" => Ok(ServoPosition::Left),
        _ => Err("unknown servo command".to_string()),
    }
}

fn parse_bottom_led_action(action: &str) -> Result<BottomLedAction, String> {
    match action {
        "off" => Ok(BottomLedAction::Off),
        "on" | "green" => Ok(BottomLedAction::On),
        _ => Err("unknown bottom LED command".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_aliases() {
        assert_eq!(
            parse_control_command("forward").unwrap(),
            ControlCommand::Motor(MotorAction::Forward)
        );
        assert_eq!(
            parse_control_command("servo-center").unwrap(),
            ControlCommand::Servo(ServoPosition::Front)
        );
        assert_eq!(
            parse_control_command("bottom-green").unwrap(),
            ControlCommand::BottomLed(BottomLedAction::On)
        );
    }

    #[test]
    fn parses_advanced_legacy_forms() {
        assert_eq!(
            parse_control_command("motor reverse").unwrap(),
            ControlCommand::Motor(MotorAction::Backward)
        );
        assert_eq!(
            parse_control_command("servo left-front").unwrap(),
            ControlCommand::Servo(ServoPosition::LeftFront)
        );
        assert_eq!(
            parse_control_command("front-leds toggle").unwrap(),
            ControlCommand::FrontLedToggle
        );
    }

    #[test]
    fn parses_analog_drive_commands() {
        let command = parse_control_command("drive 45 -20").unwrap();
        assert_eq!(
            command,
            ControlCommand::Drive {
                throttle: 45,
                steering: -20
            }
        );
        assert_eq!(command.auto_stop_effect(), AutoStopEffect::Arm);

        let writes = command.ble_writes();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].characteristic, Characteristic::Motor);
        assert_eq!(writes[0].payload, vec![DRIVE_OPCODE, 45, (-20i8) as u8]);
    }

    #[test]
    fn only_fully_neutral_drive_disarms_auto_stop() {
        let command = parse_control_command("drive 0 25").unwrap();
        assert_eq!(command.auto_stop_effect(), AutoStopEffect::Arm);

        let command = parse_control_command("drive 0 0").unwrap();
        assert_eq!(command.auto_stop_effect(), AutoStopEffect::Disarm);
    }

    #[test]
    fn rejects_out_of_range_analog_values() {
        assert!(parse_control_command("drive 101 0").is_err());
        assert!(parse_control_command("drive 0 -101").is_err());
        assert!(parse_control_command("throttle nope").is_err());
    }

    #[test]
    fn rejects_malformed_commands() {
        assert_eq!(
            parse_control_command("drive 1 2 3").unwrap_err(),
            "too many words"
        );
        assert_eq!(parse_control_command("").unwrap_err(), "missing command");
    }

    #[test]
    fn maps_axes_with_deadzone() {
        assert_eq!(axis_value_to_percent(0.04, 5, false), 0);
        assert_eq!(axis_value_to_percent(1.0, 5, false), 100);
        assert_eq!(axis_value_to_percent(-1.0, 5, false), -100);
        assert_eq!(axis_value_to_percent(1.0, 5, true), -100);
    }

    #[test]
    fn parses_receiver_text_events() {
        assert_eq!(
            parse_receiver_text_event("25,-30").unwrap(),
            Some(ControlCommand::Drive {
                throttle: 25,
                steering: -30
            })
        );
        assert_eq!(
            parse_receiver_text_event("front-toggle").unwrap(),
            Some(ControlCommand::FrontLedToggle)
        );
        assert_eq!(parse_receiver_text_event("# comment").unwrap(), None);
    }
}
