//! `diagnostic/junction_proximity_sim.py`, the AudioController stop-bar block (rendering,
//! freshness, mode gates), in the same order. The wire-validation, MCP snapshot and
//! recorder checks that follow it in the Python test `road_guidance.py`, `beamtel.py`
//! and `assistant_diagnostics.py` and live with those ports.
//!
//! Python renders into float64 buffers; `_mix_junction_proximity` adds the same float32
//! wave either way, so the float32 buffers here hold identical values.

mod audio_harness;

use audio_harness::{config_from_json, harness};
use beam_audio::AudioController;
use beam_audio::consts::{STOP_BAR_HZ, STOP_BAR_SUB_HZ, StopBarState};
use beam_audio::controller::JsonMap;
use serde_json::{Value, json};

struct Sim {
    h: audio_harness::Harness,
    sr: f64,
}

impl Sim {
    fn c(&self) -> &AudioController {
        &self.h.ctrl
    }

    fn render(&self, duration: f64) -> Vec<f64> {
        let n = (self.sr * duration).trunc() as usize;
        let mut left = vec![0.0f32; n];
        let mut right = vec![0.0f32; n];
        self.c().mix_junction_proximity(&mut left, &mut right, n);
        assert_eq!(left, right, "distance cue stays centred");
        left.into_iter().map(f64::from).collect()
    }

    fn starts(&self, wave: &[f64]) -> Vec<f64> {
        let idx: Vec<usize> = (0..wave.len()).filter(|&i| wave[i].abs() > 0.002).collect();
        let mut out = Vec::new();
        for (j, &i) in idx.iter().enumerate() {
            if j == 0 || (i - idx[j - 1]) as f64 > self.sr * 0.015 {
                out.push(i as f64 / self.sr);
            }
        }
        out
    }

    fn update(&self, target: Option<&JsonMap>) {
        self.c().update_junction_proximity(target, true);
    }

    fn mode(&self) -> &'static str {
        self.c().lock.lock()._stop_bar_mode.name()
    }
}

fn rms(wave: &[f64]) -> f64 {
    (wave.iter().map(|x| x * x).sum::<f64>() / wave.len() as f64).sqrt()
}

fn any(wave: &[f64]) -> bool {
    wave.iter().any(|&x| x != 0.0)
}

fn diff(x: &[f64]) -> Vec<f64> {
    x.windows(2).map(|w| w[1] - w[0]).collect()
}

fn obj(v: Value) -> JsonMap {
    v.as_object().expect("object").clone()
}

fn target() -> JsonMap {
    obj(json!({"id": "junction:T", "distance": 120, "speed": 20,
               "action": "approach", "source": "junctionBoundary"}))
}

/// `dict(target, distance=max(0.0, signed), signedDistance=signed, speed=speed)`.
fn at(signed: f64, speed: f64) -> JsonMap {
    let mut t = target();
    t.insert("distance".into(), json!(signed.max(0.0)));
    t.insert("signedDistance".into(), json!(signed));
    t.insert("speed".into(), json!(speed));
    t
}

fn with(mut t: JsonMap, key: &str, value: Value) -> JsonMap {
    t.insert(key.into(), value);
    t
}

#[test]
fn junction_proximity_stop_bar() {
    let cfg = config_from_json(&json!({"road_junction_volume_db": -14}));
    let h = harness(cfg.as_ref());
    let sr = h.ctrl.samplerate();
    let sim = Sim { h, sr };
    let c = sim.c();

    // patch("audio.time.monotonic", return_value=100): the harness clock starts there.
    sim.h.clock.set(100.0);

    // With road mode off, assist can still enable the shared cue.
    c.set_road_mode(false);
    c.update_junction_proximity(Some(&target()), true);
    let far = sim.starts(&sim.render(4.0));
    assert!(
        far.len() == 3 && 1.45 < far[1] - far[0] && far[1] - far[0] < 1.55,
        "{far:?}"
    );
    sim.update(Some(&at(12.0, 20.0)));
    let near = sim.starts(&sim.render(1.0));
    assert!(
        0.17 < near[1] - near[0] && near[1] - near[0] < 0.19,
        "pulses quicken to a distinct 90 ms on, 90 ms off"
    );

    // A slow approach with a fresh distance every 50 ms: the tempo rises smoothly and no
    // pulse is cut short by an update.
    sim.update(None);
    sim.render(0.05);
    let mut approach = Vec::new();
    for step in 0..360 {
        sim.update(Some(&at(20.0 - step as f64 * 0.05, 1.0)));
        approach.extend(sim.render(0.05));
    }
    let idx: Vec<usize> = (0..approach.len())
        .filter(|&i| approach[i].abs() > 0.002)
        .collect();
    let breaks: Vec<usize> = (0..idx.len() - 1)
        .filter(|&j| (idx[j + 1] - idx[j]) as f64 > sr * 0.015)
        .collect();
    let mut begins = vec![idx[0]];
    begins.extend(breaks.iter().map(|&b| idx[b + 1]));
    let mut ends: Vec<usize> = breaks.iter().map(|&b| idx[b]).collect();
    ends.push(idx[idx.len() - 1]);
    let lengths: Vec<f64> = begins
        .iter()
        .zip(&ends)
        .map(|(&b, &e)| (e - b) as f64 / sr)
        .collect();
    let begin_s: Vec<f64> = begins.iter().map(|&b| b as f64 / sr).collect();
    let intervals = diff(&begin_s);
    assert!(
        lengths.len() > 20 && lengths[..lengths.len() - 1].iter().all(|&l| l > 0.075),
        "{}",
        lengths.iter().cloned().fold(f64::INFINITY, f64::min)
    );
    assert!(
        diff(&intervals).iter().all(|&d| d < 0.02),
        "tempo only ever rises on approach"
    );

    // On the bar: one unbroken tone, D#3 over A2 at half its level.
    sim.update(Some(&at(0.0, 5.0)));
    sim.render(0.1);
    let solid = sim.render(1.0);
    assert_eq!(sim.starts(&solid).len(), 1, "solid while on the bar");
    let amplitude = |hz: f64| -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (k, &x) in solid.iter().enumerate() {
            let ph = -2.0 * std::f64::consts::PI * hz * (k as f64 / sr);
            re += x * ph.cos();
            im += x * ph.sin();
        }
        let n = solid.len() as f64;
        2.0 * (re / n).hypot(im / n)
    };
    let (top, sub) = (amplitude(STOP_BAR_HZ), amplitude(STOP_BAR_SUB_HZ));
    assert!(
        (STOP_BAR_HZ - 311.13).abs() < 0.01 && (STOP_BAR_SUB_HZ - 155.56).abs() < 0.01,
        "D#4 over D#3"
    );
    assert!(0.48 < sub / top && sub / top < 0.52, "sub oscillator at half volume");
    assert!(
        (rms(&solid).powi(2) / ((top * top + sub * sub) / 2.0) - 1.0).abs() < 0.02,
        "nothing but the two oscillators"
    );
    let moving_level = rms(&solid);

    // Stopped on the bar: still solid, 18 dB down after the fade.
    sim.update(Some(&at(-0.5, 0.0)));
    sim.render(1.0);
    let stopped = sim.render(1.0);
    assert_eq!(sim.starts(&stopped).len(), 1, "still solid once stopped");
    let drop = 20.0 * (rms(&stopped) / moving_level).log10();
    assert!(-18.5 < drop && drop < -17.5, "stopped fade is {drop:.1} dB");
    // Pulling away past the line: a short release, not a click, then silence.
    sim.update(Some(&at(0.3, 0.0)));
    sim.render(0.1);
    sim.update(Some(&at(0.3, 3.0)));
    sim.render(1.0);
    assert!(
        (rms(&sim.render(0.5)) / moving_level - 1.0).abs() < 0.02,
        "full level once moving again"
    );
    let before = c.lock.lock()._stop_bar_env;
    sim.update(Some(&at(-0.2, 3.0)));
    let mut release = Vec::new();
    for _ in 0..10 {
        release.extend(sim.render(0.004));
    }
    assert!(before == 1.0 && c.lock.lock()._stop_bar_env == 0.0);
    assert!(
        diff(&release).iter().all(|d| d.abs() < 0.02),
        "the release does not click"
    );
    let audible: Vec<usize> = (0..release.len())
        .filter(|&i| release[i].abs() > 1e-4)
        .collect();
    let last = *audible.last().expect("audible release") as f64 / sr;
    assert!(0.015 < last && last < 0.021, "released over about 20 ms");
    assert!(!any(&sim.render(0.5)), "silent once departing past the line");
    sim.update(Some(&at(-1.5, 0.0)));
    sim.render(0.1);
    assert!(
        !any(&sim.render(0.5)),
        "stopping just past the line does not bring the bar back"
    );

    // Rolled onto the bar and stopped past the line: the quiet solid tone holds.
    sim.update(Some(&with(at(0.2, 0.0), "id", json!("junction:S"))));
    sim.render(0.1);
    sim.update(Some(&with(at(-0.6, 0.1), "id", json!("junction:S"))));
    sim.render(0.1);
    assert_eq!(sim.mode(), "solid", "stopped past the line still on the bar");
    sim.update(Some(&with(at(-0.6, 3.0), "id", json!("junction:S"))));
    sim.render(0.1);
    assert_eq!(sim.mode(), "silent", "pulling away from it is departing");

    sim.update(Some(&at(-3.0, 3.0)));
    sim.render(0.05);
    assert!(!any(&sim.render(0.5)), "silent once the nose has cleared the bar");
    // Distance jitter at the edge of the bar does not flip it once on it.
    sim.update(Some(&with(at(0.3, 1.0), "id", json!("junction:edge"))));
    sim.render(0.1);
    for signed in [0.9, 0.4, 1.4, 0.6, 1.2] {
        sim.update(Some(&with(at(signed, 1.0), "id", json!("junction:edge"))));
        sim.render(0.1);
        assert_eq!(sim.mode(), "solid", "flipped off the bar at {signed} m");
    }
    sim.update(Some(&with(at(2.0, 0.0), "id", json!("junction:edge"))));
    sim.render(0.05);
    assert!(!any(&sim.render(0.5)), "silent once reversed back off the bar");
    // Stopped short of a bar not yet reached: one quiet pulse every 2 s.
    sim.update(Some(&with(at(10.0, 0.0), "id", json!("junction:W"))));
    let waiting = sim.render(4.2);
    let w = sim.starts(&waiting);
    assert!(
        w.len() == 3 && diff(&w).iter().all(|d| (d - 2.0).abs() < 0.05),
        "{w:?}"
    );
    let third = &waiting[(w[2] * sr).trunc() as usize..((w[2] + 0.12) * sr).trunc() as usize];
    let drop = 20.0 * (rms(third) / moving_level).log10();
    assert!(-19.5 < drop && drop < -16.5, "waiting pulse level {drop:.1} dB");
    for speed in [0.35, 0.1, 0.45, 0.3] {
        sim.update(Some(&with(at(10.0, speed), "id", json!("junction:W"))));
        sim.render(0.1);
        assert_eq!(
            sim.mode(),
            "waiting",
            "creeping noise does not restart the pulses"
        );
    }
    sim.update(Some(&at(10.0, 0.8)));
    sim.render(0.1);
    assert!(
        matches!(c.lock.lock()._stop_bar_mode, StopBarState::Pulse(_)),
        "pulses resume once really moving"
    );
    sim.update(Some(&with(
        with(at(12.0, 20.0), "source", json!("stopPoint")),
        "action",
        json!("stop"),
    )));
    // Same target id, so the pulse clock carries on: skip the pulse in progress.
    let st = sim.starts(&sim.render(1.0));
    let d1 = diff(&st)[1];
    assert!(0.17 < d1 && d1 < 0.19, "stop signs use the same bar");

    c.trigger_assistant_cue("junction", &["left", "right"]);
    assert!(
        c.lock.lock()._junction_proximity.is_some(),
        "exit tones do not cancel the stop bar"
    );
    c.update_junction_proximity(Some(&target()), false);
    sim.render(0.05);
    assert!(!any(&sim.render(1.0)), "neither driving mode is enabled");
    c.set_road_mode(true);
    sim.update(Some(&target()));
    assert!(any(&sim.render(0.1)), "manual road guidance");
    // A private-road entry is spoken, not a place to stop: no bar.
    sim.update(Some(&with(target(), "kind", json!("privateEntry"))));
    sim.render(0.05);
    assert!(!any(&sim.render(1.0)), "a private-road entry sounds no bar");
    sim.update(Some(&target()));
    c.clear_road_audio();
    sim.render(0.05);
    assert!(!any(&sim.render(1.0)), "loading or off-road reset");
    sim.update(Some(&target()));

    // patch("audio.time.monotonic", return_value=101)
    sim.h.clock.set(101.0);
    assert!(!any(&sim.render(1.0)), "dead UDP feed silences cue");
    let off = config_from_json(&json!({"road_junction_proximity_enabled": false}));
    sim.h.ctrl.apply_config(off.as_ref().expect("config")).expect("apply_config");
    sim.update(Some(&target()));
    assert!(!any(&sim.render(1.0)), "configuration disables cue");
    eprintln!("junction_proximity_sim: stop-bar diagnostics passed");
}
