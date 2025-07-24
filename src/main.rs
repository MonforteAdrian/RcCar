#![no_std]
#![no_main]

use embassy_executor::Spawner;

mod tasks;
use tasks::*;
mod big_led;
mod bottom_led;
mod ir_remote_control;
mod motor;
mod servo;
mod twim;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Starting...");
    let p = embassy_nrf::init(Default::default());

    // Communication for Big Leds and Motors
    spawner.must_spawn(twin_task(p.TWISPI0, p.P1_00, p.P0_26));

    // Two Big Leds in the front
    spawner.must_spawn(big_leds());

    // Four small ws2812B LEDs at the bottom
    // TODO Finish it, you lazy!
    spawner.must_spawn(bottom_leds(p.PWM0, p.P0_11));

    // Servo for the head of the car
    spawner.must_spawn(servo(p.PWM1, p.P0_01));

    // Motors( Really!)
    spawner.must_spawn(motors());

    // Infrared remote controller
    spawner.must_spawn(ir_remote_control(p.P0_02));

    // TODO Line tracking sensor

    // TODO Ultrasonic sensor

    // TODO Bluetooth remote controller

    // TODO Extra projects from the microbit sensors?
}
