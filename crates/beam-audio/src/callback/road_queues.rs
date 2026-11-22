//! Road orientation chime and junction pip queues, mixed under `b.shared`
//! (audio.py:5656-5714). Inside the road duck bus. A2.
//!
//! The near-junction pip has its own queue so it cannot erase an orientation cue when
//! entering a road right beside a junction.

use crate::callback::block::Block;
use crate::callback::common::mix_pulse_queue;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, _v: &mut Voice) {
    if !b.snap.road_active {
        return;
    }
    let frames = b.frames;
    let mut s = b.shared.lock();
    // Orientation chime: drain the pending two-tone queue.
    if !s._road_chime_queue.is_empty() {
        mix_pulse_queue(&mut s._road_chime_queue, b.buf_l, b.buf_r, frames);
    }
    // Near-junction double pip.
    if !s._road_junction_queue.is_empty() {
        mix_pulse_queue(&mut s._road_junction_queue, b.buf_l, b.buf_r, frames);
    }
}
