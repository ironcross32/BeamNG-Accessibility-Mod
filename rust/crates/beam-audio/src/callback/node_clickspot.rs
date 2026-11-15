//! Node grabber hover beep and clickspot hover beep (audio.py:5243-5287). The node beep's
//! pitch follows the node's height (resampled from the base beep); either plays its
//! reversed waveform when the pointer leaves.

use crate::callback::block::Block;
use crate::callback::common::{play_centred, play_resampled};
use crate::consts::NODE_BEEP_BASE_FREQ_HZ;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let w = b.waves;
    if v._node_beep_playback_pos >= 0.0 && w.NODE_BEEP_WAVEFORM.is_some() {
        let beep = if v._node_beep_reverse {
            &w.NODE_BEEP_REV_WAVEFORM
        } else {
            &w.NODE_BEEP_WAVEFORM
        };
        // Both are generated together; Python would raise on a missing reverse beep.
        if let Some(beep) = beep {
            let pitch_mult = v._node_beep_freq / NODE_BEEP_BASE_FREQ_HZ;
            play_resampled(
                b.buf_l,
                b.buf_r,
                beep,
                beep,
                1.0,
                1.0,
                &mut v._node_beep_playback_pos,
                pitch_mult,
                b.frames,
            );
        }
    }

    if v._clickspot_beep_playback_pos >= 0.0 {
        let beep = if v._clickspot_beep_reverse {
            &w.CLICKSPOT_BEEP_REV_WAVEFORM
        } else {
            &w.CLICKSPOT_BEEP_WAVEFORM
        };
        if let Some(beep) = beep {
            play_centred(
                b.buf_l,
                b.buf_r,
                beep,
                &mut v._clickspot_beep_playback_pos,
                b.frames,
            );
        }
    }
}
