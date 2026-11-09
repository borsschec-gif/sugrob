//! Снежинка для значка в трее: шесть лучей, на каждом по две веточки.

fn distance_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy).max(1e-6)).clamp(0.0, 1.0);
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

/// Покрытие пикселей 0–255, построчно, сторона `size`.
pub fn snowflake_alpha(size: usize) -> Vec<u8> {
    let center = size as f32 / 2.0;
    let radius = center - 0.5;
    let half_width = (size as f32 * 0.05).max(0.75);
    let mut segments = Vec::with_capacity(18);
    for arm in 0..6 {
        let angle = std::f32::consts::FRAC_PI_3 * arm as f32 + std::f32::consts::FRAC_PI_2;
        let at = |along: f32, turn: f32| (center + along * (angle + turn).cos(), center - along * (angle + turn).sin());
        let tip = at(radius - half_width, 0.0);
        segments.push(((center, center), tip));
        let fork = at(radius * 0.6, 0.0);
        for side in [-1.0f32, 1.0] {
            let branch = angle + side * std::f32::consts::FRAC_PI_3;
            let length = radius * 0.28;
            segments.push((fork, (fork.0 + length * branch.cos(), fork.1 - length * branch.sin())));
        }
    }
    let mut alpha = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let nearest = segments.iter().map(|s| distance_to_segment(p, s.0, s.1)).fold(f32::MAX, f32::min);
            alpha.push(((half_width + 0.5 - nearest).clamp(0.0, 1.0) * 255.0) as u8);
        }
    }
    alpha
}

#[cfg(test)]
mod tests {
    use super::snowflake_alpha;

    #[test]
    fn has_right_size_and_empty_corners() {
        for size in [16usize, 20, 24, 32, 48] {
            let alpha = snowflake_alpha(size);
            assert_eq!(alpha.len(), size * size);
            assert_eq!(alpha[0], 0);
            assert_eq!(alpha[size * size - 1], 0);
            assert_eq!(alpha[size * (size / 2) + size / 2], 255);
        }
    }

    #[test]
    fn is_mirror_symmetric() {
        let size = 32;
        let alpha = snowflake_alpha(size);
        for y in 0..size {
            for x in 0..size {
                let (a, b) = (alpha[y * size + x] as i32, alpha[y * size + size - 1 - x] as i32);
                assert!((a - b).abs() <= 2, "{x},{y}: {a} != {b}");
            }
        }
    }
}
