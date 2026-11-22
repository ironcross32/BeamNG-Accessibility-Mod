//! The centred one-shots (audio.py:4315-4332): TC tone, check-engine buzzer, oil chime,
//! describe-error buzz, low-speed standstill tone. Ported by AA (inseparable from the
//! frame's shape and too small to hand on).

use crate::callback::block::Block;
use crate::callback::common::{Wave, play_centred};
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let w = b.waves;
    let slots: [(&mut f64, &Option<Wave>); 5] = [
        (&mut v._tc_playback_pos, &w.TC_TONE_WAVEFORM),
        (&mut v._buzzer_playback_pos, &w.CHECK_ENGINE_BUZZER_WAVEFORM),
        (&mut v._chime_playback_pos, &w.OIL_CHIME_WAVEFORM),
        (
            &mut v._describe_error_playback_pos,
            &w.DESCRIBE_ERROR_WAVEFORM,
        ),
        (&mut v._ls_stop_playback_pos, &w.LS_STOP_WAVEFORM),
    ];
    for (pos, waveform) in slots {
        if *pos >= 0.0 {
            if let Some(wave) = waveform {
                play_centred(b.buf_l, b.buf_r, wave, pos, b.frames);
            }
        }
    }
}
