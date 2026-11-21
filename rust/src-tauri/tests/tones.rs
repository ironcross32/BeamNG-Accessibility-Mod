//! Moved out of `src/tones.rs`: see build.rs for why this crate's tests are integration tests.

use beam_app::tones::*;

#[test]
fn test_tone_shape() {
    let w = test_tone(1000.0, 0.0);
    assert_eq!(w.len(), 36000);
    // t = 0 is the triangle's peak: 2*|0-1|-1 = 1.
    assert_eq!(w[0], 1.0);
    assert_eq!(*w.last().unwrap(), 0.0);
    assert!(w.iter().all(|x| x.abs() <= 1.0));
}

#[test]
fn gear_preview_length_and_content() {
    let b = gear_preview("C", 4, "major", -12.0);
    assert_eq!(b.len(), 14400 * 10 + 12000);
    assert!(b.iter().any(|x| *x != 0.0));
}
