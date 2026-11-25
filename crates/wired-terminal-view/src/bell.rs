//! The visual bell: the text of the pane shakes sideways, as Tabby's `terminalShakeFrames`
//! (https://github.com/Eugeny/tabby, MIT, Copyright (c) 2017 Tabby Developers): 0, +5, -5, +5,
//! 0 px at the quarters of 0.3 s, CSS `ease` between each pair of keyframes.

use std::time::Duration;

const SHAKE: Duration = Duration::from_millis(300);

const KEYFRAMES: [f32; 5] = [0., 5., -5., 5., 0.];

/// The sideways offset of the text in logical pixels `elapsed` after the bell, `None` once the
/// shake is over.
pub(crate) fn shake_offset(elapsed: Duration) -> Option<f32> {
    let t = elapsed.as_secs_f32() / SHAKE.as_secs_f32();
    if !(0. ..1.).contains(&t) {
        return None;
    }
    let segments = (KEYFRAMES.len() - 1) as f32;
    let position = t * segments;
    let index = (position.floor() as usize).min(KEYFRAMES.len() - 2);
    let (from, to) = (KEYFRAMES[index], KEYFRAMES[index + 1]);
    Some(from + (to - from) * ease(position - index as f32))
}

/// CSS `ease`, `cubic-bezier(0.25, 0.1, 0.25, 1)`, at progress `x` in 0..=1.
fn ease(x: f32) -> f32 {
    const X1: f32 = 0.25;
    const Y1: f32 = 0.1;
    const X2: f32 = 0.25;
    const Y2: f32 = 1.;
    let bezier = |s: f32, p1: f32, p2: f32| 3. * (1. - s).powi(2) * s * p1 + 3. * (1. - s) * s * s * p2 + s.powi(3);
    // The curve's x grows with s, so bisection finds the s whose x is `x`.
    let (mut lo, mut hi) = (0_f32, 1_f32);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.;
        if bezier(mid, X1, X2) < x { lo = mid } else { hi = mid }
    }
    bezier((lo + hi) / 2., Y1, Y2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Option<f32> {
        shake_offset(Duration::from_millis(ms))
    }

    #[test]
    fn passes_the_keyframes_and_ends_after_the_shake() {
        let close = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 0.01);
        assert!(close(at(0), 0.));
        assert!(close(at(75), 5.));
        assert!(close(at(150), -5.));
        assert!(close(at(225), 5.));
        assert!(at(299).is_some_and(|v| v.abs() < 0.1), "back near the start at the end");
        assert_eq!(at(300), None);
        assert_eq!(at(10_000), None);
    }

    #[test]
    fn stays_within_the_keyframes() {
        for ms in 0..300 {
            let offset = at(ms).expect("shaking");
            assert!((-5. ..=5.).contains(&offset), "{ms} ms: {offset}");
        }
    }

    #[test]
    fn ease_matches_css() {
        assert!((ease(0.) - 0.).abs() < 1e-4);
        assert!((ease(1.) - 1.).abs() < 1e-4);
        // Chrome's `ease` at 50% progress is about 0.8024.
        assert!((ease(0.5) - 0.8024).abs() < 0.002, "{}", ease(0.5));
    }
}
