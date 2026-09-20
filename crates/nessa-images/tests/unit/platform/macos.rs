//! Undoing Core Graphics' premultiplied alpha, pixel by pixel.
//!
//! The only arithmetic in the macOS adapter that changes what a pixel says, and
//! the one part of it no real file exercises: every photograph ImageIO is asked
//! about here is opaque. `tests/macos.rs` drives the adapter against files the
//! system writes; this covers what those files never contain.
use super::straightened;

/// What the adapter undoes: a colour channel as Core Graphics stored it, scaled
/// down by the alpha it was drawn with, rounded to nearest.
fn premultiplied(channel: u8, alpha: u8) -> u8 {
    ((u32::from(channel) * u32::from(alpha) + 127) / 255) as u8
}

#[test]
fn a_pixel_that_was_never_multiplied_is_handed_back_as_it_is() {
    for pixel in [
        [0, 0, 0, 255],
        [12, 34, 56, 255],
        [255, 255, 255, 255],
        // Nothing was drawn here, so there is nothing to divide by and the
        // colour channels are whatever the buffer happened to hold.
        [0, 0, 0, 0],
        [200, 100, 50, 0],
    ] {
        assert_eq!(straightened(pixel), pixel, "{pixel:?}");
    }
}

#[test]
fn a_see_through_pixel_is_divided_by_its_own_alpha() {
    // Half alpha halves each channel, so straightening doubles it back.
    assert_eq!(straightened([64, 32, 16, 128]), [128, 64, 32, 128]);
    // The extremes of see-through: one step from clear, and one from opaque.
    assert_eq!(straightened([1, 0, 1, 1]), [255, 0, 255, 1]);
    assert_eq!(straightened([254, 127, 0, 254]), [255, 128, 0, 254]);
}

#[test]
fn every_channel_survives_the_round_trip_within_a_step() {
    // Premultiplying loses precision, so an exact round trip is not on offer.
    // What is promised is that nothing drifts further than the rounding of the
    // two divisions, at every alpha that divides at all.
    for alpha in 1..u8::MAX {
        for channel in 0..=u8::MAX {
            let stored = premultiplied(channel, alpha);
            let [recovered, ..] = straightened([stored, 0, 0, alpha]);
            let step = (255_u32.div_ceil(u32::from(alpha))) as i32;
            let drift = i32::from(recovered) - i32::from(channel);
            assert!(
                drift.abs() <= step,
                "alpha {alpha}, channel {channel}: {stored} became {recovered}"
            );
        }
    }
}

#[test]
fn a_channel_brighter_than_its_alpha_is_held_at_full_rather_than_wrapping() {
    // Core Graphics should never produce one, and a buffer this crate did not
    // write might. Division would answer above 255, which no byte holds.
    for alpha in [1, 127, 254] {
        assert_eq!(straightened([255, 255, 255, alpha]), [255, 255, 255, alpha]);
    }
    // Just over its alpha, which is the first value that has to be held.
    assert_eq!(straightened([2, 2, 2, 1]), [255, 255, 255, 1]);
    assert_eq!(straightened([128, 130, 200, 128]), [255, 255, 255, 128]);
}

#[test]
fn a_whole_buffer_is_straightened_pixel_by_pixel() {
    let mut buffer = vec![64, 32, 16, 128, 7, 8, 9, 255, 0, 0, 0, 0];
    super::straighten_alpha(&mut buffer);
    assert_eq!(buffer, vec![128, 64, 32, 128, 7, 8, 9, 255, 0, 0, 0, 0]);
}
