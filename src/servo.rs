use defmt::info;

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
