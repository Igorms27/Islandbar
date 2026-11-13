//! Animação por mola (spring): é o que dá o efeito "elástico" da ilha.

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub target: f32,
    velocity: f32,
    stiffness: f32,
    damping: f32,
}

impl Spring {
    /// Mola com um leve quique no final (boa para tamanho/posição).
    pub fn bouncy(value: f32) -> Self {
        Self { value, target: value, velocity: 0.0, stiffness: 240.0, damping: 19.0 }
    }

    /// Mola sem quique (boa para opacidade).
    pub fn smooth(value: f32) -> Self {
        Self { value, target: value, velocity: 0.0, stiffness: 300.0, damping: 34.0 }
    }

    pub fn update(&mut self, dt: f32) {
        // Passos pequenos mantêm a simulação estável mesmo se um frame atrasar.
        let steps = (dt / 0.004).ceil().max(1.0);
        let h = dt / steps;
        for _ in 0..steps as usize {
            let accel = self.stiffness * (self.target - self.value) - self.damping * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
        }
        if self.settled() {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }

    pub fn settled(&self) -> bool {
        (self.target - self.value).abs() < 0.01 && self.velocity.abs() < 0.05
    }
}

/// Aproxima `current` de `target`: sobe rápido e desce devagar, como um VU.
pub fn meter(current: f32, target: f32, dt: f32) -> f32 {
    let rate = if target > current { 30.0 } else { 6.0 };
    current + (target - current) * (1.0 - (-rate * dt).exp())
}

pub fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}
