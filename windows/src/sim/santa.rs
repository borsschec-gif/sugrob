//! Перенос `Santa.swift`: проезд Деда Мороза и его подарок.
//! Живёт в общих точках основного экрана, y вниз.
use super::santa_art::{SANTA_FRAME_COUNT, SANTA_HEIGHT, SANTA_WIDTH};
use super::{v2, CapField, Rect, Rng, V2};

const SPEED: f32 = 230.0;
pub const GIFT_SIZE: f32 = 44.0;
pub const PHRASE_PADDING: f32 = 18.0;
pub const PHRASES: [&str; 8] = [
    "С наступающим!",
    "Оливье само себя не нарежет",
    "Рендер закончится к Новому году",
    "Снег идёт. Дедлайн тоже",
    "Мандарины официально считаются овощем",
    "В этом году ты вёл себя… на монтаже",
    "Ёлку поливать не забывай",
    "Подарок был внутри тебя",
];

/// Что рисовать на кадре: прямоугольник в локальных точках экрана и номер спрайта.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SantaDraw {
    pub rect: [f32; 4],
    pub frame: usize,
}

#[derive(Clone, Copy, Debug)]
struct Gift {
    position: V2,
    velocity: f32,
    expires: f64,
}

/// Открытый подарок: где показать фразу и какую.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpenedGift {
    pub center: V2,
    pub phrase: &'static str,
}

pub struct Santa {
    x: Option<f32>,
    drop_x: f32,
    did_drop: bool,
    ground: Option<f32>,
    last_tick: f64,
    next_ride: f64,
    animation: f32,
    gift: Option<Gift>,
    rng: Rng,
    pub auto_rides: bool,
}

/// На видимой панели тройка едет по её нижней кромке, иначе вдоль низа экрана.
fn ground_level(screen: Rect, dock: Option<Rect>) -> f32 {
    let bottom = screen.max_y();
    match dock {
        Some(dock) if dock.y < bottom - 1.0 => (dock.max_y() - 6.0).min(bottom - 4.0),
        _ => bottom - 4.0,
    }
}

/// Левый верхний угол плашки с фразой: над подарком и в пределах экрана.
pub fn phrase_origin(center: V2, size: V2, screen: Rect) -> V2 {
    v2((center.x - size.x / 2.0).max(12.0).min(screen.w - size.x - 12.0), center.y - 34.0 - size.y)
}

impl Santa {
    pub fn new(seed: u64) -> Self {
        Santa { x: None, drop_x: 0.0, did_drop: false, ground: None, last_tick: 0.0, next_ride: 0.0, animation: 0.0, gift: None, rng: Rng(seed), auto_rides: true }
    }

    /// Запускает проезд из меню, не дожидаясь расписания.
    pub fn ride_now(&mut self, screen: Rect) {
        if self.x.is_none() {
            self.start_ride(screen);
        }
    }

    fn start_ride(&mut self, screen: Rect) {
        self.x = Some(screen.w + 20.0);
        self.drop_x = screen.w * self.rng.range(0.3, 0.65);
        self.did_drop = false;
    }

    pub fn tick(&mut self, now: f64, screen: Rect, caps: &mut CapField) {
        if now <= self.last_tick {
            return;
        }
        let dt = (now - self.last_tick).min(1.0 / 15.0) as f32;
        self.last_tick = now;

        if self.next_ride == 0.0 {
            self.next_ride = now + 60.0;
        }
        if self.auto_rides && self.x.is_none() && now >= self.next_ride {
            self.next_ride = now + self.rng.range(240.0, 480.0) as f64;
            self.start_ride(screen);
        }

        let target = ground_level(screen, caps.dock_frame(screen));
        let ground = self.ground.map_or(target, |g| g + (target - g) * (dt * 14.0).min(1.0));
        self.ground = Some(ground);
        self.update_ride(dt, now, ground, caps);
        self.update_gift(dt, now, ground);
    }

    fn update_ride(&mut self, dt: f32, now: f64, ground: f32, caps: &mut CapField) {
        let Some(mut position) = self.x else { return };
        position -= SPEED * dt;
        self.animation += dt * 9.0;

        // Из-под полоза летит снег.
        caps.spawn_puff(v2(position + 190.0, ground - 3.0), 1, (20.0, 90.0), (20.0, 110.0), (0.3, 0.6));

        if !self.did_drop && position + 150.0 <= self.drop_x {
            self.did_drop = true;
            self.gift = Some(Gift { position: v2(position + 180.0, ground - 120.0), velocity: 0.0, expires: now + 60.0 });
        }
        self.x = if position < -SANTA_WIDTH - 20.0 { None } else { Some(position) };
    }

    fn update_gift(&mut self, dt: f32, now: f64, ground: f32) {
        let Some(gift) = &mut self.gift else { return };
        if now >= gift.expires {
            self.gift = None;
            return;
        }
        let rest = ground - GIFT_SIZE;
        if gift.position.y < rest || gift.velocity != 0.0 {
            gift.velocity += 1300.0 * dt;
            gift.position.y += gift.velocity * dt;
            if gift.position.y >= rest {
                gift.position.y = rest;
                // Небольшой отскок, затем подарок лежит.
                gift.velocity = if gift.velocity.abs() > 180.0 { -gift.velocity.abs() * 0.32 } else { 0.0 };
            }
        } else {
            gift.position.y = rest;
        }
    }

    pub fn draw_item(&self, origin: V2) -> Option<SantaDraw> {
        let (x, ground) = (self.x?, self.ground?);
        Some(SantaDraw { rect: [x - origin.x, ground - SANTA_HEIGHT - origin.y, SANTA_WIDTH, SANTA_HEIGHT], frame: self.animation as usize % SANTA_FRAME_COUNT })
    }

    /// Левый верхний угол подарка.
    pub fn gift_position(&self) -> Option<V2> {
        self.gift.map(|g| g.position)
    }

    pub fn remove_gift(&mut self) {
        self.gift = None;
    }

    pub fn open_gift(&mut self, caps: &mut CapField) -> Option<OpenedGift> {
        let gift = self.gift.take()?;
        let center = gift.position.plus(v2(GIFT_SIZE / 2.0, GIFT_SIZE / 2.0));
        caps.spawn_puff(center, 170, (80.0, 420.0), (120.0, 620.0), (0.8, 1.5));
        Some(OpenedGift { center, phrase: PHRASES[self.rng.index(PHRASES.len())] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: f64 = 1.0 / 60.0;
    const SCREEN: Rect = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 800.0 };

    fn run(santa: &mut Santa, caps: &mut CapField, from: f64, seconds: f64, mut each: impl FnMut(&Santa, f64)) -> f64 {
        let mut now = from;
        let end = from + seconds;
        while now < end {
            now += STEP;
            santa.tick(now, SCREEN, caps);
            each(santa, now);
        }
        now
    }

    #[test]
    fn first_automatic_ride_comes_after_a_minute() {
        let (mut santa, mut caps) = (Santa::new(1), CapField::new(1));
        let now = run(&mut santa, &mut caps, 1.0, 59.0, |s, _| assert!(s.draw_item(V2::default()).is_none()));
        run(&mut santa, &mut caps, now, 2.0, |_, _| {});
        assert!(santa.draw_item(V2::default()).is_some());
    }

    #[test]
    fn disabled_setting_stops_schedule_but_not_the_menu_call() {
        let (mut santa, mut caps) = (Santa::new(2), CapField::new(2));
        santa.auto_rides = false;
        let now = run(&mut santa, &mut caps, 1.0, 120.0, |s, _| assert!(s.draw_item(V2::default()).is_none()));
        santa.ride_now(SCREEN);
        santa.tick(now + STEP, SCREEN, &mut caps);
        assert!(santa.draw_item(V2::default()).is_some());
    }

    #[test]
    fn ride_crosses_the_screen_at_mac_speed_and_ends() {
        let (mut santa, mut caps) = (Santa::new(3), CapField::new(3));
        santa.tick(1.0, SCREEN, &mut caps);
        santa.ride_now(SCREEN);
        let mut last = None;
        let mut frames = [false; SANTA_FRAME_COUNT];
        let mut ended_at = None;
        run(&mut santa, &mut caps, 1.0, 8.0, |s, now| match s.draw_item(V2::default()) {
            Some(item) => {
                last = Some(item.rect[0]);
                frames[item.frame] = true;
            }
            None => {
                ended_at.get_or_insert(now);
            }
        });
        // Путь: ширина экрана, 20 pt справа и спрайт с 20 pt слева.
        let expected = (1000.0 + 20.0 + 210.0 + 20.0) / 230.0;
        let elapsed = ended_at.unwrap_or(0.0) - 1.0;
        assert!((elapsed - expected).abs() < 0.05, "{elapsed} vs {expected}");
        assert!(last.unwrap_or(0.0) < -220.0);
        assert!(frames.iter().all(|f| *f));
    }

    #[test]
    fn call_during_a_ride_does_not_restart_it() {
        let (mut santa, mut caps) = (Santa::new(4), CapField::new(4));
        santa.tick(1.0, SCREEN, &mut caps);
        santa.ride_now(SCREEN);
        run(&mut santa, &mut caps, 1.0, 1.0, |_, _| {});
        let before = santa.draw_item(V2::default()).map(|d| d.rect[0]);
        santa.ride_now(SCREEN);
        assert_eq!(before, santa.draw_item(V2::default()).map(|d| d.rect[0]));
    }

    #[test]
    fn gift_falls_bounces_and_rests_on_the_ground() {
        let (mut santa, mut caps) = (Santa::new(5), CapField::new(5));
        santa.tick(1.0, SCREEN, &mut caps);
        santa.ride_now(SCREEN);
        let mut lowest = f32::MIN;
        let mut bounced = false;
        let mut previous = None;
        run(&mut santa, &mut caps, 1.0, 8.0, |s, _| {
            if let Some(p) = s.gift_position() {
                bounced |= previous.is_some_and(|y| p.y < y);
                previous = Some(p.y);
                lowest = lowest.max(p.y);
            }
        });
        let rest = 800.0 - 4.0 - GIFT_SIZE;
        assert_eq!(santa.gift_position().map(|p| p.y), Some(rest));
        assert_eq!(lowest, rest);
        assert!(bounced);
    }

    #[test]
    fn unopened_gift_disappears_after_a_minute() {
        let (mut santa, mut caps) = (Santa::new(6), CapField::new(6));
        santa.auto_rides = false;
        santa.tick(1.0, SCREEN, &mut caps);
        santa.ride_now(SCREEN);
        let mut dropped_at = None;
        let mut gone_at = None;
        run(&mut santa, &mut caps, 1.0, 75.0, |s, now| {
            if s.gift_position().is_some() {
                dropped_at.get_or_insert(now);
            } else if dropped_at.is_some() {
                gone_at.get_or_insert(now);
            }
        });
        let life = gone_at.unwrap_or(0.0) - dropped_at.unwrap_or(0.0);
        assert!((life - 60.0).abs() < 0.05, "{life}");
    }

    #[test]
    fn opened_gift_gives_a_phrase_and_a_puff_once() {
        let (mut santa, mut caps) = (Santa::new(7), CapField::new(7));
        santa.tick(1.0, SCREEN, &mut caps);
        santa.ride_now(SCREEN);
        run(&mut santa, &mut caps, 1.0, 5.0, |_, _| {});
        let before = caps.debris_count();
        let position = santa.gift_position().unwrap_or_default();
        let opened = santa.open_gift(&mut caps);
        assert_eq!(opened.map(|o| o.center), Some(position.plus(v2(22.0, 22.0))));
        assert!(opened.is_some_and(|o| PHRASES.contains(&o.phrase)));
        assert_eq!(caps.debris_count(), before + 170);
        assert!(santa.open_gift(&mut caps).is_none());
    }

    #[test]
    fn taskbar_lifts_the_ground_only_when_visible() {
        assert_eq!(ground_level(SCREEN, None), 796.0);
        assert_eq!(ground_level(SCREEN, Some(Rect { x: 0.0, y: 752.0, w: 1000.0, h: 48.0 })), 794.0);
        assert_eq!(ground_level(SCREEN, Some(Rect { x: 0.0, y: 799.5, w: 1000.0, h: 48.0 })), 796.0);
    }

    #[test]
    fn phrase_stays_on_screen() {
        let size = v2(300.0, 40.0);
        assert_eq!(phrase_origin(v2(500.0, 700.0), size, SCREEN), v2(350.0, 626.0));
        assert_eq!(phrase_origin(v2(10.0, 700.0), size, SCREEN).x, 12.0);
        assert_eq!(phrase_origin(v2(990.0, 700.0), size, SCREEN).x, 688.0);
    }
}
