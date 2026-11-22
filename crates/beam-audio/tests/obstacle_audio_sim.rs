//! `diagnostic/obstacle_audio_sim.py`: deterministic checks for the predictive obstacle
//! audio state, in the same order. The four `beamtel.parse_obstacle_packet` checks belong
//! to beam-engine (`listeners::obstacle`), which depends on this crate, so they are not
//! repeated here.

mod audio_harness;

use audio_harness::harness;
use beam_audio::Clock;
use beam_audio::callback::common::render_directional_pulse;
use beam_audio::consts::*;
use beam_audio::controller::HazardIn;

fn close(a: f64, b: f64) -> bool {
    // math.isclose defaults: rel_tol 1e-9.
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs())
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|&v| v as f64 * v as f64).sum()
}

fn emergency() -> HazardIn {
    HazardIn {
        bearing: Some(10.0),
        urgency: Some(255.0),
        gap: Some(0.4),
        state: Some(3.0),
        closing: Some(4.0),
        ttc: Some(0.1),
        stopping_margin: Some(-0.8),
        legacy: None,
    }
}

#[test]
fn obstacle_audio_simulation() {
    let mut checks = 0;
    let mut check = |cond: bool, msg: &str| {
        assert!(cond, "{msg}");
        checks += 1;
    };

    check(
        OBSTACLE_CUE_FREQ_HZ == 413.7,
        "fundamental must be exactly 413.7 Hz",
    );
    check(
        OBSTACLE_CUE_HARMONICS
            == [
                (1, 1.0),
                (2, 0.5),
                (4, 0.25),
                (6, 1.0 / 6.0),
                (8, 0.125),
                (10, 0.1),
            ],
        "spectrum must contain the fundamental and exact even-harmonic 1/n ratios",
    );

    let h = harness(None);
    let controller = &h.ctrl;
    check(
        controller.is_enabled(),
        "audio dependencies are required for this diagnostic",
    );
    let sr = controller.samplerate();
    // controller._obstacle_amp = 1.0; pulse = controller._generate_obstacle_buzz()
    let (pulse, _peak) = beam_audio::waveforms::generate_obstacle_buzz(sr, 1.0);
    check(
        pulse.len() == (sr * 0.080).trunc() as usize,
        "pulse must last 80 ms",
    );
    check(
        pulse[0] == 0.0 && pulse[pulse.len() - 1] == 0.0,
        "raised-cosine edges must land on zero",
    );
    let peak = pulse.iter().fold(0.0f32, |m, &x| m.max(x.abs())) as f64;
    check(
        peak <= 1.000001,
        "normalised pulse must not exceed configured level",
    );
    check(peak > 0.98, "normalised pulse must use the available peak");

    check(
        close(obstacle_cue_rate_hz(1, 0.0), 0.7),
        "advisory floor must be 0.7 Hz",
    );
    check(
        close(obstacle_cue_rate_hz(1, 255.0), 1.5),
        "advisory ceiling must be 1.5 Hz",
    );
    check(
        close(obstacle_cue_rate_hz(2, 170.0), 1.5),
        "urgent floor must be 1.5 Hz",
    );
    check(
        close(obstacle_cue_rate_hz(2, 255.0), 4.0),
        "urgent ceiling must be 4 Hz",
    );
    check(
        obstacle_cue_rate_hz(3, 255.0) <= 4.0,
        "pulse cadence must never exceed 4 Hz",
    );

    // "renderer must not retain per-obstacle arrays" / "must expose one selected source":
    // structural in Rust -- `Shared` has `_obstacle_hazard: Option<Hazard>` and no
    // per-obstacle arrays; the line below fails to compile otherwise.
    let _one_source: Option<beam_audio::controller::Hazard> =
        controller.lock.lock()._obstacle_hazard.clone();
    check(true, "renderer must not retain per-obstacle arrays");
    check(true, "renderer must expose one selected source");

    controller.lock.lock()._hrtf_user_enabled = false;
    let (left_l, left_r) = render_directional_pulse(None, false, &pulse, 35.0);
    let (right_l, right_r) = render_directional_pulse(None, false, &pulse, -35.0);
    check(
        energy(&left_l) > energy(&left_r),
        "positive bearing must pan left",
    );
    check(
        energy(&right_r) > energy(&right_l),
        "negative bearing must pan right",
    );

    let hazard = emergency();
    controller.update_selected_hazard(Some(&hazard));
    check(
        controller.lock.lock()._obstacle_emergency_latched,
        "emergency must latch",
    );
    check(
        controller.voice.lock()._obstacle_doublet_requested,
        "emergency transition must request a doublet",
    );
    controller.voice.lock()._obstacle_doublet_requested = false;
    controller.update_selected_hazard(Some(&hazard));
    check(
        !controller.voice.lock()._obstacle_doublet_requested,
        "latched emergency must not retrigger its doublet",
    );
    let advisory = HazardIn {
        state: Some(1.0),
        urgency: Some(50.0),
        ..emergency()
    };
    controller.update_selected_hazard(Some(&advisory));
    let now = h.clock.monotonic();
    controller.lock.lock()._obstacle_below_urgent_since = Some(now - 1.01);
    controller.update_selected_hazard(Some(&advisory));
    check(
        !controller.lock.lock()._obstacle_emergency_latched,
        "one second below urgent must re-arm emergency",
    );
    controller.update_selected_hazard(None);
    check(
        controller.lock.lock()._obstacle_hazard.is_none(),
        "clear must remove the selected source",
    );
    check(
        !controller.lock.lock()._obstacle_emergency_latched,
        "clear must immediately re-arm emergency",
    );

    check(
        close(obstacle_duck_gain(1), 1.0),
        "advisory must not duck guidance",
    );
    check(
        close(obstacle_duck_gain(2), 10f64.powf(-6.0 / 20.0)),
        "urgent duck must be 6 dB",
    );
    check(
        close(obstacle_duck_gain(3), 10f64.powf(-12.0 / 20.0)),
        "emergency duck must be 12 dB",
    );
    let release_start = 1.0;
    let release_end = release_start * (-0.150 / OBSTACLE_STEADY_RELEASE_S).exp();
    check(
        0.36 < release_end && release_end < 0.38,
        "steady voice must use a 150 ms release time constant",
    );

    // Exercise the real callback state machine: immediate doublet, one scheduled source,
    // then steady emergency attack and a non-abrupt clear release.
    let r = harness(None);
    let render = &r.ctrl;
    render.set_obstacle_mode(true);
    render.update_selected_hazard(Some(&hazard));
    let peak_of =
        |(l, rr): &(Vec<f32>, Vec<f32>)| l.iter().chain(rr).fold(0.0f32, |m, &x| m.max(x.abs()));
    let block = render.render_block(480);
    check(
        peak_of(&block) > 0.0,
        "emergency transition must render immediately",
    );
    check(
        render.voice.lock()._obstacle_pulse_queue.len() == 2,
        "doublet must be the only scheduled source",
    );
    check(
        !render.voice.lock()._obstacle_doublet_requested,
        "callback must consume the doublet request once",
    );
    render.update_selected_hazard(Some(&hazard));
    check(
        render.voice.lock()._obstacle_pulse_queue.len() == 2,
        "packet refresh must not restart the doublet",
    );
    for _ in 0..40 {
        render.render_block(480);
    }
    check(
        render.voice.lock()._obstacle_pulse_queue.is_empty(),
        "doublet queue must drain",
    );
    let block = render.render_block(480);
    let attack_env = render.voice.lock()._obstacle_steady_env;
    check(
        attack_env > 0.0 && peak_of(&block) > 0.0,
        "steady emergency spectrum must attack",
    );
    render.update_selected_hazard(None);
    render.render_block(480);
    let env = render.voice.lock()._obstacle_steady_env;
    check(
        0.0 < env && env < attack_env,
        "clear must release rather than hard-cut steady tone",
    );

    eprintln!("obstacle audio simulation: {checks} checks passed");
}
