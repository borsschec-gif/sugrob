//! Перенос `Caps.swift` и `Snow.swift`. Все размеры в тех же точках, что и на Mac,
//! поэтому константы взяты оттуда без изменений. Физические пиксели Windows
//! переводятся в точки делением на масштаб основного монитора.
mod caps;
mod folders;
mod icon;
mod pause;
mod santa;
mod santa_art;
mod snow;

pub use caps::*;
pub use folders::*;
pub use icon::*;
pub use pause::*;
pub use santa::*;
pub use santa_art::*;
pub use snow::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V2 {
    pub x: f32,
    pub y: f32,
}

pub fn v2(x: f32, y: f32) -> V2 {
    V2 { x, y }
}

impl V2 {
    pub fn len(self) -> f32 {
        self.x.hypot(self.y)
    }
    pub fn scaled(self, k: f32) -> V2 {
        v2(self.x * k, self.y * k)
    }
    pub fn plus(self, o: V2) -> V2 {
        v2(self.x + o.x, self.y + o.y)
    }
    pub fn minus(self, o: V2) -> V2 {
        v2(self.x - o.x, self.y - o.y)
    }
}

/// Прямоугольник в точках, y вниз.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn max_x(&self) -> f32 {
        self.x + self.w
    }
    pub fn max_y(&self) -> f32 {
        self.y + self.h
    }
    pub fn mid_x(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn contains(&self, p: V2) -> bool {
        p.x >= self.x && p.x < self.max_x() && p.y >= self.y && p.y < self.max_y()
    }
    pub fn same_size(&self, o: &Rect) -> bool {
        self.w == o.w && self.h == o.h
    }
}

/// SplitMix64, как `SeededGenerator` на Mac.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }
    pub fn index(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
    pub fn flip(&mut self) -> bool {
        self.next_u64() & 1 == 0
    }
}

/// Формат совпадает со структурой в шейдере.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FlakeInstance {
    pub rect: [f32; 4],
    pub alpha: f32,
    pub softness: f32,
    pub ring: f32,
    pub padding: f32,
}
