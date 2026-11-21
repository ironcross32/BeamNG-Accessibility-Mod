//! Slam, entry and approach cues, centred and unducked (audio.py:6818-6867).
//!
//! All three sit outside the dock gate on purpose: a cue that started as the instrument
//! was switched off still finishes rather than being cut mid-note.

use crate::callback::block::Block;
use crate::callback::common::play_centred;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let frames = b.frames;
    let w = b.waves;

    // Slam gate earcon.
    if v._slam_cue_pos >= 0.0 {
        match w.SLAM_CUE_WAVEFORMS.get(v._slam_cue_kind) {
            None => v._slam_cue_pos = -1.0,
            Some(wf) => play_centred(b.buf_l, b.buf_r, wf, &mut v._slam_cue_pos, frames),
        }
    }

    // Entry gate earcon.
    if v._entry_cue_pos >= 0.0 {
        match w.ENTRY_CUE_WAVEFORM.as_ref() {
            None => v._entry_cue_pos = -1.0,
            Some(wf) => play_centred(b.buf_l, b.buf_r, wf, &mut v._entry_cue_pos, frames),
        }
    }

    // Ramp acquisition handoff: independent of the sustained dock gate so the two-pip
    // figure always finishes once its edge has fired.
    if v._dock_approach_cue_pos >= 0.0 {
        match w.DOCK_APPROACH_CUE_WAVEFORM.as_ref() {
            None => v._dock_approach_cue_pos = -1.0,
            Some(wf) => play_centred(b.buf_l, b.buf_r, wf, &mut v._dock_approach_cue_pos, frames),
        }
    }
}
