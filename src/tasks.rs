use crate::{
    big_led::{BIG_LEDS_CHANNEL, BigLed, BigLedCommand},
    bottom_led::{BOTTOM_LEDS_CHANNEL, BottomLedCommand},
    ir_remote_control::{IrDecodeResult, IrRemoteController, decode_nec},
    motor::MOTORS_CHANNEL,
    servo::{SERVO_CHANNEL, ServoCommand, ServoDirection, position_to_duty},
    twim::{Irqs, TWIN_CHANNEL},
};
use defmt::debug;
use embassy_nrf::{
    gpio::{Input, Pull},
    peripherals::{P0_01, P0_02, P0_11, P0_26, P1_00, PWM0, PWM1, TWISPI0},
    pwm::{
        Prescaler, SequenceConfig, SequenceLoad, SequencePwm, SimplePwm, SingleSequenceMode,
        SingleSequencer,
    },
    twim::Twim,
};
use embassy_time::{Duration, Instant, Timer};

// Low-level constants for WS2812B LED control
const T1H: u16 = 0x8000 | 13; // Duty = 13/20 ticks (0.8us/1.25us) for a 1
const T0H: u16 = 0x8000 | 7; // Duty 7/20 ticks (0.4us/1.25us) for a 0
const RES: u16 = 0x8000;

// IR remote control constants
const NEC_REPEAT_HIGH_MIN: u32 = 2000;
const TIMINGS_SIZE: usize = 120;
const PULSE_TIMEOUT_US: u32 = 18000;
const SAMPLE_INTERVAL_US: u64 = 15;

// This allows the under-leds and the motors to work
#[embassy_executor::task]
pub async fn twin_task(p_twin: TWISPI0, p_i2c_ext_sda: P1_00, p_i2c_ext_scl: P0_26) {
    let config = embassy_nrf::twim::Config::default();
    let mut twi = Twim::new(p_twin, Irqs, p_i2c_ext_sda, p_i2c_ext_scl, config);
    debug!("TWIM initialized");

    loop {
        let command = TWIN_CHANNEL.receive().await;
        let buffer = [command.channel(), command.value()];
        if twi.blocking_write(0x30, &buffer).is_err() {
            debug!("I2C write failed channel={} value={}", buffer[0], buffer[1]);
        }
    }
}

#[embassy_executor::task]
pub async fn big_leds() {
    debug!("Big LEDs initialized");
    let mut big_led_state = 0x00u8;
    loop {
        match BIG_LEDS_CHANNEL.receive().await {
            BigLedCommand::Toggle => {
                // Toggle state between 0x00 and 0xFF
                big_led_state = if big_led_state == 0x00 { 0xFF } else { 0x00 };
                // Set all LEDs to the new state sequentially
                for mut led in BigLed::all_leds().into_iter() {
                    led.set_value(big_led_state).await;
                }
            }
        }
    }
}

#[embassy_executor::task]
pub async fn bottom_leds(p_pwm: PWM0, p: P0_11) {
    debug!("Bottom LEDs initialized");
    let mut config = embassy_nrf::pwm::Config::default();
    config.sequence_load = SequenceLoad::Common;
    config.prescaler = Prescaler::Div1;
    config.max_duty = 20; // 1.25us (1s / 16Mhz * 20)
    let mut pwm = SequencePwm::new_1ch(p_pwm, p, config).unwrap();

    let mut seq_words = [RES; 97];
    let mut seq_config = SequenceConfig::default();
    seq_config.end_delay = 799; // 50us (20 ticks * 40) - 1 tick because we've already got one RES;

    loop {
        match BOTTOM_LEDS_CHANNEL.receive().await {
            BottomLedCommand::AllOff => encode_bottom_leds(&mut seq_words, 0x00, 0x00, 0x00),
            BottomLedCommand::AllOn => encode_bottom_leds(&mut seq_words, 0x10, 0x00, 0x00),
        }

        let sequences = SingleSequencer::new(&mut pwm, &seq_words, seq_config.clone());
        if sequences.start(SingleSequenceMode::Times(1)).is_err() {
            debug!("Bottom LED sequence failed");
        }

        Timer::after_millis(1).await;
    }
}

fn encode_bottom_leds(seq_words: &mut [u16; 97], g: u8, r: u8, b: u8) {
    for led in 0..4 {
        let base = led * 24;
        encode_byte(g, &mut seq_words[base..base + 8]);
        encode_byte(r, &mut seq_words[base + 8..base + 16]);
        encode_byte(b, &mut seq_words[base + 16..base + 24]);
    }
    seq_words[96] = RES;
}

fn encode_byte(byte: u8, words: &mut [u16]) {
    for (bit, word) in words.iter_mut().enumerate() {
        *word = if (byte << bit) & 0x80 != 0 { T1H } else { T0H };
    }
}

#[embassy_executor::task]
pub async fn servo(p_pwm1: PWM1, p: P0_01) {
    let mut pwm = SimplePwm::new_1ch(p_pwm1, p);
    pwm.set_prescaler(Prescaler::Div128);
    pwm.set_max_duty(2500);
    pwm.set_duty(0, ServoDirection::Front.direction_to_duty());
    debug!("Servo initialized");

    loop {
        match SERVO_CHANNEL.receive().await {
            ServoCommand::SetDirection(direction) => {
                pwm.set_duty(0, direction.direction_to_duty());
            }
            ServoCommand::SetPosition(position) => {
                pwm.set_duty(0, position_to_duty(position));
            }
        }
    }
}

#[embassy_executor::task]
pub async fn motors() {
    loop {
        let command = MOTORS_CHANNEL.receive().await;
        command.execute().await;
    }
}

#[embassy_executor::task]
pub async fn ir_remote_control(p: P0_02) {
    let mut ir_pin = Input::new(p, Pull::Up);
    let mut controller = IrRemoteController;
    debug!("IR Remote Control initialized");

    loop {
        // Ensure line is idle before starting
        ir_pin.wait_for_high().await;
        ir_pin.wait_for_low().await;

        // Use a larger timings array and finer sampling
        let mut timings = [0u32; TIMINGS_SIZE];
        let mut i = 0;
        while i < timings.len() {
            let level = ir_pin.is_low();
            let start = Instant::now();
            let mut elapsed;
            loop {
                Timer::after_micros(SAMPLE_INTERVAL_US).await;
                elapsed = Instant::now() - start;
                if ir_pin.is_low() != level || elapsed.as_micros() > PULSE_TIMEOUT_US as u64 {
                    break;
                }
            }
            timings[i] = elapsed.as_micros() as u32;
            if timings[i] > PULSE_TIMEOUT_US {
                break;
            }
            i += 1;
        }

        match decode_nec(&timings[..i]) {
            IrDecodeResult::Button(button) => button.execute(&mut controller),
            IrDecodeResult::Repeat => debug!("Button held (NEC repeat code)"),
            IrDecodeResult::None => {
                if i > 10 && timings[0] > NEC_REPEAT_HIGH_MIN {
                    debug!(
                        "No valid NEC leader detected or signal too short: timings[0]={}, timings[1]={}, count={}",
                        timings[0], timings[1], i
                    );
                }
            }
        }

        Timer::after(Duration::from_millis(120)).await;
    }
}
