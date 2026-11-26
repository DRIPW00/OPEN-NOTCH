//! A damped spring, set up the same way as SwiftUI's springs:
//! - `response`: roughly how long one swing takes, in seconds (smaller = snappier)
//! - `damping_fraction`: 1.0 = no bounce, lower = more bounce

#[derive(Clone, Copy)]
pub struct SpringConfig {
    pub response: f32,
    pub damping_fraction: f32,
}

/// Opening: quick, with a small overshoot.
pub const EXPAND: SpringConfig = SpringConfig { response: 0.42, damping_fraction: 0.74 };
/// Closing: a touch faster, and settles without wobbling.
pub const COLLAPSE: SpringConfig = SpringConfig { response: 0.36, damping_fraction: 0.92 };
/// Settings rows sliding into their new places while you reorder.
pub const ROW: SpringConfig = SpringConfig { response: 0.3, damping_fraction: 0.82 };
/// The camera/mic bubble popping in: snappy with a small bounce...
pub const POP_IN: SpringConfig = SpringConfig { response: 0.32, damping_fraction: 0.68 };
/// ...and going away quickly.
pub const POP_OUT: SpringConfig = SpringConfig { response: 0.2, damping_fraction: 1.0 };

// Stop animating once motion is far too small to see.
const SETTLE_DISTANCE: f32 = 0.02; // logical pixels
const SETTLE_SPEED: f32 = 1.0; // logical pixels per second

pub struct Spring {
    pub value: f32,
    pub target: f32,
    velocity: f32,
    stiffness: f32,
    damping: f32,
}

impl Spring {
    pub fn new(value: f32) -> Self {
        let mut spring = Self { value, target: value, velocity: 0.0, stiffness: 0.0, damping: 0.0 };
        spring.configure(EXPAND);
        spring
    }

    /// Head for a new target. Keeps the current velocity, so changing direction
    /// mid-animation stays smooth instead of jerking.
    pub fn set_target(&mut self, target: f32, config: SpringConfig) {
        self.target = target;
        self.configure(config);
    }

    fn configure(&mut self, config: SpringConfig) {
        let omega = std::f32::consts::TAU / config.response;
        self.stiffness = omega * omega;
        self.damping = 2.0 * config.damping_fraction * omega;
    }

    /// Advance by `dt` seconds. Sub-stepped at 1 ms, so the motion is the same
    /// whatever the screen's refresh rate.
    pub fn step(&mut self, dt: f32) {
        const SUB_STEP: f32 = 1.0 / 1000.0;
        let mut remaining = dt;
        while remaining > 0.0 {
            let h = remaining.min(SUB_STEP);
            let force = -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += force * h;
            self.value += self.velocity * h;
            remaining -= h;
        }
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }

    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < SETTLE_DISTANCE && self.velocity.abs() < SETTLE_SPEED
    }
}

/// A timed slide that speeds up, then brakes smoothly into place (ease-in-out), used for
/// switching pages. Retargeted mid-slide, it continues with an ease-out so it never stalls.
pub struct Ease {
    pub value: f32,
    pub target: f32,
    from: f32,
    elapsed: f32,
    duration: f32,
    ease_in: bool,
}

impl Ease {
    pub fn new(value: f32) -> Self {
        Self { value, target: value, from: value, elapsed: 0.0, duration: 0.0, ease_in: true }
    }

    /// Slide to `target`; longer distances (wrapping around) take a little longer.
    pub fn set_target(&mut self, target: f32) {
        let moving = !self.is_settled();
        self.from = self.value;
        self.target = target;
        self.elapsed = 0.0;
        let distance = (target - self.from).abs();
        self.duration = (0.36 + 0.08 * (distance - 1.0).max(0.0)).min(0.7);
        self.ease_in = !moving;
    }

    pub fn step(&mut self, dt: f32) {
        if self.is_settled() {
            self.value = self.target;
            return;
        }
        self.elapsed += dt;
        let t = (self.elapsed / self.duration.max(0.001)).min(1.0);
        let k = if self.ease_in {
            // ease-in-out cubic: accelerate, then brake
            if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
        } else {
            1.0 - (1.0 - t).powi(3) // ease-out cubic
        };
        self.value = self.from + (self.target - self.from) * k;
    }

    pub fn is_settled(&self) -> bool {
        self.elapsed >= self.duration || (self.value - self.target).abs() < 0.0005 && self.duration == 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_reaches_target_smoothly() {
        let mut e = Ease::new(0.0);
        e.set_target(1.0);
        let mut last = 0.0;
        for _ in 0..60 {
            e.step(1.0 / 120.0);
            assert!(e.value >= last);
            last = e.value;
        }
        assert!(e.is_settled());
        assert!((e.value - 1.0).abs() < 1e-6);
    }
}
