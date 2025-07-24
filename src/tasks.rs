use crate::{
    big_led::{BIG_LEDS_CHANNEL, BigLed, BigLedCommand},
    bottom_led::BOTTOM_LEDS_CHANNEL,
    ir_remote_control::{IrButton, IrDecodeResult, IrRemoteController, decode_nec},
    motor::{MOTORS_CHANNEL, Motor, MotorPower},
    servo::ServoDirection,
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

    loop {
        let command = TWIN_CHANNEL.receive().await;
        let _ = twi.blocking_write(0x30, &mut [command.channel(), command.value()]);
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
    // Use the smart-leds crate to better integration
    let mut config = embassy_nrf::pwm::Config::default();
    config.sequence_load = SequenceLoad::Common;
    config.prescaler = Prescaler::Div1;
    config.max_duty = 20; // 1.25us (1s / 16Mhz * 20)
    let mut pwm = SequencePwm::new_1ch(p_pwm, p, config).unwrap();

    loop {
        //BOTTOM_LEDS_CHANNEL.receive().await.execute(&mut pwm).await;
    }
    // Declare the bits of 24 bits in a buffer we'll be
    // mutating later.
    // GRB
    let mut seq_words = [
        // seq_word[0-2] Front Left led
        T0H, T0H, T0H, T0H, T0H, T0H, T0H, T0H, // 0
        T0H, T0H, T0H, T0H, T0H, T0H, T0H, T0H, // 0
        T1H, T1H, T1H, T1H, T1H, T1H, T1H, T1H, // 1
        // seq_word[3-5] Front Right led
        T0H, T0H, T0H, T0H, T0H, T0H, T0H, T0H, // 0
        T0H, T0H, T0H, T0H, T0H, T0H, T0H, T0H, // 0
        T1H, T1H, T1H, T1H, T1H, T1H, T1H, T1H, // 1
        // seq_word[6-8] Back Righ led
        T0H, T0H, T0H, T0H, T0H, T0H, T0H, T0H, // 0
        T0H, T0H, T0H, T0H, T0H, T0H, T0H, T0H, // 0
        T1H, T1H, T1H, T1H, T1H, T1H, T1H, T1H, // 1
        // seq_word[9-11] Back Left led
        T0H, T0H, T0H, T0H, T0H, T0H, T0H, T0H, // 0
        T0H, T0H, T0H, T0H, T0H, T0H, T0H, T0H, // 0
        T1H, T1H, T1H, T1H, T1H, T1H, T1H, T1H, // 1
        RES,
    ];

    let mut seq_config = SequenceConfig::default();
    seq_config.end_delay = 799; // 50us (20 ticks * 40) - 1 tick because we've already got one RES;

    let mut color_bit = 16;
    let mut bit_value = T0H;

    loop {
        let sequences = SingleSequencer::new(&mut pwm, &seq_words, seq_config.clone());
        sequences.start(SingleSequenceMode::Times(1)).unwrap();

        Timer::after_millis(1000).await;

        if bit_value == T0H {
            if color_bit == 20 {
                bit_value = T1H;
            } else {
                color_bit += 1;
            }
        } else {
            if color_bit == 16 {
                bit_value = T0H;
            } else {
                color_bit -= 1;
            }
        }

        drop(sequences);

        for i in 0..4 {
            seq_words[color_bit + (i * 24)] = bit_value;
        }
    }
}

#[embassy_executor::task]
pub async fn servo(p_pwm1: PWM1, p: P0_01) {
    let mut pwm = SimplePwm::new_1ch(p_pwm1, p);
    pwm.set_prescaler(Prescaler::Div128);
    pwm.set_max_duty(2500);
    debug!("Servo initialized");

    loop {
        debug!("Servo Left");
        pwm.set_duty(0, ServoDirection::Left.direction_to_duty());
        Timer::after(Duration::from_millis(5000)).await;

        debug!("Servo Left Front");
        pwm.set_duty(0, ServoDirection::LeftFront.direction_to_duty());
        Timer::after_millis(5000).await;

        debug!("Servo Front");
        pwm.set_duty(0, ServoDirection::Front.direction_to_duty());
        Timer::after_millis(5000).await;

        debug!("Servo Right Front");
        pwm.set_duty(0, ServoDirection::RightFront.direction_to_duty());
        Timer::after_millis(5000).await;

        debug!("Servo Right");
        pwm.set_duty(0, ServoDirection::Right.direction_to_duty());
        Timer::after(Duration::from_millis(5000)).await;
    }
}

#[embassy_executor::task]
pub async fn motors() {
    loop {
        MOTORS_CHANNEL.receive().await.execute().await;
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
