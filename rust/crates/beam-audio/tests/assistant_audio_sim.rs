//! `diagnostic/assistant_audio_sim.py`: spatial turn slots and the independent
//! disengagement alarm, with the real MIT KEMAR HRTF set.

use std::sync::Arc;

use beam_audio::AudioController;
use beam_audio::callback::assistant_cues::mix_assistant_cues;
use beam_base::pyval::from_json_map;
use serde_json::json;

fn render(audio: &AudioController) -> (Vec<f32>, Vec<f32>) {
    let (mut all_l, mut all_r) = (Vec::new(), Vec::new());
    loop {
        let mut s = audio.lock.lock();
        if s._assistant_cue_queue.is_empty() {
            break;
        }
        let (mut left, mut right) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        mix_assistant_cues(&mut s, &mut left, &mut right, 512);
        all_l.extend(left);
        all_r.extend(right);
    }
    (all_l, all_r)
}

fn energy(x: &[f32]) -> f32 {
    x.iter().map(|v| v * v).sum()
}

#[test]
fn assistant_audio_sim() {
    let audio: Arc<AudioController> = AudioController::new();
    let cfg = json!({"hrtf_enabled": true});
    audio
        .apply_config(&from_json_map(cfg.as_object().unwrap()))
        .expect("apply_config");
    let path = beam_testutil::repo_root().join("hrtf_mit_kemar.npz");
    audio.load_hrtf(&path, None, None, None);
    assert!(audio.lock.lock()._hrtf.is_some());
    let sample_rate = audio.samplerate();

    let cases: [&[&str]; 3] = [
        &["left", "right"],
        &["straight", "right"],
        &["left", "straight", "right"],
    ];
    for choices in cases {
        audio.trigger_assistant_cue("junction", choices);
        assert_eq!(audio.lock.lock()._assistant_cue_queue.len(), choices.len());
        let (left, right) = render(&audio);
        for (index, direction) in ["left", "straight", "right"].iter().enumerate() {
            let i = index as f64;
            let start = (i * 0.24 * sample_rate).trunc() as usize;
            let end = ((i * 0.24 + 0.20) * sample_rate).trunc() as usize;
            let slice = |x: &[f32]| x[start.min(x.len())..end.min(x.len())].to_vec();
            let l_energy = energy(&slice(&left));
            let r_energy = energy(&slice(&right));
            if !choices.contains(direction) {
                assert_eq!(l_energy + r_energy, 0.0, "Missing exits must be silent");
            } else {
                assert!(l_energy + r_energy > 0.0, "{choices:?}: {direction} silent");
                if *direction == "left" {
                    assert!(l_energy > r_energy, "{choices:?}: left not left-heavy");
                } else if *direction == "right" {
                    assert!(r_energy > l_energy, "{choices:?}: right not right-heavy");
                }
            }
        }
    }

    audio.trigger_assistant_cue("junction", &["left", "right"]);
    audio.trigger_assistant_cue("off", &[]);
    audio.set_road_mode(false);
    let (left, right) = render(&audio);
    assert_eq!(left, right, "Off must cancel spatial turn cues");
    let at = |s: f64| ((sample_rate * s).trunc() as usize).min(left.len());
    assert!(left[..at(0.23)].iter().any(|&x| x != 0.0));
    assert!(!left[at(0.23)..at(0.31)].iter().any(|&x| x != 0.0));
    assert!(left[at(0.31)..].iter().any(|&x| x != 0.0));
}
