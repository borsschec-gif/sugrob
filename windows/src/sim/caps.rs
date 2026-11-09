use std::collections::{HashMap, VecDeque};

use super::{v2, FlakeInstance, Rect, Rng, V2};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CapVertex {
    /// x, y в точках экрана; глубина от поверхности снега; координата вдоль шапки.
    pub position: [f32; 4],
    pub thickness: f32,
}

pub struct BallDraw {
    pub center: V2,
    pub radius: f32,
    pub rotation: f32,
    pub owner: u64,
}

/// Одно окно в порядке отрисовки: стереть его прямоугольник и нарисовать его шапку.
#[derive(Clone, Copy)]
pub struct CapDraw {
    pub rect: [f32; 4],
    pub vertex_start: usize,
    pub vertex_count: usize,
    pub window: u64,
    pub corner_radius: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackedWindow {
    pub id: u64,
    pub frame: Rect,
    /// Процесс-владелец: вкладка окна узнаётся по нему и по той же рамке.
    pub owner: u32,
    /// Обычное окно приложения. Остальные только перекрывают снег.
    pub holds_snow: bool,
    /// Панель задач, замена Dock.
    pub is_dock: bool,
    /// Панель автоскрылась за нижний край своего монитора.
    pub is_dock_hidden: bool,
    pub is_desktop_icon: bool,
    /// Радиус углов, который сообщила система.
    pub corner_radius: Option<f32>,
    pub top_profile: Option<Vec<f32>>,
}

/// Скрытая панель задач оставляет на краю своего монитора полоску в пару точек.
pub fn dock_is_hidden(top: f32, monitor_bottom: f32) -> bool {
    top >= monitor_bottom - 3.0
}

impl TrackedWindow {
    fn is_fixed_surface(&self) -> bool {
        self.is_dock || self.is_desktop_icon
    }
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub timestamp: f64,
    /// От переднего окна к заднему.
    pub windows: Vec<TrackedWindow>,
    pub is_mouse_down: bool,
    pub cursor: V2,
    /// Нижний край всех мониторов: ниже снежку падать некуда.
    pub screen_bottom: f32,
}

#[derive(Clone, Copy)]
struct SnowDebris {
    position: V2,
    velocity: V2,
    radius: f32,
    alpha: f32,
    life: f32,
    gravity: f32,
    /// Окно, с которого сорвалась частица: обратно на него она не садится.
    source: u64,
}

#[derive(Clone, Copy)]
struct SnowBall {
    position: V2,
    velocity: V2,
    radius: f32,
    rotation: f32,
    window: Option<u64>,
    dropped_from: u64,
}

const STEP: f32 = 2.0;

type Visible<'a> = &'a dyn Fn(V2) -> bool;

fn debris(position: V2, velocity: V2, radius: f32, alpha: f32, life: f32, gravity: f32) -> SnowDebris {
    SnowDebris { position, velocity, radius, alpha, life, gravity, source: 0 }
}

fn stochastic_count(share: f32, rng: &mut Rng) -> usize {
    share as usize + usize::from(rng.unit() < share - share.floor())
}

/// Карта высот снега вдоль верхнего края окна.
struct SnowCap {
    corner_radius: Option<f32>,
    height_limit: f32,
    surface_profile: Option<Vec<f32>>,
    corner_wrap_start: f32,
    heights: Vec<f32>,
    detail: Vec<f32>,
    detail_offset: i32,
    left_carry: f32,
    width: f32,
    phases: V2,
    active_until: f64,
    velocity: V2,
    trail: VecDeque<(V2, f64)>,
    last_seen: f64,
}

fn sample_count(width: f32) -> usize {
    ((width / STEP) as usize + 1).max(2)
}

impl SnowCap {
    fn new(width: f32, phases: V2, now: f64) -> Self {
        let mut cap = SnowCap {
            corner_radius: None,
            height_limit: f32::INFINITY,
            surface_profile: None,
            corner_wrap_start: 0.05,
            heights: vec![0.0; sample_count(width)],
            detail: Vec::new(),
            detail_offset: 0,
            left_carry: 0.0,
            width,
            phases,
            active_until: 0.0,
            velocity: V2::default(),
            trail: VecDeque::new(),
            last_seen: now,
        };
        cap.rebuild_detail();
        cap
    }

    fn snow_start(&self) -> f32 {
        match self.corner_radius {
            None => 16.0,
            Some(r) if r < 6.0 => r * 0.36,
            Some(r) => r * self.corner_wrap_start,
        }
    }

    fn snow_taper(&self) -> f32 {
        match self.corner_radius {
            None => 10.0,
            Some(r) if r < 6.0 => 6.0,
            Some(r) => r * 0.6,
        }
    }

    fn limit_height(&mut self, limit: f32) {
        self.height_limit = limit;
        for h in &mut self.heights {
            *h = h.min(limit);
        }
    }

    fn rebuild_detail(&mut self) {
        let (offset, phases) = (self.detail_offset, self.phases);
        self.detail = (0..self.heights.len() as i32)
            .map(|index| {
                let u = (index + offset) as f32 * STEP;
                0.7 * (u * 0.13 + phases.x).sin() + 0.4 * (u * 0.31 + phases.y).sin()
            })
            .collect();
    }

    /// Начальный слой для окон, которые уже были открыты при запуске.
    fn seed(&mut self, rng: &mut Rng) {
        let base = rng.range(7.0, 10.0);
        let wave1 = rng.range(0.02, 0.05);
        let wave2 = rng.range(0.06, 0.11);
        let phase1 = rng.range(0.0, 6.28);
        let phase2 = rng.range(0.0, 6.28);
        for (index, h) in self.heights.iter_mut().enumerate() {
            let u = index as f32 * STEP;
            *h = (base + 3.0 * (u * wave1 + phase1).sin() + 1.5 * (u * wave2 + phase2).sin()).max(0.0);
        }
    }

    /// Шапка привязана к экранным координатам: с уходящей стороны снег ссыпается,
    /// на открывшийся участок ложится тонкий слой.
    fn reshape(&mut self, old: Rect, new: Rect, out: &mut Vec<SnowDebris>, limit: usize, rng: &mut Rng, visible: Visible) {
        let old_surface: Vec<f32> = (0..self.heights.len()).map(|i| self.surface_height(i)).collect();

        let left_shift = new.x - old.x + self.left_carry;
        let left = (left_shift / STEP).round() as i32;
        self.left_carry = left_shift - left as f32 * STEP;
        if left > 0 {
            let n = (left as usize).min(self.heights.len() - 2);
            self.heights.drain(..n);
        } else if left < 0 {
            let zeros = vec![0.0; (-left) as usize];
            self.heights.splice(0..0, zeros);
        }
        self.detail_offset += left;

        let right_growth = sample_count(new.w) as i32 - self.heights.len() as i32;
        if right_growth < 0 {
            let n = ((-right_growth) as usize).min(self.heights.len() - 2);
            self.heights.truncate(self.heights.len() - n);
        } else if right_growth > 0 {
            self.heights.extend(std::iter::repeat(0.0).take(right_growth as usize));
        }
        self.width = new.w;
        self.rebuild_detail();

        // Снег, ушедший под скрытую зону у края, ссыпался и обратно не вернётся.
        let count = self.heights.len();
        let hidden = ((self.snow_start() / STEP) as usize).min(count);
        let (left_shrank, right_shrank) = (new.x > old.x, new.max_x() < old.max_x());
        let (left_grew, right_grew) = (new.x < old.x, new.max_x() > old.max_x());
        if left_shrank {
            self.heights[..hidden].fill(0.0);
        }
        if right_shrank {
            self.heights[count - hidden..].fill(0.0);
        }

        // При расширении новый участок присыпается из общего сугроба: объём снега не растёт,
        // вся шапка становится тоньше.
        let total: f32 = self.heights.iter().sum();
        let fill = 0.7 * total / count as f32;
        if fill > 0.3 && (left_grew || right_grew) {
            if left_grew {
                self.raise(0, count.min(hidden + (-left).max(0) as usize), fill);
            }
            if right_grew {
                let from = count.saturating_sub(right_growth.max(0) as usize + hidden);
                self.raise(from, count, fill);
            }
            let spread = total / self.heights.iter().sum::<f32>();
            for h in &mut self.heights {
                *h *= spread;
            }
        }

        for (index, before) in old_surface.into_iter().enumerate() {
            let x = old.x + index as f32 * STEP;
            let is_left_half = x < new.mid_x();
            if !(if is_left_half { left_shrank } else { right_shrank }) {
                continue;
            }
            let new_index = ((x - new.x) / STEP).round() as i32;
            let after = if new_index >= 0 && (new_index as usize) < count { self.surface_height(new_index as usize) } else { 0.0 };
            let lost = before - after;
            if lost <= 0.25 || !visible(v2(x, old.y - before / 2.0)) {
                continue;
            }
            let direction = if is_left_half { -1.0 } else { 1.0 };
            for _ in 0..stochastic_count(lost / 2.5, rng) {
                if out.len() >= limit {
                    break;
                }
                out.push(debris(
                    v2(x, old.y - rng.range(after, before)),
                    v2(direction * rng.range(0.0, 70.0), rng.range(-30.0, 30.0)),
                    rng.range(1.8, 4.6),
                    rng.range(0.75, 1.0),
                    rng.range(0.9, 1.6),
                    900.0,
                ));
            }
        }
    }

    fn raise(&mut self, from: usize, to: usize, level: f32) {
        for index in from..to {
            let wave = 0.75 + 0.25 * ((index as i32 + self.detail_offset) as f32 * 0.09 + self.phases.x).sin();
            self.heights[index] = level * wave;
        }
    }

    /// Четверть окружности: конец шапки круглый, без острого шипа.
    fn envelope(&self, u: f32) -> f32 {
        let edge = u.min(self.width - u);
        let t = ((edge - self.snow_start()) / self.snow_taper()).clamp(0.0, 1.0);
        (1.0 - (1.0 - t) * (1.0 - t)).sqrt()
    }

    /// Куда откладывается толщина снега: на дуге угла снег обтекает окно по нормали.
    fn surface_normal(&self, u: f32) -> V2 {
        let Some(radius) = self.corner_radius.filter(|r| *r >= 6.0 && self.surface_profile.is_none()) else {
            return v2(0.0, 1.0);
        };
        let edge = u.min(self.width - u).max(0.0);
        if edge >= radius {
            return v2(0.0, 1.0);
        }
        let sine = (radius - edge) / radius;
        v2(if u < self.width / 2.0 { -sine } else { sine }, (1.0 - sine * sine).sqrt())
    }

    fn can_hold_snow(&self, u: f32) -> bool {
        u.min(self.width - u) > self.snow_start()
    }

    /// На сколько край окна опускается в этой точке из-за скругления угла.
    fn base_drop(&self, u: f32) -> f32 {
        if let Some(profile) = self.surface_profile.as_ref().filter(|p| !p.is_empty()) {
            return profile[(u.max(0.0) as usize).min(profile.len() - 1)];
        }
        let Some(radius) = self.corner_radius else { return 0.0 };
        let edge = u.min(self.width - u).max(0.0);
        if edge >= radius {
            return 0.0;
        }
        let offset = radius - edge;
        radius - (radius * radius - offset * offset).sqrt()
    }

    /// Снег срывается частицами с видимых участков; вся шапка обнуляется.
    fn shed(&mut self, origin: V2, direction: f32, fling: Option<V2>, out: &mut Vec<SnowDebris>, limit: usize, rng: &mut Rng, visible: Visible) {
        for index in (0..self.heights.len()).step_by(2) {
            let u = index as f32 * STEP;
            let height = self.surface_height(index);
            if height <= 0.6 {
                continue;
            }
            let base_y = origin.y + self.base_drop(u);
            if !visible(v2(origin.x + u, base_y - height / 2.0)) {
                continue;
            }
            for _ in 0..1 + (height / 5.0) as usize {
                if out.len() >= limit {
                    break;
                }
                let position = v2(origin.x + u + rng.range(-2.0, 2.0), base_y - rng.range(0.0, height));
                let radius = rng.range(1.8, 4.6);
                let alpha = rng.range(0.75, 1.0);
                if let Some(fling) = fling {
                    // Снег тяжёлый: срывается комьями и почти сразу падает.
                    let velocity = fling.scaled(rng.range(0.08, 0.2)).plus(v2(rng.range(-60.0, 60.0), rng.range(-60.0, 40.0)));
                    out.push(debris(position, velocity, radius, alpha, rng.range(1.0, 1.6), 1300.0));
                } else {
                    let velocity = v2(rng.range(-30.0, 30.0) + direction * rng.range(0.0, 60.0), rng.range(-20.0, 60.0));
                    out.push(debris(position, velocity, radius, alpha, rng.range(0.9, 1.6), 900.0));
                }
            }
        }
        self.clear();
    }

    /// Окно тащат вбок: с отстающего края шапки снег сыплется тонкой струйкой.
    fn shed_trailing_edge(&mut self, origin: V2, from_left: bool, distance: f32, out: &mut Vec<SnowDebris>, limit: usize, rng: &mut Rng, visible: Visible) {
        let count = self.heights.len();
        let found = if from_left {
            (0..count).find(|&i| self.surface_height(i) > 1.0)
        } else {
            (0..count).rev().find(|&i| self.surface_height(i) > 1.0)
        };
        let Some(edge) = found else { return };

        let band = 8i32;
        let inward = if from_left { 1 } else { -1 };
        for _ in 0..stochastic_count(distance / 6.0, rng) {
            if out.len() >= limit {
                break;
            }
            let index = edge as i32 + inward * rng.index(band as usize) as i32;
            if index < 0 || index as usize >= count {
                continue;
            }
            let height = self.surface_height(index as usize);
            // Сразу за краем окна: над самим окном снег не упал бы на окно позади него.
            let gap = rng.range(0.0, 5.0);
            let x = if from_left { origin.x - gap } else { origin.x + self.width + gap };
            if height <= 0.5 || !visible(v2(x, origin.y - height / 2.0)) {
                continue;
            }
            out.push(debris(
                v2(x, origin.y - rng.range(0.0, height)),
                v2(rng.range(-12.0, 12.0), rng.range(0.0, 30.0)),
                rng.range(1.4, 3.0),
                rng.range(0.75, 1.0),
                rng.range(0.9, 1.4),
                900.0,
            ));
        }
        for offset in 0..band {
            let index = edge as i32 + inward * offset;
            if index >= 0 && (index as usize) < count {
                let h = &mut self.heights[index as usize];
                *h = (*h - (distance * 0.1).min(1.2)).max(0.0);
            }
        }
    }

    fn span(&self, center: i32, radius: i32) -> std::ops::RangeInclusive<usize> {
        let last = self.heights.len() as i32 - 1;
        ((center - radius).max(0) as usize)..=((center + radius).min(last).max(0) as usize)
    }

    /// Снежок забирает снег под собой. Возвращает собранную площадь в pt².
    fn carve(&mut self, u: f32, half_width: f32, strength: f32) -> f32 {
        if u < 0.0 || u > self.width {
            return 0.0;
        }
        let radius = ((half_width / STEP) as i32).max(1);
        let mut taken = 0.0;
        for index in self.span((u / STEP) as i32, radius) {
            let amount = self.heights[index] * strength * 0.8;
            self.heights[index] -= amount;
            taken += amount * STEP;
        }
        taken
    }

    /// Курсор прошёл сквозь снег: сбивает его вокруг себя.
    fn brush(&mut self, u: f32, origin: V2, cursor_velocity: V2, out: &mut Vec<SnowDebris>, limit: usize, rng: &mut Rng) {
        let center = (u / STEP) as i32;
        let radius = 6;
        for index in self.span(center, radius) {
            let visible = self.surface_height(index);
            let falloff = 1.0 - (index as i32 - center).abs() as f32 / (radius + 1) as f32;
            let taken = self.heights[index] * falloff.max(0.0) * 0.7;
            self.heights[index] -= taken;
            if visible <= 0.5 {
                continue;
            }
            for _ in 0..stochastic_count(taken / 3.0, rng) {
                if out.len() >= limit {
                    break;
                }
                let velocity = cursor_velocity.scaled(rng.range(0.1, 0.3)).plus(v2(rng.range(-50.0, 50.0), rng.range(-80.0, 0.0)));
                out.push(debris(
                    v2(origin.x + index as f32 * STEP, origin.y - rng.range(0.0, visible)),
                    velocity,
                    rng.range(1.6, 3.8),
                    rng.range(0.75, 1.0),
                    rng.range(0.9, 1.4),
                    1300.0,
                ));
            }
        }
    }

    /// Пока окно тащат, с шапки понемногу срываются комки.
    fn crumble(&mut self, origin: V2, intensity: f32, out: &mut Vec<SnowDebris>, limit: usize, rng: &mut Rng, visible: Visible) {
        for _ in 0..1 + (intensity * 5.0) as usize {
            if out.len() >= limit {
                break;
            }
            let index = rng.index(self.heights.len());
            let height = self.surface_height(index);
            let u = index as f32 * STEP;
            if height <= 1.5 || !visible(v2(origin.x + u, origin.y - height / 2.0)) {
                continue;
            }
            for neighbour in self.span(index as i32, 2) {
                self.heights[neighbour] = (self.heights[neighbour] - rng.range(0.3, 0.9)).max(0.0);
            }
            let velocity = self.velocity.scaled(rng.range(0.05, 0.2)).plus(v2(rng.range(-40.0, 40.0), rng.range(-40.0, 40.0)));
            out.push(debris(v2(origin.x + u, origin.y - height), velocity, rng.range(1.4, 3.0), rng.range(0.75, 1.0), rng.range(0.7, 1.2), 900.0));
        }
    }

    fn clear(&mut self) {
        self.heights.fill(0.0);
        self.active_until = 0.0;
    }

    /// Высота поверхности с учётом скругления углов и мелких неровностей.
    fn surface_height(&self, index: usize) -> f32 {
        let raw = self.heights[index];
        (raw + self.detail[index] * (raw / 5.0).min(1.0)).max(0.0) * self.envelope(index as f32 * STEP)
    }

    fn height(&self, u: f32) -> f32 {
        let last = self.heights.len() as i32 - 1;
        self.surface_height(((u / STEP) as i32).clamp(0, last) as usize)
    }

    fn deposit(&mut self, u: f32, amount: f32, max_height: f32) {
        let center = (u / STEP) as i32;
        for index in self.span(center, 12) {
            let distance = (index as i32 - center) as f32 / 5.0;
            let room = (1.0 - self.heights[index] / max_height.min(self.height_limit)).max(0.0);
            self.heights[index] += amount * (-0.5 * distance * distance).exp() * room;
        }
    }

    /// Излишек крутизны сползает к соседним точкам, общий объём снега не меняется.
    fn relax(&mut self) {
        for index in 0..self.heights.len() - 1 {
            let difference = self.heights[index] - self.heights[index + 1];
            let excess = difference.abs() - 0.45;
            if excess > 0.0 {
                let transfer = excess * 0.25 * difference.signum();
                self.heights[index] -= transfer;
                self.heights[index + 1] += transfer;
            }
        }
    }
}

const MAX_HEIGHT: f32 = 16.0;
const MAX_CACHED_CAPS: usize = 64;
const LANDING_CHANCE: f32 = 0.55;
pub const MAX_DEBRIS: usize = 3000;
const CRUMBLE_SPEED: f32 = 500.0;
const FLING_SPEED: f32 = 1500.0;
const CLOSE_DELAY: f64 = 0.15;
const MAX_BRUSH_SPEED: f32 = 1500.0;
const MIN_BRUSH_SPEED: f32 = 60.0;
const MAX_BALL_RADIUS: f32 = 34.0;
/// С какой скорости движение курсора считается взмахом, pt/s.
const THROW_SPEED: f32 = 700.0;
const MAX_THROW_SPEED: f32 = 3500.0;

fn mark_source(list: &mut [SnowDebris], id: u64, from: usize) {
    for particle in &mut list[from..] {
        particle.source = id;
    }
}

fn covered(front: &[TrackedWindow], p: V2) -> bool {
    front.iter().any(|w| w.frame.contains(p))
}

struct PendingClosed {
    id: u64,
    list: Vec<TrackedWindow>,
    deadline: f64,
}

/// Снег на всех окнах в общих координатах. Один на все экраны.
pub struct CapField {
    caps: HashMap<u64, SnowCap>,
    windows: Vec<TrackedWindow>,
    debris: Vec<SnowDebris>,
    last_debris_step: f64,
    pending_closed: Vec<PendingClosed>,
    grabbed: Option<u64>,
    grab_cursor: V2,
    was_mouse_down: bool,
    last_brush_time: f64,
    last_brush_cursor: Option<V2>,
    ball: Option<SnowBall>,
    pub cursor_effects: bool,
    dwell_time: f32,
    dwell_path: f32,
    smoothed_cursor_speed: f32,
    flick_velocity: V2,
    last_sync: f64,
    last_relax: f64,
    has_seeded: bool,
    /// Первый список папок приходит из своего потока, позже первого снимка окон.
    has_seeded_folders: bool,
    screen_bottom: f32,
    rng: Rng,
}

impl CapField {
    pub fn new(seed: u64) -> Self {
        CapField {
            caps: HashMap::new(),
            windows: Vec::new(),
            debris: Vec::new(),
            last_debris_step: 0.0,
            pending_closed: Vec::new(),
            grabbed: None,
            grab_cursor: V2::default(),
            was_mouse_down: false,
            last_brush_time: 0.0,
            last_brush_cursor: None,
            ball: None,
            cursor_effects: true,
            dwell_time: 0.0,
            dwell_path: 0.0,
            smoothed_cursor_speed: 0.0,
            flick_velocity: V2::default(),
            last_sync: -1.0,
            last_relax: 0.0,
            has_seeded: false,
            has_seeded_folders: false,
            screen_bottom: f32::INFINITY,
            rng: Rng(seed),
        }
    }

    pub fn debris_count(&self) -> usize {
        self.debris.len()
    }

    pub fn cap_volume(&self, id: u64) -> Option<f32> {
        self.caps.get(&id).map(|c| c.heights.iter().sum())
    }

    /// Снимок окон применяется один раз, сколько бы экранов его ни запросили.
    pub fn sync(&mut self, snapshot: &Snapshot, now: f64) {
        if snapshot.timestamp == self.last_sync {
            return;
        }
        self.last_sync = snapshot.timestamp;
        self.screen_bottom = snapshot.screen_bottom;
        let previous = std::mem::replace(&mut self.windows, snapshot.windows.clone());

        let first_batch = !self.has_seeded && snapshot.timestamp > 0.0;
        let previous_frames: HashMap<u64, Rect> = previous.iter().rev().map(|w| (w.id, w.frame)).collect();
        let windows = std::mem::take(&mut self.windows);
        for (index, window) in windows.iter().enumerate().filter(|(_, w)| w.holds_snow) {
            if let Some(cap) = self.caps.get_mut(&window.id) {
                if cap.width != window.frame.w {
                    let first_new = self.debris.len();
                    let front = &windows[..index];
                    let old = previous_frames.get(&window.id).copied().unwrap_or(Rect { w: cap.width, ..window.frame });
                    cap.reshape(old, window.frame, &mut self.debris, MAX_DEBRIS, &mut self.rng, &|p| !covered(front, p));
                    mark_source(&mut self.debris, window.id, first_new);
                    cap.active_until = now + 3.0;
                }
                cap.last_seen = now;
                if !window.is_desktop_icon {
                    cap.corner_radius = window.corner_radius;
                } else if cap.surface_profile != window.top_profile {
                    // Размер значков поменяли: контур другой.
                    cap.surface_profile = window.top_profile.clone();
                }
            } else {
                let phases = v2(self.rng.range(0.0, 6.28), self.rng.range(0.0, 6.28));
                let mut cap = SnowCap::new(window.frame.w, phases, now);
                if first_batch || (window.is_desktop_icon && !self.has_seeded_folders) {
                    cap.seed(&mut self.rng);
                }
                cap.corner_radius = window.corner_radius;
                // Угол папки маленький: снег лежит почти на всю её ширину.
                if window.is_desktop_icon {
                    cap.corner_radius = Some(4.0);
                    cap.limit_height(5.0);
                    cap.surface_profile = window.top_profile.clone();
                }
                self.caps.insert(window.id, cap);
            }
        }
        self.has_seeded_folders |= windows.iter().any(|w| w.is_desktop_icon);
        self.windows = windows;
        if first_batch {
            self.has_seeded = true;
        }
        self.shed_closed_windows(&previous, now);
        self.apply_window_motion(&previous, snapshot, now);
        self.evict_if_needed();
    }

    /// Окно закрыли, свернули или оно ушло на другой рабочий стол: снег падает с места,
    /// где оно было. Решение откладывается: окно могло выпасть из списка на миг.
    fn shed_closed_windows(&mut self, previous: &[TrackedWindow], now: f64) {
        let current: std::collections::HashSet<u64> = self.windows.iter().map(|w| w.id).collect();
        let previous_ids: std::collections::HashSet<u64> = previous.iter().map(|w| w.id).collect();
        self.pending_closed.retain(|p| !current.contains(&p.id));

        for window in previous.iter().filter(|w| w.holds_snow && !w.is_dock && !current.contains(&w.id)) {
            // Вкладка окна: та же рамка и процесс, но другой номер. Снег остаётся на месте.
            let twin = self
                .windows
                .iter()
                .find(|w| w.holds_snow && !previous_ids.contains(&w.id) && w.frame == window.frame && w.owner == window.owner)
                .map(|w| w.id);
            match (twin, self.caps.remove(&window.id)) {
                (Some(twin), Some(cap)) => {
                    self.caps.insert(twin, cap);
                    if let Some(ball) = self.ball.as_mut().filter(|b| b.window == Some(window.id)) {
                        ball.window = Some(twin);
                    }
                }
                (_, cap) => {
                    if let Some(cap) = cap {
                        self.caps.insert(window.id, cap);
                    }
                    self.pending_closed.push(PendingClosed { id: window.id, list: previous.to_vec(), deadline: now + CLOSE_DELAY });
                }
            }
        }

        let mut index = 0;
        while index < self.pending_closed.len() {
            if self.pending_closed[index].deadline <= now {
                let entry = self.pending_closed.swap_remove(index);
                self.shed_snow(&entry.list, Some(entry.id), 0.0, None);
                self.caps.remove(&entry.id);
            } else {
                index += 1;
            }
        }
    }

    fn apply_window_motion(&mut self, previous: &[TrackedWindow], snapshot: &Snapshot, now: f64) {
        if snapshot.is_mouse_down && !self.was_mouse_down {
            self.grabbed = self.windows.iter().find(|w| w.frame.contains(snapshot.cursor)).map(|w| w.id);
            self.grab_cursor = snapshot.cursor;
        }
        // Папку взяли и потащили: положение иконки известно редко, поэтому снег с неё ссыпается сразу.
        if snapshot.is_mouse_down {
            if let Some(id) = self.grabbed {
                if self.windows.iter().any(|w| w.id == id && w.is_desktop_icon) && snapshot.cursor.minus(self.grab_cursor).len() >= 4.0 {
                    let list = self.windows.clone();
                    self.shed_snow(&list, Some(id), 0.0, None);
                }
            }
        }
        self.was_mouse_down = snapshot.is_mouse_down;
        if self.grabbed.is_none() || !snapshot.is_mouse_down {
            for cap in self.caps.values_mut() {
                cap.trail.clear();
            }
        }
        if !snapshot.is_mouse_down {
            return;
        }
        let Some(grabbed) = self.grabbed else { return };
        let Some(window) = self.windows.iter().find(|w| w.id == grabbed).cloned() else { return };
        if !window.holds_snow || window.is_fixed_surface() {
            return;
        }
        let Some(cap) = self.caps.get_mut(&grabbed) else { return };
        let old = match previous.iter().find(|w| w.id == grabbed) {
            Some(w) if w.frame.same_size(&window.frame) => w.frame,
            _ => {
                cap.trail.clear();
                return;
            }
        };

        let front: Vec<TrackedWindow> = previous.iter().take_while(|w| w.id != grabbed).cloned().collect();
        let visible = |p: V2| !covered(&front, p);
        let old_origin = v2(old.x, old.y);
        let shift_x = window.frame.x - old.x;
        if shift_x.abs() >= 1.0 {
            let first_new = self.debris.len();
            cap.shed_trailing_edge(old_origin, shift_x > 0.0, shift_x.abs(), &mut self.debris, MAX_DEBRIS, &mut self.rng, &visible);
            mark_source(&mut self.debris, grabbed, first_new);
            cap.active_until = now + 3.0;
        }

        // Скорость за последние 100 мс: один догоняющий шаг после паузы её не исказит.
        let origin = v2(window.frame.x, window.frame.y);
        cap.trail.push_back((origin, snapshot.timestamp));
        while cap.trail.front().is_some_and(|t| snapshot.timestamp - t.1 > 0.11) {
            cap.trail.pop_front();
        }
        let Some(&(oldest, oldest_time)) = cap.trail.front() else { return };
        let span = (snapshot.timestamp - oldest_time) as f32;
        if span < 0.06 {
            return;
        }
        cap.velocity = origin.minus(oldest).scaled(1.0 / span);
        let speed = cap.velocity.len();
        if speed < CRUMBLE_SPEED {
            return;
        }
        if speed >= FLING_SPEED {
            let fling = cap.velocity;
            self.shed_snow(previous, Some(grabbed), 0.0, Some(fling));
        } else {
            let first_new = self.debris.len();
            let intensity = (speed - CRUMBLE_SPEED) / (FLING_SPEED - CRUMBLE_SPEED);
            cap.crumble(old_origin, intensity, &mut self.debris, MAX_DEBRIS, &mut self.rng, &visible);
            mark_source(&mut self.debris, grabbed, first_new);
            cap.active_until = now + 3.0;
        }
    }

    fn shed_snow(&mut self, list: &[TrackedWindow], only: Option<u64>, direction: f32, fling: Option<V2>) {
        for (index, window) in list.iter().enumerate() {
            if !window.holds_snow || only.is_some_and(|id| id != window.id) {
                continue;
            }
            let Some(cap) = self.caps.get_mut(&window.id) else { continue };
            let first_new = self.debris.len();
            let front = &list[..index];
            cap.shed(v2(window.frame.x, window.frame.y), direction, fling, &mut self.debris, MAX_DEBRIS, &mut self.rng, &|p| !covered(front, p));
            mark_source(&mut self.debris, window.id, first_new);
        }
    }

    /// Вызывается каждый кадр. Медленное движение по снегу скатывает снежок,
    /// обычное стряхивает снег, резкий взмах ничего не задевает.
    pub fn brush(&mut self, cursor: V2, now: f64) {
        if now <= self.last_brush_time {
            return;
        }
        let interval = (now - self.last_brush_time).min(1.0 / 15.0) as f32;
        self.last_brush_time = now;
        let last = self.last_brush_cursor.replace(cursor);

        self.step_falling_ball(interval, now);
        if !self.cursor_effects {
            self.release_ball();
            return;
        }
        let Some(last) = last else { return };
        if self.was_mouse_down {
            self.release_ball();
            self.dwell_time = 0.0;
            self.dwell_path = 0.0;
            return;
        }

        let delta = cursor.minus(last);
        let distance = delta.len();
        let mut velocity = delta.scaled(1.0 / interval.max(0.004));
        let speed = velocity.len();
        // На малой скорости курсор идёт рывками, поэтому порог сравнивается со сглаженной скоростью.
        self.smoothed_cursor_speed += (speed - self.smoothed_cursor_speed) * (interval * 8.0).min(1.0);

        self.flick_velocity = self.flick_velocity.plus(velocity.minus(self.flick_velocity).scaled((interval * 20.0).min(1.0)));

        if self.ball.is_some_and(|b| b.window.is_some()) {
            if self.flick_velocity.len() >= THROW_SPEED {
                self.throw_ball(self.flick_velocity);
            } else {
                self.roll_ball(cursor, interval, now);
            }
            return;
        }
        if self.ball.is_none() && self.start_ball_if_dwelling(cursor, self.smoothed_cursor_speed, distance, interval) {
            return;
        }
        if distance < 1.0 || distance >= 600.0 || self.smoothed_cursor_speed < MIN_BRUSH_SPEED || speed > MAX_BRUSH_SPEED {
            return;
        }
        if speed > 900.0 {
            velocity = velocity.scaled(900.0 / speed);
        }
        let steps = ((distance / 5.0) as i32 + 1).min(30);
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            self.knock_snow(last.x + delta.x * t, last.y + delta.y * t, velocity, now);
        }
    }

    /// Окно с видимым снегом, в слое которого находится точка.
    fn snow_surface(&self, x: f32, y: f32) -> Option<(usize, f32)> {
        for (index, window) in self.windows.iter().enumerate().filter(|(_, w)| w.holds_snow) {
            let frame = window.frame;
            if x <= frame.x || x >= frame.max_x() {
                continue;
            }
            let Some(cap) = self.caps.get(&window.id) else { continue };
            let height = cap.height(x - frame.x);
            if height > 0.5 && y >= frame.y - height - 3.0 && y <= frame.y + 1.0 && !covered(&self.windows[..index], v2(x, y)) {
                return Some((index, height));
            }
        }
        None
    }

    fn start_ball_if_dwelling(&mut self, cursor: V2, speed: f32, distance: f32, dt: f32) -> bool {
        let surface = if speed < MIN_BRUSH_SPEED { self.snow_surface(cursor.x, cursor.y) } else { None };
        let Some((index, height)) = surface else {
            self.dwell_time = 0.0;
            self.dwell_path = 0.0;
            return false;
        };
        self.dwell_time += dt;
        self.dwell_path += distance;
        if self.dwell_time < 0.2 || self.dwell_path < 8.0 {
            return false;
        }
        self.dwell_time = 0.0;
        self.dwell_path = 0.0;
        let (id, top) = (self.windows[index].id, self.windows[index].frame.y);
        let radius = 6.0;
        self.ball = Some(SnowBall {
            position: v2(cursor.x, top - height - radius * 0.8),
            velocity: V2::default(),
            radius,
            rotation: 0.0,
            window: Some(id),
            dropped_from: 0,
        });

        // Шар появляется не из воздуха: снег под ним вздымается облачком.
        let first_new = self.debris.len();
        let center = v2(cursor.x, top - height * 0.5);
        let rng = &mut self.rng;
        for _ in 0..26 {
            if self.debris.len() >= MAX_DEBRIS {
                break;
            }
            self.debris.push(debris(
                center.plus(v2(rng.range(-9.0, 9.0), rng.range(-4.0, 3.0))),
                v2(rng.range(-95.0, 95.0), rng.range(-190.0, -40.0)),
                rng.range(1.2, 3.2),
                rng.range(0.7, 1.0),
                rng.range(0.35, 0.7),
                900.0,
            ));
        }
        mark_source(&mut self.debris, id, first_new);
        true
    }

    fn roll_ball(&mut self, cursor: V2, dt: f32, now: f64) {
        let Some(mut current) = self.ball else { return };
        let Some(id) = current.window else { return };
        let Some(index) = self.windows.iter().position(|w| w.id == id) else {
            self.release_ball();
            return;
        };
        let frame = self.windows[index].frame;
        let Some(cap) = self.caps.get_mut(&id) else {
            self.release_ball();
            return;
        };

        let previous_x = current.position.x;
        current.position.x += (cursor.x - previous_x) * (dt * 18.0).min(1.0);
        let moved = current.position.x - previous_x;
        current.rotation += moved / current.radius;
        current.velocity = v2((moved / dt.max(0.004)).clamp(-400.0, 400.0), 0.0);

        let u = current.position.x - frame.x;
        if u < 0.0 || u > frame.w {
            self.ball = Some(current);
            self.release_ball();
            return;
        }

        if moved.abs() > 0.05 {
            let taken = cap.carve(u, current.radius * 0.8, (moved.abs() / 5.0).min(1.0));
            current.radius = (current.radius * current.radius + taken * 0.25 / std::f32::consts::PI).sqrt().min(MAX_BALL_RADIUS);
            cap.active_until = now + 3.0;
        }

        // Из-под катящегося шара летит немного снега: видно, что он идёт по сугробу.
        let ground = cap.height(u);
        let snow_ahead = cap.height(u + if moved > 0.0 { 1.0 } else { -1.0 } * current.radius * 1.1);
        if moved.abs() > 0.05 && snow_ahead > 0.5 {
            let rng = &mut self.rng;
            let behind = if moved > 0.0 { -1.0 } else { 1.0 };
            let first_new = self.debris.len();
            for _ in 0..stochastic_count(moved.abs() / 4.0 * (current.radius / 8.0), rng) {
                if self.debris.len() >= MAX_DEBRIS {
                    break;
                }
                self.debris.push(debris(
                    v2(current.position.x + behind * current.radius * rng.range(0.2, 0.8), frame.y - snow_ahead * rng.range(0.2, 1.0)),
                    v2(behind * rng.range(15.0, 90.0), rng.range(-130.0, -30.0)),
                    rng.range(1.2, 3.2),
                    rng.range(0.7, 1.0),
                    rng.range(0.35, 0.7),
                    900.0,
                ));
            }
            mark_source(&mut self.debris, id, first_new);
        }

        let target_y = frame.y - ground - current.radius * 0.8;
        current.position.y += (target_y - current.position.y) * (dt * 20.0).min(1.0);
        self.ball = Some(current);

        let reach = current.radius * 2.0 + 30.0;
        if covered(&self.windows[..index], current.position) || cursor.y < frame.y - reach || cursor.y > frame.y + 30.0 {
            self.release_ball();
        }
    }

    /// Взмах курсора бросает снежок: дальность зависит от скорости взмаха.
    fn throw_ball(&mut self, velocity: V2) {
        let Some(mut current) = self.ball else { return };
        let Some(id) = current.window else { return };
        current.velocity = velocity.scaled(0.85 * (MAX_THROW_SPEED / velocity.len()).min(1.0));
        current.dropped_from = id;
        current.window = None;
        self.ball = Some(current);
        self.spawn_puff(v2(current.position.x, current.position.y + current.radius), current.radius as usize, (20.0, 110.0), (30.0, 150.0), (0.3, 0.6));
    }

    fn release_ball(&mut self) {
        if let Some(ball) = self.ball.as_mut() {
            if let Some(id) = ball.window.take() {
                ball.dropped_from = id;
            }
        }
    }

    fn step_falling_ball(&mut self, dt: f32, now: f64) {
        let Some(mut current) = self.ball.filter(|b| b.window.is_none()) else { return };
        let previous_bottom = current.position.y + current.radius;
        current.velocity.y += 1300.0 * dt;
        current.position = current.position.plus(current.velocity.scaled(dt));
        current.rotation += current.velocity.x * dt / current.radius;

        let bottom = current.position.y + current.radius;
        let landed = self.land(current.position.x, previous_bottom, bottom, (current.radius * 0.4).min(8.0), now, 1.0, current.dropped_from, true);
        if landed {
            self.shatter(current);
            self.ball = None;
        } else if bottom - current.radius * 2.0 > self.screen_bottom {
            self.ball = None;
        } else {
            self.ball = Some(current);
        }
    }

    fn shatter(&mut self, ball: SnowBall) {
        let count = ((ball.radius * ball.radius * 0.35) as usize).min(220);
        let rng = &mut self.rng;
        for _ in 0..count {
            if self.debris.len() >= MAX_DEBRIS {
                break;
            }
            let angle = rng.range(0.0, std::f32::consts::TAU);
            let direction = v2(angle.cos(), angle.sin());
            self.debris.push(debris(
                ball.position.plus(direction.scaled(rng.range(0.0, ball.radius))),
                direction.scaled(rng.range(60.0, 220.0)).minus(v2(0.0, rng.range(40.0, 200.0))),
                rng.range(1.6, 4.2),
                rng.range(0.75, 1.0),
                rng.range(0.7, 1.3),
                1300.0,
            ));
        }
    }

    pub fn ball_for_drawing(&self, origin: V2) -> Option<BallDraw> {
        self.ball.map(|b| BallDraw {
            center: b.position.minus(origin),
            radius: b.radius,
            rotation: b.rotation,
            owner: b.window.unwrap_or(b.dropped_from),
        })
    }

    fn knock_snow(&mut self, x: f32, y: f32, velocity: V2, now: f64) {
        let Some((index, _)) = self.snow_surface(x, y) else { return };
        let (id, frame) = (self.windows[index].id, self.windows[index].frame);
        let Some(cap) = self.caps.get_mut(&id) else { return };
        let first_new = self.debris.len();
        cap.brush(x - frame.x, v2(frame.x, frame.y), velocity, &mut self.debris, MAX_DEBRIS, &mut self.rng);
        mark_source(&mut self.debris, id, first_new);
        cap.active_until = now + 3.0;
    }

    /// Панель задач основного экрана, если она лежит вдоль его низа.
    pub fn dock_frame(&self, screen: Rect) -> Option<Rect> {
        self.windows.iter().find(|w| w.is_dock && w.frame.x < screen.max_x() && w.frame.max_x() > screen.x && w.frame.max_y() >= screen.max_y() - 1.0 && w.frame.y < screen.max_y() + 1.0).map(|w| w.frame)
    }

    /// Облачко снега: из-под полоза и из открытого подарка.
    pub fn spawn_puff(&mut self, center: V2, count: usize, speed: (f32, f32), lift: (f32, f32), life: (f32, f32)) {
        for _ in 0..count {
            if self.debris.len() >= MAX_DEBRIS {
                break;
            }
            let rng = &mut self.rng;
            let side = if rng.flip() { 1.0 } else { -1.0 };
            let position = center.plus(v2(rng.range(-14.0, 14.0), rng.range(-8.0, 8.0)));
            let velocity = v2(side * rng.range(speed.0, speed.1), -rng.range(lift.0, lift.1));
            let (radius, alpha, life) = (rng.range(1.4, 3.6), rng.range(0.7, 1.0), rng.range(life.0, life.1));
            self.debris.push(debris(position, velocity, radius, alpha, life, 900.0));
        }
    }

    pub fn step_debris(&mut self, now: f64) {
        let last = std::mem::replace(&mut self.last_debris_step, now);
        if now <= last || last <= 0.0 || self.debris.is_empty() {
            return;
        }
        let dt = (now - last).min(1.0 / 15.0) as f32;
        for index in 0..self.debris.len() {
            let mut p = self.debris[index];
            p.velocity.y += p.gravity * dt;
            p.velocity.x *= (1.0 - 1.2 * dt).max(0.0);
            let previous_y = p.position.y;
            p.position = p.position.plus(p.velocity.scaled(dt));
            p.life -= dt;
            // Снег пересыпается на окна ниже и пополняет их сугроб.
            if p.velocity.y > 0.0 && self.land(p.position.x, previous_y, p.position.y, 0.6, now, 1.0, p.source, true) {
                p.life = 0.0;
            }
            self.debris[index] = p;
        }
        self.debris.retain(|p| p.life > 0.0);
    }

    pub fn write_debris(&self, out: &mut Vec<FlakeInstance>, origin: V2, viewport: V2) {
        for p in &self.debris {
            let local = p.position.minus(origin);
            if local.x > -8.0 && local.x < viewport.x + 8.0 && local.y > -8.0 && local.y < viewport.y + 8.0 {
                out.push(FlakeInstance {
                    rect: [local.x - p.radius, local.y - p.radius, p.radius * 2.0, p.radius * 2.0],
                    alpha: p.alpha * (p.life / 0.35).min(1.0),
                    softness: 0.7,
                    ring: 0.0,
                    padding: 0.0,
                });
            }
        }
    }

    fn evict_if_needed(&mut self) {
        if self.caps.len() <= MAX_CACHED_CAPS {
            return;
        }
        let mut ages: Vec<(f64, u64)> = self.caps.iter().map(|(id, c)| (c.last_seen, *id)).collect();
        ages.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, id) in ages.iter().take(self.caps.len() - MAX_CACHED_CAPS) {
            self.caps.remove(id);
        }
    }

    pub fn relax(&mut self, now: f64) {
        if now - self.last_relax < 1.0 / 30.0 {
            return;
        }
        self.last_relax = now;
        for cap in self.caps.values_mut().filter(|c| c.active_until > now) {
            cap.relax();
        }
    }

    /// Хлопок снегопада: садится не всегда и только на ровную часть края.
    pub fn try_land(&mut self, x: f32, from_y: f32, to_y: f32, amount: f32, now: f64) -> bool {
        self.land(x, from_y, to_y, amount, now, LANDING_CHANCE, 0, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn land(&mut self, x: f32, from_y: f32, to_y: f32, amount: f32, now: f64, chance: f32, excluding: u64, whole_edge: bool) -> bool {
        for index in 0..self.windows.len() {
            let window = &self.windows[index];
            if !window.holds_snow || window.id == excluding || x <= window.frame.x || x >= window.frame.max_x() {
                continue;
            }
            // Скрытая панель задач ушла за нижний край: на неё снег не садится.
            if window.is_dock_hidden {
                continue;
            }
            let frame = window.frame;
            let id = window.id;
            let Some(cap) = self.caps.get_mut(&id) else { continue };
            let u = x - frame.x;
            if !whole_edge && !cap.can_hold_snow(u) {
                continue;
            }
            let surface = frame.y + cap.base_drop(u) - cap.height(u);
            if from_y > surface || to_y < surface || covered(&self.windows[..index], v2(x, surface)) {
                continue;
            }
            // Часть хлопьев пролетает дальше, иначе окна под верхним окном остались бы без снега.
            if self.rng.unit() >= chance {
                continue;
            }
            cap.deposit(u, amount, MAX_HEIGHT);
            cap.active_until = now + 3.0;
            return true;
        }
        false
    }

    /// Строит сетки шапок для одного экрана от задних окон к передним.
    pub fn build_mesh(&self, vertices: &mut Vec<CapVertex>, capacity: usize, origin: V2, viewport: V2, draws: &mut Vec<CapDraw>) {
        vertices.clear();
        draws.clear();
        for window in self.windows.iter().rev() {
            let x = window.frame.x - origin.x;
            let mut y = window.frame.y - origin.y;
            // Шапка скрытой панели уходит за нижний край вместе с ней и не оставляет полосу.
            if window.is_dock {
                y += (y - (viewport.y - MAX_HEIGHT - 2.0)).max(0.0);
            }
            let (width, height) = (window.frame.w, window.frame.h);
            if !(x < viewport.x && x + width > 0.0 && y - MAX_HEIGHT - 4.0 < viewport.y && y + height > 0.0) {
                continue;
            }
            let cap = self.caps.get(&window.id);
            let mut draw = CapDraw {
                rect: [x, y, width, height],
                vertex_start: vertices.len(),
                vertex_count: 0,
                window: window.id,
                corner_radius: window.corner_radius.unwrap_or(0.0),
            };
            if let Some(cap) = cap.filter(|c| window.holds_snow && vertices.len() + c.heights.len() * 2 <= capacity) {
                for index in 0..cap.heights.len() {
                    let u = (index as f32 * STEP).min(width);
                    let h = cap.surface_height(index);
                    let base = y + cap.base_drop(u);
                    let n = cap.surface_normal(u);
                    vertices.push(CapVertex { position: [x + u + n.x * (h + 1.0), base - n.y * (h + 1.0), -1.0, u], thickness: h });
                    vertices.push(CapVertex { position: [x + u - n.x, base + n.y, h + 1.0, u], thickness: h });
                }
                draw.vertex_count = cap.heights.len() * 2;
            }
            draws.push(draw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SnowField;

    fn window(id: u64, x: f32, y: f32, w: f32, h: f32) -> TrackedWindow {
        TrackedWindow { id, frame: Rect { x, y, w, h }, owner: id as u32, holds_snow: true, is_dock: false, is_dock_hidden: false, is_desktop_icon: false, corner_radius: Some(8.0), top_profile: None }
    }

    fn folder(id: u64) -> TrackedWindow {
        TrackedWindow { is_desktop_icon: true, corner_radius: None, top_profile: Some(vec![0.0; 8]), ..window(id, 100.0, 100.0, 64.0, 64.0) }
    }

    fn snap(t: f64, windows: Vec<TrackedWindow>, down: bool, cursor: V2) -> Snapshot {
        Snapshot { timestamp: t, windows, is_mouse_down: down, cursor, screen_bottom: 1000.0 }
    }

    #[test]
    fn folders_from_late_list_get_first_snow_once() {
        let mut field = CapField::new(1);
        field.sync(&snap(0.1, vec![window(1, 400.0, 300.0, 400.0, 300.0)], false, v2(0.0, 0.0)), 0.1);
        field.sync(&snap(0.2, vec![window(1, 400.0, 300.0, 400.0, 300.0), folder(2)], false, v2(0.0, 0.0)), 0.2);
        assert!(field.cap_volume(2).unwrap() > 0.0);
        field.sync(&snap(0.3, vec![window(1, 400.0, 300.0, 400.0, 300.0), folder(2), folder(3)], false, v2(0.0, 0.0)), 0.3);
        assert_eq!(field.cap_volume(3), Some(0.0));
    }

    #[test]
    fn dock_below_primary_monitor_holds_snow_unless_hidden() {
        let dock = |hidden| TrackedWindow { is_dock: true, is_dock_hidden: hidden, corner_radius: Some(0.0), ..window(1, 0.0, 2000.0, 1000.0, 48.0) };
        let mut field = CapField::new(1);
        field.sync(&snap(0.1, vec![dock(false)], false, v2(0.0, 0.0)), 0.1);
        assert!(field.land(500.0, 1900.0, 2010.0, 1.0, 0.2, 1.0, 0, true));
        let mut field = CapField::new(1);
        field.sync(&snap(0.1, vec![dock(true)], false, v2(0.0, 0.0)), 0.1);
        assert!(!field.land(500.0, 1900.0, 2010.0, 1.0, 0.2, 1.0, 0, true));
    }

    #[test]
    fn hidden_dock_is_measured_by_its_own_monitor() {
        assert!(dock_is_hidden(1438.0, 1440.0));
        assert!(!dock_is_hidden(1392.0, 1440.0));
        assert!(!dock_is_hidden(2552.0, 2600.0));
        assert!(dock_is_hidden(-2.0, 0.0));
    }

    #[test]
    fn seeded_window_gets_snow_and_flakes_add_to_it() {
        let mut field = CapField::new(1);
        let w = vec![window(1, 100.0, 300.0, 400.0, 300.0)];
        field.sync(&snap(0.1, w, false, v2(0.0, 0.0)), 0.1);
        let before = field.cap_volume(1).unwrap();
        assert!(before > 0.0);
        let mut landed = 0;
        for i in 0..400 {
            if field.try_land(300.0, 0.0, 400.0, 1.8, 0.2 + i as f64) {
                landed += 1;
            }
        }
        assert!(landed > 0);
    }

    #[test]
    fn covered_edge_takes_no_snow() {
        let mut field = CapField::new(2);
        let front = window(2, 0.0, 0.0, 800.0, 800.0);
        let back = window(1, 100.0, 300.0, 400.0, 300.0);
        field.sync(&snap(0.0, vec![front, back], false, v2(0.0, 0.0)), 0.0);
        for i in 0..200 {
            field.land(300.0, 290.0, 310.0, 1.8, i as f64, 1.0, 2, true);
        }
        assert_eq!(field.cap_volume(1), Some(0.0));
    }

    #[test]
    fn closing_window_sheds_after_delay() {
        let mut field = CapField::new(3);
        field.sync(&snap(0.1, vec![window(1, 100.0, 300.0, 400.0, 300.0)], false, v2(0.0, 0.0)), 0.1);
        field.sync(&snap(0.2, vec![], false, v2(0.0, 0.0)), 0.2);
        assert!(field.cap_volume(1).is_some());
        field.sync(&snap(0.5, vec![], false, v2(0.0, 0.0)), 0.5);
        assert_eq!(field.cap_volume(1), None);
        assert!(field.debris_count() > 0);
    }

    #[test]
    fn widening_keeps_volume() {
        let mut field = CapField::new(4);
        field.sync(&snap(0.1, vec![window(1, 100.0, 300.0, 400.0, 300.0)], false, v2(0.0, 0.0)), 0.1);
        let before = field.cap_volume(1).unwrap();
        field.sync(&snap(0.2, vec![window(1, 100.0, 300.0, 600.0, 300.0)], false, v2(0.0, 0.0)), 0.2);
        assert!((field.cap_volume(1).unwrap() - before).abs() < before * 0.02);
    }

    #[test]
    fn fast_drag_flings_all_snow() {
        let mut field = CapField::new(5);
        let at = |x: f32| vec![window(1, x, 300.0, 400.0, 300.0)];
        field.sync(&snap(0.1, at(100.0), false, v2(200.0, 310.0)), 0.1);
        for i in 1..12 {
            let t = 0.1 + i as f64 / 60.0;
            field.sync(&snap(t, at(100.0 + i as f32 * 40.0), true, v2(200.0 + i as f32 * 40.0, 310.0)), t);
        }
        assert_eq!(field.cap_volume(1), Some(0.0));
        assert!(field.debris_count() > 0);
    }

    #[test]
    fn mesh_follows_corner_arc_and_stays_finite() {
        let mut field = CapField::new(6);
        field.sync(&snap(0.1, vec![window(1, 100.0, 300.0, 400.0, 300.0)], false, v2(0.0, 0.0)), 0.1);
        let (mut vertices, mut draws) = (Vec::new(), Vec::new());
        field.build_mesh(&mut vertices, 64_000, v2(0.0, 0.0), v2(1920.0, 1080.0), &mut draws);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].vertex_count, vertices.len());
        assert!(vertices.iter().all(|v| v.position.iter().all(|c| c.is_finite()) && v.thickness.is_finite()));
    }

    #[test]
    fn flick_throws_rolling_ball() {
        let mut field = CapField::new(9);
        let w = window(1, 100.0, 300.0, 800.0, 300.0);
        field.sync(&snap(0.1, vec![w], false, v2(0.0, 0.0)), 0.1);
        field.ball = Some(SnowBall { position: v2(400.0, 280.0), velocity: V2::default(), radius: 10.0, rotation: 0.0, window: Some(1), dropped_from: 0 });
        field.brush(v2(400.0, 295.0), 1.0);
        for i in 1..12 {
            field.brush(v2(400.0 + i as f32 * 40.0, 295.0), 1.0 + i as f64 / 60.0);
            if field.ball.is_some_and(|b| b.window.is_none()) {
                break;
            }
        }
        let ball = field.ball.expect("снежок в полёте");
        assert_eq!(ball.window, None);
        assert_eq!(ball.dropped_from, 1);
        assert!(ball.velocity.x > 500.0, "скорость {}", ball.velocity.x);
    }

    /// Случайная жизнь рабочего стола: панику или NaN здесь дешевле поймать, чем на ПК.
    #[test]
    fn random_desktop_never_panics() {
        let mut rng = Rng(77);
        let mut field = CapField::new(7);
        let mut snow = SnowField::new(1920.0, 1080.0, 8);
        let mut windows: Vec<TrackedWindow> = (1..=8)
            .map(|id| window(id, rng.range(-200.0, 1500.0), rng.range(-100.0, 900.0), rng.range(60.0, 1200.0), rng.range(60.0, 800.0)))
            .collect();
        let mut next_id = 100;
        let mut cursor = v2(500.0, 500.0);
        let mut down = false;
        let (mut vertices, mut draws, mut flakes) = (Vec::new(), Vec::new(), Vec::new());
        for frame in 0..20_000 {
            let now = 1.0 + frame as f64 / 60.0;
            match rng.index(40) {
                0 if !windows.is_empty() => {
                    windows.remove(rng.index(windows.len()));
                }
                1 if windows.len() < 30 => {
                    next_id += 1;
                    windows.insert(0, window(next_id, rng.range(-200.0, 1500.0), rng.range(-100.0, 900.0), rng.range(60.0, 1200.0), rng.range(60.0, 800.0)));
                }
                2 | 3 if !windows.is_empty() => {
                    let i = rng.index(windows.len());
                    windows[i].frame.x += rng.range(-300.0, 300.0);
                    windows[i].frame.w = (windows[i].frame.w + rng.range(-400.0, 400.0)).max(60.0);
                }
                4 => down = !down,
                5 if !windows.is_empty() => {
                    let i = rng.index(windows.len());
                    windows[i].corner_radius = [None, Some(0.0), Some(4.0), Some(8.0), Some(12.0)][rng.index(5)];
                }
                _ => {}
            }
            cursor = cursor.plus(v2(rng.range(-30.0, 30.0), rng.range(-30.0, 30.0)));
            if rng.index(50) == 0 && !windows.is_empty() {
                let f = windows[rng.index(windows.len())].frame;
                cursor = v2(f.mid_x(), f.y - 2.0);
            }
            field.sync(&snap(now, windows.clone(), down, cursor), now);
            snow.step(1.0 / 60.0, now as f32, &mut field, v2(0.0, 0.0), now);
            field.relax(now);
            field.brush(cursor, now);
            field.step_debris(now);
            field.build_mesh(&mut vertices, 64_000, v2(0.0, 0.0), v2(1920.0, 1080.0), &mut draws);
            flakes.clear();
            snow.write_instances(&mut flakes, now as f32);
            field.write_debris(&mut flakes, v2(0.0, 0.0), v2(1920.0, 1080.0));
            assert!(vertices.iter().all(|v| v.position.iter().all(|c| c.is_finite())), "кадр {frame}");
            assert!(flakes.iter().all(|f| f.rect.iter().all(|c| c.is_finite())), "кадр {frame}");
            assert!(field.debris_count() <= MAX_DEBRIS);
        }
    }
}
