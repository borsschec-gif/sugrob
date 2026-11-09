use super::{CapField, FlakeInstance, Rng};

struct DepthLayer {
    area_per_flake: f32,
    radius: (f32, f32),
    fall_speed: (f32, f32),
    alpha: (f32, f32),
    sway_amplitude: (f32, f32),
    parallax: f32,
    softness: f32,
    ring: f32,
    /// Сколько снега добавляет упавший хлопок. 0: пролетает мимо окон.
    deposit: f32,
}

const LAYERS: [DepthLayer; 3] = [
    // Дальний план: мелкая пыль.
    DepthLayer { area_per_flake: 9_000.0, radius: (1.3, 2.0), fall_speed: (16.0, 26.0), alpha: (0.4, 0.65),
        sway_amplitude: (3.0, 7.0), parallax: 0.5, softness: 1.0, ring: 0.0, deposit: 0.9 },
    // Средний план: основные хлопья.
    DepthLayer { area_per_flake: 8_000.0, radius: (2.6, 4.2), fall_speed: (30.0, 48.0), alpha: (0.8, 1.0),
        sway_amplitude: (6.0, 16.0), parallax: 1.0, softness: 1.0, ring: 0.0, deposit: 1.8 },
    // У камеры: редкие крупные размытые хлопья.
    DepthLayer { area_per_flake: 90_000.0, radius: (7.0, 13.0), fall_speed: (62.0, 95.0), alpha: (0.2, 0.34),
        sway_amplitude: (10.0, 24.0), parallax: 1.7, softness: 1.0, ring: 0.2, deposit: 0.0 },
];

const MAX_DENSITY: f32 = 1.5;
const MARGIN: f32 = 24.0;

#[derive(Clone, Copy)]
struct Flake {
    x: f32,
    y: f32,
    radius: f32,
    fall_speed: f32,
    alpha: f32,
    phase: f32,
    sway_amplitude: f32,
    sway_rate: f32,
    layer: usize,
    /// Хлопок виден, если его ранг не выше текущей густоты.
    rank: f32,
}

/// Снегопад одного экрана в его локальных точках, y вниз.
pub struct SnowField {
    flakes: Vec<Flake>,
    rng: Rng,
    width: f32,
    height: f32,
    /// Густота: 0,5 редкий, 1 обычный, 1,5 густой снег.
    pub density: f32,
}

fn wind(time: f32) -> f32 {
    let drift = 8.0 + 12.0 * (time * 0.11).sin() + 6.0 * (time * 0.27 + 1.3).sin();
    let gust_shape = (time * 0.047 + 0.7).sin().max(0.0).powi(12);
    let gust_direction = if (time * 0.013).sin() >= 0.0 { 1.0 } else { -1.0 };
    drift + gust_shape * gust_direction * 90.0
}

impl SnowField {
    pub fn new(width: f32, height: f32, seed: u64) -> Self {
        let mut field = SnowField { flakes: Vec::new(), rng: Rng(seed), width, height, density: 1.0 };
        let area = width * height;
        for (index, layer) in LAYERS.iter().enumerate() {
            for _ in 0..((area * MAX_DENSITY / layer.area_per_flake) as usize).max(1) {
                let mut flake = field.make_flake(index);
                flake.y = field.rng.range(-MARGIN, height + MARGIN);
                field.flakes.push(flake);
            }
        }
        field
    }

    pub fn instance_count(&self) -> usize {
        self.flakes.len()
    }

    fn make_flake(&mut self, index: usize) -> Flake {
        let layer = &LAYERS[index];
        let rng = &mut self.rng;
        Flake {
            x: rng.range(-MARGIN, self.width + MARGIN),
            y: -MARGIN,
            radius: rng.range(layer.radius.0, layer.radius.1),
            fall_speed: rng.range(layer.fall_speed.0, layer.fall_speed.1),
            alpha: rng.range(layer.alpha.0, layer.alpha.1),
            phase: rng.range(0.0, std::f32::consts::TAU),
            sway_amplitude: rng.range(layer.sway_amplitude.0, layer.sway_amplitude.1),
            sway_rate: rng.range(2.0, 5.5),
            layer: index,
            rank: rng.unit(),
        }
    }

    /// `origin` — левый верхний угол экрана в общих координатах.
    pub fn step(&mut self, dt: f32, time: f32, caps: &mut CapField, origin: super::V2, now: f64) {
        let wind = wind(time);
        let visible_rank = self.density / MAX_DENSITY;
        for index in 0..self.flakes.len() {
            let mut flake = self.flakes[index];
            if flake.rank > visible_rank {
                continue;
            }
            let layer = &LAYERS[flake.layer];
            let sway = (time * flake.sway_rate + flake.phase).cos() * flake.sway_amplitude;
            let previous_y = flake.y;
            flake.x += (wind * layer.parallax + sway) * dt;
            flake.y += flake.fall_speed * dt;

            let landed = layer.deposit > 0.0
                && caps.try_land(flake.x + origin.x, previous_y + origin.y, flake.y + origin.y, layer.deposit, now);
            if landed || flake.y > self.height + MARGIN {
                // Ранг постоянный, иначе видимых хлопьев становилось бы всё меньше.
                let rank = flake.rank;
                flake = self.make_flake(flake.layer);
                flake.rank = rank;
            } else if flake.x > self.width + MARGIN {
                flake.x = -MARGIN;
            } else if flake.x < -MARGIN {
                flake.x = self.width + MARGIN;
            }
            self.flakes[index] = flake;
        }
    }

    pub fn write_instances(&self, out: &mut Vec<FlakeInstance>, time: f32) {
        let visible_rank = self.density / MAX_DENSITY;
        for flake in self.flakes.iter().filter(|f| f.rank <= visible_rank) {
            let layer = &LAYERS[flake.layer];
            let twinkle = 0.85 + 0.15 * (time * 1.7 + flake.phase * 3.0).sin();
            out.push(FlakeInstance {
                rect: [flake.x - flake.radius, flake.y - flake.radius, flake.radius * 2.0, flake.radius * 2.0],
                alpha: flake.alpha * twinkle,
                softness: layer.softness,
                ring: layer.ring,
                padding: 0.0,
            });
        }
    }
}
