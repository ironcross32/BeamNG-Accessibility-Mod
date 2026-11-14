//! Module-level constants of audio.py, in source order, with their values exactly as
//! Python holds them. Python ints are `i64`, floats `f64`, tuples are arrays or tuples.
//! `tests/consts_fixtures.rs` checks every item against `fixtures/data/audio_consts.json`.
//!
//! The module-level pure functions that sit between these constants in audio.py
//! (`obstacle_cue_rate_hz`, `obstacle_duck_gain`, `stop_bar_state`, `road_beacon_rate_hz`,
//! `route_beacon_rate_hz`, `road_correction_rate_hz`) are at the end of this file.

#![allow(clippy::excessive_precision, clippy::approx_constant)]

pub const DEFAULT_SR: i64 = 48000;
// ------------------------------------------------------------
//  Audio Constants
// ------------------------------------------------------------
// Compass Click
pub const CLICK_DUR_MS: i64 = 20;
// Attitude Cues
pub const ATT_START_DEG: f64 = 8.0;
pub const ATT_STOP_DEG: f64 = 5.0;
pub const ROLL_CAP_DEG: f64 = 60.0;
pub const PITCH_CAP_DEG: f64 = 30.0;
pub const ATT_MAX_DBFS: f64 = -24.0;
pub const ATT_MIN_DBFS: f64 = -36.0;
pub const ATT_BASE_ROLL_HZ: f64 = 220.0;
pub const ATT_BASE_PITCH_HZ: f64 = 300.0;
pub const ATT_SMOOTH_MS: f64 = 80.0;
// Attitude Fade Constants (Stability Check)
/// Target gain when stable (approx -28dB drop)
pub const ATT_FADE_MIN_GAIN: f64 = 0.04;
/// Sensitivity to variation (Higher = easier to trigger max vol)
pub const ATT_FADE_SENSITIVITY: f64 = 50.0;
/// Time to reach full volume on instability
pub const ATT_FADE_ATTACK_MS: f64 = 20.0;
/// Time to fade out when stable
pub const ATT_FADE_DECAY_MS: f64 = 1250.0;
/// dB attenuation at maximum roll (farthest perceived distance)
pub const ATT_HRTF_FAR_DB: f64 = -10.0;
// Pedal Tones
pub const PEDAL_BASE_FREQ: f64 = 300.0;
pub const PEDAL_MAX_MULT: f64 = 4.0;
pub const PEDAL_SMOOTH_MS: f64 = 60.0;
// Steering Tone
pub const STEER_BASE_FREQ: f64 = 100.0;
pub const STEER_MAX_FREQ: f64 = 400.0;
pub const STEER_AMP_DB: f64 = -20.0;
// NEW: Drift Detection Constants
pub const DRIFT_FREQ_HZ: f64 = 180.0;
pub const DRIFT_AMP_DB: f64 = -15.0;
/// s; de-staircases the incoming rate at block boundaries
pub const DRIFT_RATE_TAU: f64 = 0.12;
/// s; crossfade when the drift flips sides
pub const DRIFT_PAN_TAU: f64 = 0.1;
/// s; fade-in of the tone
pub const DRIFT_ATTACK_TAU: f64 = 0.05;
/// s; fade-out of the tone
pub const DRIFT_RELEASE_TAU: f64 = 0.12;
// NEW: Vehicle Scanner Constants
/// Duration of a single beep
pub const SCANNER_BEEP_DUR_MS: i64 = 60;
/// Beeps per second at close range
pub const SCANNER_MAX_RATE_HZ: f64 = 10.0;
/// Beeps per second at long range
pub const SCANNER_MIN_RATE_HZ: f64 = 0.5;
// Half-distances are calibrated against the scanner's SURFACE GAP, not a centre-to-centre
// distance. The mod used to report the gap between the two objects' reference nodes, which
// carried roughly half a vehicle of phantom range at each end; the gap is smaller by that
// much, so these were reduced to match or a truck-to-truck pair pinned SCANNER_MAX_RATE_HZ
// from several metres out and the rate stopped carrying any information over the last, most
// useful part of the approach.
/// Half-distance at standstill (meters of gap)
pub const SCANNER_HALF_DIST_BASE_M: f64 = 13.0;
/// Additional half-distance per m/s of closing speed
pub const SCANNER_HALF_DIST_PER_MS: f64 = 2.0;
/// Intrinsic render frequency of the beep waveform (pitch reference)
pub const SCANNER_BEEP_FREQ_HZ: f64 = 1000.0;
/// Bearing threshold for alignment tone (bright waveform)
pub const SCANNER_ALIGN_THRESHOLD_DEG: f64 = 1.5;
// Scanner pitch model: frequency = base_freq * 2^(offset_oct * proximity), where proximity is 1 at
// dead-center and 0 directly behind. base_freq and offset_oct are user-configurable (see apply_config).
// proximity blends a broad linear ramp (back->front) with a sharp alignment term so the pitch climbs
// continuously AND emphasises dead-center. The decay must be wide enough that the climb is trackable
// across a usable band — a narrow spike is only sampled when a beep fires within ~1deg of center.
/// Decay constant of the sharp alignment term (~15deg usable band)
pub const SCANNER_ALIGN_PITCH_DECAY_DEG: f64 = 6.0;
/// Share of the pitch rise from the sharp alignment term vs broad ramp
pub const SCANNER_ALIGN_WEIGHT: f64 = 0.5;
/// Default base (resting) frequency, Hz
pub const SCANNER_BASE_FREQ_DEFAULT: f64 = 1000.0;
/// Default max pitch rise at alignment, octaves
pub const SCANNER_OFFSET_OCT_DEFAULT: f64 = 1.0;
/// Minimum allowed offset (half an octave above base)
pub const SCANNER_OFFSET_OCT_MIN: f64 = 0.5;
/// Maximum allowed offset (two octaves above base)
pub const SCANNER_OFFSET_OCT_MAX: f64 = 2.0;
// Beep rate is driven by CLOSING speed (rate of change of distance), not raw speed, so heading away
// from the target slows the beeps. Half-distance grows when approaching and shrinks when receding;
// clamped to a positive minimum so the exp() denominator stays valid.
// The scanner was the only continuous spatial voice in this file with no smoothing on its
// azimuth — the drift tone, the coord guidance tone and the waypoint tone all smooth theirs,
// and the comment on DRIFT_PAN_TAU spells out why ("de-staircases the incoming rate at block
// boundaries"). The bearing arrives at ~10 Hz while the callback runs at 50-100 Hz, so
// without this it steps rather than glides. Matched to DRIFT_PAN_TAU: fast enough to track a
// real turn, slow enough that a single noisy packet cannot throw the image.
/// s; EMA on the scanner bearing before it becomes an azimuth
pub const SCANNER_BEARING_TAU: f64 = 0.09;
pub const SCANNER_HALF_DIST_MIN_M: f64 = 4.0;
/// EMA factor for closing-speed estimate (per scanner packet)
pub const SCANNER_CLOSING_SMOOTH: f64 = 0.25;
/// Clamp implausible distance jumps (e.g. on target switch)
pub const SCANNER_CLOSING_MAX_MS: f64 = 60.0;
// Steering-locked steady tone: while the player is steering (non-zero input), the beep train morphs
// into a continuous HRTF-positioned tone so the target's direction can be locked in even when beeps
// are slow (far away). Returns to normal beeps as steering re-centers.
/// |steer| below this => pure beeps
pub const SCANNER_STEER_TONE_DEADZONE: f64 = 0.04;
/// |steer| at/above this => fully steady tone
pub const SCANNER_STEER_TONE_FULL: f64 = 0.45;
// Exponent on the normalised steer above the deadzone. Below 1 front-loads the morph: a linear
// ramp left the tone near-silent for the minute corrections that hold a collision course
// (6% steer was 5% level); at 0.5 that is 22%, and 10% steer is 38%.
pub const SCANNER_STEER_TONE_CURVE: f64 = 0.5;
/// per-block one-pole smoothing of the morph level (click-free)
pub const SCANNER_TONE_LEVEL_SMOOTH: f64 = 0.12;
/// peak amplitude of the steady tone
pub const SCANNER_TONE_AMP: f64 = 0.12;
// Old Cannon vertical channel. The scanner retains horizontal direction and distance; this
// centred pulse answers only elevation. Low means raise, high means lower, pulse rate grows
// with error, and silence means the ballistic elevation is within the dead zone.
pub const CANNON_AIM_STALE_S: f64 = 0.6;
pub const CANNON_AIM_DEADZONE_DEG: f64 = 0.5;
pub const CANNON_AIM_FULL_ERROR_DEG: f64 = 10.0;
pub const CANNON_AIM_RAISE_HZ: f64 = 440.0;
pub const CANNON_AIM_LOWER_HZ: f64 = 660.0;
pub const CANNON_AIM_MIN_RATE_HZ: f64 = 1.5;
pub const CANNON_AIM_MAX_RATE_HZ: f64 = 10.0;
pub const CANNON_AIM_DUTY: f64 = 0.32;
pub const CANNON_AIM_MIN_DB: f64 = -28.0;
pub const CANNON_AIM_MAX_DB: f64 = -18.0;
// Coupler Tracking Constants
/// Base frequency (E5, distinct from scanner/obstacle)
pub const COUPLER_TONE_FREQ_HZ: f64 = 660.0;
/// Volume level
pub const COUPLER_TONE_AMP_DB: f64 = -18.0;
/// Pitch multiplier at max distance
pub const COUPLER_MIN_PITCH: f64 = 0.8;
/// Pitch multiplier at close range
pub const COUPLER_MAX_PITCH: f64 = 2.0;
/// Pitch multiplier in coupling range
pub const COUPLER_BEEP_PITCH: f64 = 2.5;
/// Distance for half pitch scaling
pub const COUPLER_HALF_DIST_M: f64 = 10.0;
/// Coupling range threshold
pub const COUPLER_RANGE_M: f64 = 1.5;
/// Beep duration when in range
pub const COUPLER_BEEP_DUR_MS: i64 = 40;
/// Max beeps/sec at very close range
pub const COUPLER_BEEP_MAX_RATE: f64 = 20.0;
/// Min beeps/sec at edge of range
pub const COUPLER_BEEP_MIN_RATE: f64 = 4.0;
// Predictive obstacle cue. This spectrum is deliberately unrelated to every road-guidance
// generator: a 413.7 Hz fundamental plus only even harmonics gives the warning a stable pitch
// and a recognisable, hollow edge without reusing the old square-wave buzzer.
pub const OBSTACLE_CUE_FREQ_HZ: f64 = 413.7;
pub const OBSTACLE_CUE_HARMONICS: [(i64, f64); 6] = [
    (1, 1.0),
    (2, 0.5),
    (4, 0.25),
    (6, 1.0 / 6.0),
    (8, 0.125),
    (10, 0.1),
];
pub const OBSTACLE_CUE_DUR_MS: i64 = 80;
pub const OBSTACLE_CUE_ATTACK_MS: i64 = 8;
pub const OBSTACLE_CUE_BODY_MS: i64 = 42;
pub const OBSTACLE_CUE_RELEASE_MS: i64 = 30;
pub const OBSTACLE_DEFAULT_DB: f64 = -18.0;
pub const OBSTACLE_ADVISORY_OFFSET_DB: f64 = -6.0;
pub const OBSTACLE_STEADY_OFFSET_DB: f64 = -9.0;
pub const OBSTACLE_DOUBLET_GAP_MS: i64 = 100;
pub const OBSTACLE_BEARING_CROSSFADE_S: f64 = 0.05;
pub const OBSTACLE_STEADY_ATTACK_S: f64 = 0.05;
pub const OBSTACLE_STEADY_RELEASE_S: f64 = 0.15;
pub const OBSTACLE_REARM_S: f64 = 1.0;
pub const OBSTACLE_ADVISORY_RATE: (f64, f64) = (0.7, 1.5);
pub const OBSTACLE_URGENT_RATE: (f64, f64) = (1.5, 4.0);
pub const OBSTACLE_URGENT_DUCK_DB: f64 = -6.0;
pub const OBSTACLE_EMERGENCY_DUCK_DB: f64 = -12.0;
// Terrain Warning Constants
/// Duration of terrain warning sweep
pub const TERRAIN_SWEEP_DUR_MS: i64 = 200;
/// Volume level for terrain sweeps
pub const TERRAIN_SWEEP_AMP_DB: f64 = -14.0;
/// Descending sweep start freq
pub const DROPOFF_FREQ_START: f64 = 500.0;
/// Descending sweep end freq
pub const DROPOFF_FREQ_END: f64 = 200.0;
/// Ascending sweep start freq
pub const HILL_FREQ_START: f64 = 200.0;
/// Ascending sweep end freq
pub const HILL_FREQ_END: f64 = 500.0;
// NEW: Low Speed Detection Constants
pub const LS_CLICK_DUR_MS: i64 = 80;
pub const LS_CARRIER_HZ: f64 = 250.0;
pub const LS_MOD_HZ: f64 = 375.0;
pub const LS_MOD_INDEX_PEAK: f64 = 4.0;
pub const LS_AMP_DB: f64 = -14.0;
pub const LS_MIN_RATE_HZ: f64 = 1.0;
pub const LS_MAX_RATE_HZ: f64 = 10.0;
// Separate normalisers per direction. A car pulls far less under power than it does
// under brakes, so a single shared scale either saturates instantly when braking or
// never reaches the top rate when accelerating. Split, each direction spans the full
// LS_MIN_RATE_HZ..LS_MAX_RATE_HZ range.
pub const LS_ACCEL_FOR_MAX_RATE: f64 = 3.0;
pub const LS_DECEL_FOR_MAX_RATE: f64 = 6.0;
pub const LS_PITCH_AT_0MPH: f64 = 0.7;
pub const LS_PITCH_AT_25MPH: f64 = 1.8;
pub const LS_FADE_ATTACK_S: f64 = 0.05;
pub const LS_FADE_DECAY_S: f64 = 0.3;
pub const LS_STOP_FADE_S: f64 = 0.08;
pub const LS_HRTF_AZIMUTH: f64 = 0.0;
// Standstill confirmation tone
pub const LS_STOP_TONE_HZ: f64 = 140.0;
pub const LS_STOP_TONE_MS: i64 = 180;
pub const LS_STOP_AMP_DB: f64 = -16.0;
// Wheel slip (lockup / wheelspin) voice
/// wheels slower than the ground: locked up
pub const SLIP_LOCK_HZ: f64 = 90.0;
/// wheels faster than the ground: spinning
pub const SLIP_SPIN_HZ: f64 = 220.0;
/// amplitude modulation, gives it a scrubbing character
pub const SLIP_AM_HZ: f64 = 22.0;
pub const SLIP_AM_DEPTH: f64 = 0.6;
/// m/s of divergence at which the voice is at full level
pub const SLIP_MAG_FOR_FULL: f64 = 5.0;
// Pitch rises with the SLIP RATIO (wheel_slip.slip_ratio: divergence over the faster of
// wheel and MotionSim ground speed, 0..1), not with the absolute divergence -- 3 m/s of
// spin at 60 mph is a scrabble, 3 m/s from a standstill is all of it. Each carrier climbs
// this many octaves from its base at ratio 0 to the top at ratio 1 (spin 220 -> 440 Hz,
// lockup 90 -> 180 Hz), so the two voices never meet in pitch.
pub const SLIP_PITCH_RISE_OCT: f64 = 1.0;
/// glide, so packet-rate steps in the ratio do not zipper
pub const SLIP_PITCH_TAU_S: f64 = 0.06;
pub const SLIP_FADE_ATTACK_S: f64 = 0.04;
pub const SLIP_FADE_DECAY_S: f64 = 0.12;
pub const SLIP_AMP_DB: f64 = -18.0;
// The voices are HRTF-placed toward the direction of travel, and a 90 Hz sine carries
// almost no interaural or pinna cue -- ahead and behind would sound identical. So both
// carriers are band-limited wavetables with harmonics up to SLIP_HARMONIC_TOP_HZ, keeping
// the pitch identity: a sawtooth for spin, odd harmonics only (square-ish) for lockup.
pub const SLIP_HARMONIC_TOP_HZ: f64 = 5000.0;
pub const SLIP_TABLE_LEN: i64 = 4096;
/// the old sine + 0.35 3rd harmonic was ~0.75 RMS; brighter reads louder
pub const SLIP_TABLE_RMS: f64 = 0.6;
/// azimuth smoothing, as a unit vector so +-180 never sweeps the front
pub const SLIP_AZ_TAU_S: f64 = 0.08;
// Side-slide voice: band-passed noise, so it cannot be mistaken for either tonal voice
// and localises well. Two bands crossfaded by slide angle: darker at the threshold,
// brighter as the car gets more sideways.
pub const SLIDE_BAND_LO: (f64, f64) = (700.0, 2000.0);
pub const SLIDE_BAND_HI: (f64, f64) = (2000.0, 5500.0);
/// length of the precomputed seamless noise loop
pub const SLIDE_NOISE_S: f64 = 2.0;
pub const SLIDE_AM_HZ: f64 = 9.0;
pub const SLIDE_AM_DEPTH: f64 = 0.35;
/// matches wheel_slip.SLIDE_ENTER_DEG
pub const SLIDE_DEG_MIN: f64 = 12.0;
pub const SLIDE_DEG_FULL: f64 = 45.0;
// NEW: Heading Guidance Constants
/// Steady tone frequency
pub const GUIDANCE_FREQ_HZ: f64 = 440.0;
/// No sound if error is within +/- this amount
pub const GUIDANCE_DEADZONE_DEG: f64 = 0.5;
/// Error amount for max volume/pan
pub const GUIDANCE_FULL_SCALE_DEG: f64 = 5.0;
/// Volume just outside deadzone
pub const GUIDANCE_MIN_DBFS: f64 = -40.0;
/// Volume at full scale error
pub const GUIDANCE_MAX_DBFS: f64 = -12.0;
// Hydraulic steering misalignment tone
// Plays whenever the wheels/frame are off-centre (actual_steering != 0), regardless of
// driver input — on an articulated machine like the WL-40 the frame stays bent after you
// release the joystick, which is exactly the state this is here to report.
// actual_steering = 0.0 for all non-hydraulic vehicles → auto-detects hydraulic vehicles.
/// pitch just outside the deadzone
pub const HYDRO_STEER_LO_HZ: f64 = 220.0;
/// pitch at full lock
pub const HYDRO_STEER_HI_HZ: f64 = 440.0;
/// dBFS just outside the deadzone — audible, not shouty
pub const HYDRO_STEER_MIN_DB: f64 = -26.0;
/// dBFS at full lock
pub const HYDRO_STEER_MAX_DB: f64 = -18.0;
/// |actual_steering| below this = straight (silent)
pub const HYDRO_STEER_DEADZONE: f64 = 0.05;
/// |actual_steering| at which full amplitude/pitch is reached
pub const HYDRO_STEER_FULL: f64 = 0.85;
// How far off centre the HRTF image swings at full articulation. Not 90°: a hard-left
// image reads as "beside you", which is too wide to judge small corrections against.
// Note this scales on raw actual_steering, not the HYDRO_STEER_FULL-normalised value —
// level and pitch saturate at HYDRO_STEER_FULL, the stereo image keeps opening past it.
pub const HYDRO_STEER_MAX_AZIMUTH_DEG: f64 = 75.0;
// FM voice: the loader's own engine/hydraulics are loud, so a pure sine disappears under
// them. The C:M ratio is a clean integer near straight and drifts off it towards full
// lock, so "wrong" sounds progressively rough and inharmonic rather than merely higher.
/// exact integer -> sidebands land on harmonics (clean)
pub const HYDRO_STEER_FM_RATIO_LO: f64 = 2.0;
/// non-integer -> sidebands land off-harmonic (dirty)
pub const HYDRO_STEER_FM_RATIO_HI: f64 = 2.12;
/// modulation depth near straight — a few sidebands
pub const HYDRO_STEER_FM_INDEX_LO: f64 = 0.9;
/// at full lock — buzzy enough to cut through, not shrill
pub const HYDRO_STEER_FM_INDEX_HI: f64 = 1.4;
// Ratio and index follow norm**CURVE, not norm. Scaling them linearly made ordinary
// steering angles piercing: the ratio was already 2.38 and the centroid 1.5 kHz at
// half lock. Cubed, the ratio holds within ~0.1 of the integer through the middle of the
// range and only goes wild in the last quarter, which is where "wild" is informative.
pub const HYDRO_STEER_FM_DIRT_CURVE: f64 = 3.0;
/// seconds; envelope rise
pub const HYDRO_STEER_ATTACK_TAU: f64 = 0.02;
/// seconds; envelope fall
pub const HYDRO_STEER_REL_TAU: f64 = 0.05;
/// seconds; smoothing on the pitch-driving magnitude
pub const HYDRO_STEER_PITCH_TAU: f64 = 0.05;
// Centre-crossing click: fires once as the frame passes into the straight deadzone.
// Silence alone is ambiguous (it also means "feature idle"), so the click confirms centre.
/// re-arm once |actual| exceeds this * deadzone
pub const HYDRO_CENTER_REARM: f64 = 2.5;
/// short atonal tick; band-limited noise, so it stays clear
pub const HYDRO_CENTER_CLICK_DUR_MS: i64 = 16;
/// of the compass/cam clicks (FM tones around 900 Hz)
pub const HYDRO_CENTER_CLICK_DB: f64 = -6.0;
/// passband: above the compass band, below the "tss" region
pub const HYDRO_CENTER_CLICK_LO_HZ: f64 = 2500.0;
pub const HYDRO_CENTER_CLICK_HI_HZ: f64 = 7000.0;
// ------------------------------------------------------------
// Trailer articulation — the SAME tone, fed from a different hinge.
// ------------------------------------------------------------
// A tractor and its trailer are an articulated machine with the joint moved to the coupler,
// so the WL-40's tone answers the question already and there is nothing here but a second
// source for it. Reusing it rather than adding a second tone is the point: a driver backing a
// trailer has no attention spare for a new sound to interpret, and the two can never play at
// once (see the source selection in the callback), so there is no ambiguity to resolve.
//
// The angle arrives from beamtel already normalised onto this tone's -1..1 scale, which is why
// no mapping constant appears in the synthesis path. TRAILER_FULL_DEG is that scale factor and
// MUST match beamtel.py's; trailer_angle_sim.lua greps both files and asserts it.
pub const TRAILER_FULL_DEG: f64 = 45.0;
// How far the tone is ducked when NOT reversing. Forward, a trailer straightens itself out of
// every corner, so an un-ducked tone would sound on ordinary driving and be trained away as
// noise long before the one manoeuvre it exists for. Ducked rather than gated because a trailer
// CAN be pushed out of line going forward — on a tight turn against a kerb, or unloaded on
// camber — and going silent there would teach that silence means straight when it does not.
pub const TRAILER_FWD_DUCK_DB: f64 = -10.0;
// Seconds; the duck glides rather than steps. Selecting reverse must not click, and it lands
// well inside the gearbox's own shift time so the tone is at full volume before the vehicle
// actually moves.
pub const TRAILER_DUCK_TAU: f64 = 0.15;
// ------------------------------------------------------------
// Loader implement (bucket / forks)
// ------------------------------------------------------------
// Two tones, both driven off the implement block in the extended telemetry struct. They are
// silent on every vehicle that has no hydraulic implement (implementFlags bit 0 clear), the
// same way the hydraulic steering tone is silent on anything that self-centres.
//
// Both are CENTRE-PANNED, deliberately. The hydraulic steering tone already owns the HRTF
// slot on this machine, and the implement is dead ahead by definition — an azimuth of 0
// carries no information and would cost two np.convolve per callback plus overlap-tail
// bookkeeping for nothing.

// --- Ground proximity ---------------------------------------------------------------------
// Grit means "close to the ground". Pitch means "you are levering the machine off its
// wheels". Keeping those on two separate axes is the whole design: the carrier is FIXED
// during the approach, so a rising pitch can only ever mean lift.
/// tone starts fading in at this clearance
pub const IMPL_GROUND_FAR_M: f64 = 2.0;
/// fixed while approaching; only jacking moves it
pub const IMPL_GROUND_CARRIER_HZ: f64 = 180.0;
/// exact integer -> sidebands land on harmonics (clean)
pub const IMPL_GROUND_RATIO_BASE: f64 = 3.0;
// MUST stay below 0.5. The drift off the integer is what encodes proximity; if it reached
// 1.0 the ratio would arrive at the NEXT integer exactly at contact and the tone would sound
// clean at the very moment it means "you are about to hit", i.e. the inverse of its meaning.
pub const IMPL_GROUND_RATIO_DRIFT: f64 = 0.45;
// Curve 2.0, NOT the hydro steering tone's 3.0. There, grit is an alarm reserved for the last
// quarter of travel, so cubing is right. Here the drift IS the graded distance readout and
// has to be legible from about a metre out; cubed, 2.0 m -> 0.5 m would stay nearly clean.
pub const IMPL_GROUND_DIRT_CURVE: f64 = 2.0;
/// modulation depth far from the ground — few sidebands
pub const IMPL_GROUND_INDEX_LO: f64 = 0.6;
// Dropped from 3.2 after play-testing: getting a bucket UNDER something means working with
// it near the ground continuously, not passing through that state on the way to a dig, and
// at 3.2 the tone was harsh enough to be a reason not to. 2.2 keeps a clear gradient from
// INDEX_LO while taking about a quarter off the brightness at contact.
/// at contact — rough, but liveable to work in
pub const IMPL_GROUND_INDEX_HI: f64 = 2.2;
/// metres of machine lift that counts as "full"
pub const IMPL_GROUND_JACK_FULL_M: f64 = 0.35;
/// carrier rise across that range
pub const IMPL_GROUND_JACK_OCTAVES: f64 = 1.5;
/// modulation depth once the ratio is back on an integer
pub const IMPL_GROUND_JACK_INDEX: f64 = 1.2;
/// dBFS at the far edge of the range, AT THE DEFAULT LEVEL
pub const IMPL_GROUND_MIN_DB: f64 = -30.0;
// Also dropped 3.5 dB for the same reason. This is only the DEFAULT for the loud end —
// implement_ground_tone_dbfs overrides it, so an existing config keeps whatever it has.
/// dBFS at contact / full jack
pub const IMPL_GROUND_MAX_DB: f64 = -19.5;
// What the level map actually uses. The pair above are a play-tested SPAN, not an absolute
// floor and ceiling: the configured level sets the loud end and the quiet end follows it down.
// Anchoring the quiet end to the constant instead inverts the whole map as soon as the user
// configures a level below it — at -40 dBFS the tone came out at -30 two metres off the ground
// and -40 at contact, i.e. loudest where it had least to say.
pub const IMPL_GROUND_SPAN_DB: f64 = IMPL_GROUND_MAX_DB - IMPL_GROUND_MIN_DB;
// The gate also opens on approach speed, not only on joystick input. Driving forward into a
// pile with a motionless bucket is the implement's position changing relative to the ground,
// and going deaf there would silence exactly the approach this tone exists to report.
pub const IMPL_GROUND_CLOSING_MPS: f64 = 0.15;
// --- Streaming split ----------------------------------------------------------------------
// The two implement voices are both continuous, both overlap in register (the 180 Hz carrier
// sits 1.6 semitones under the tilt scale's 200 Hz floor, and the FM sidebands land right
// across the middle of it), and they were both rendered CENTRE. Two continuous sounds from one
// point in space fuse into a single object — the exact argument this file already makes for
// rendering the docking beat pair centre, here running the other way. "The implement is dead
// ahead, so azimuth carries no information" was true and beside the point: this split is not a
// direction cue, it is a STREAMING cue. Without it the pair was reported as one tone that
// changes character, so lift and tilt could not be told apart.
// A CONSTANT offset can never be misread as a bearing, because a bearing moves and this does
// not; and being pure gain it costs two multiplies, not the two np.convolve a real azimuth
// would. Equal-power, so the summed loudness is unchanged from the centred version.
/// 0 = centre, 1 = hard. Ground goes left, tilt goes right.
pub const IMPL_STREAM_PAN: f64 = 0.35;
/// pan -> [0, pi/2], left of centre
pub const IMPL_PAN_THETA: f64 = (1.0 - IMPL_STREAM_PAN) * 0.25 * std::f64::consts::PI;
/// ground panned LEFT, hence cos/sin...
/// `cos(IMPL_PAN_THETA)`, the value Python computes (cos is not const in Rust).
pub const IMPL_GROUND_GAIN_L: f64 = 0.8724960070727972;
/// `sin(IMPL_PAN_THETA)`, the value Python computes.
pub const IMPL_GROUND_GAIN_R: f64 = 0.4886212414969549;
/// ...and tilt is the mirror of it
pub const IMPL_TILT_GAIN_L: f64 = IMPL_GROUND_GAIN_R;
pub const IMPL_TILT_GAIN_R: f64 = IMPL_GROUND_GAIN_L;
// --- Tilt ---------------------------------------------------------------------------------
// A pure sine, not FM: against the two FM voices already in play (hydraulic steering and the
// ground tone) a clean spectrum is instantly distinguishable, and quarter-tone steps are far
// easier to count when there are no sidebands cluttering the picture.
/// exactly level
pub const IMPL_TILT_LEVEL_HZ: f64 = 400.0;
/// quarter tones; +-24 steps = +-1 octave = 200..800 Hz
pub const IMPL_TILT_STEPS_PER_OCT: i64 = 24;
// Used until both mechanical stops have actually been visited, so the tone is usable on the
// very first lift rather than needing a calibration sweep.
pub const IMPL_TILT_FALLBACK_SPAN_DEG: f64 = 50.0;
/// below this a learned span is noise, keep the fallback
pub const IMPL_TILT_MIN_SPAN_DEG: f64 = 5.0;
// A running extreme is NOT a mechanical stop. On the very first stroke it equals the CURRENT
// angle, so tilt/span is 1.0 and the step pins to the top of the scale for the whole travel —
// heard as a snap to 800 Hz on the first tilt after a load, going smooth only once a real
// stop has been passed. An extreme only counts as a stop once the implement has come back off
// it by this much.
pub const IMPL_TILT_STOP_CONFIRM_DEG: f64 = 3.0;
// ...and a newly confirmed stop is only swapped into the scale while within this of level,
// where every span maps to step 0 and the rescale is therefore inaudible. Adopting it at angle
// jumps the readout by however much the fallback span over-estimated the real travel.
pub const IMPL_TILT_SPAN_SWAP_DEG: f64 = 2.0;
/// at level, AT THE DEFAULT LEVEL — a span, see IMPL_TILT_SPAN_DB
pub const IMPL_TILT_MIN_DB: f64 = -26.0;
/// at a mechanical stop
pub const IMPL_TILT_MAX_DB: f64 = -20.0;
pub const IMPL_TILT_SPAN_DB: f64 = IMPL_TILT_MAX_DB - IMPL_TILT_MIN_DB;
// 8 ms: fast enough that a step still reads as a step, slow enough to kill the click. Do NOT
// glide this across a whole callback — that turns a discrete step into portamento and
// destroys the countability the quarter-tone scale exists for.
pub const IMPL_TILT_GLIDE_TAU: f64 = 0.008;
/// brief notch on each step change...
pub const IMPL_TILT_STEP_DIP_DB: f64 = 1.5;
/// ...which is what makes a fast sweep countable
pub const IMPL_TILT_STEP_DIP_MS: f64 = 6.0;
// --- Shared activity gate -----------------------------------------------------------------
// Both tones fade out when the implement stops moving and snap back when it moves again.
// There is no hold timer: implementActivity already folds in the ram's travel RATE, so a
// momentary joystick release mid-lift keeps the gate open on its own. If chatter ever does
// show up in play, a ~0.2 s hold before the release is the fix — don't build both.
// Schmitt trigger, not one threshold: a single level sitting in the noise makes the gate
// flutter open and shut, which is far more irritating than either state.
/// activity that opens the gate
pub const IMPL_GATE_ON: f64 = 0.06;
/// ...and the lower level it must fall back through to close
pub const IMPL_GATE_OFF: f64 = 0.02;
/// sharp ramp back in
pub const IMPL_GATE_ATTACK_TAU: f64 = 0.03;
/// ~1 s to inaudible
pub const IMPL_GATE_REL_TAU: f64 = 0.35;
/// hard cut; a one-pole never actually reaches zero
pub const IMPL_GATE_CUT: f64 = 0.02;
// Closing-rate term. The clearance only refreshes at the mod's 20 Hz, while the audio
// callback runs every ~10 ms, so the rate MUST be measured against the time actually elapsed
// since the value last changed — dividing by the callback's own dt overstates it about
// fivefold and turns raycast jitter into a phantom approach. Smoothed and decayed so a
// single noisy sample cannot latch the gate open.
/// seconds, smoothing on the measured closing rate
pub const IMPL_CLOSING_TAU: f64 = 0.25;
/// no fresh clearance for this long -> forget the estimate
pub const IMPL_CLOSING_STALE_S: f64 = 0.5;
// Frame articulation is not what you are attending to while curling a bucket, so duck the
// hydraulic steering tone while the implement is being worked.
pub const IMPL_HYDRO_DUCK_DB: f64 = -4.0;
// ------------------------------------------------------------
//  Docking instrument (loader implement alignment)
// ------------------------------------------------------------
//
// Three axes for the last few metres of lining an implement up: lateral on the stereo image,
// range on a pulse rate, vertical on a beat pair that goes to unison at alignment. Two voices
// for three dimensions, because pan and rate ride on the same pulse.
//
// The design constraint that shapes all of it is that this machine already has an
// articulation tone, a ground-proximity tone, a tilt scale and possibly the scanner running.
// So the instrument REPLACES rather than adds: while it is open the scanner is suppressed
// outright and the articulation tone is ducked hard. That is not arbitrary — articulation
// tells you what your input did, lateral offset tells you what you need, and on a final
// approach at walking pace those are the same axis expressed twice. The goal beats the means,
// and the articulation centre-crossing click still fires, so the frame reference is not lost.
// Must not exceed implementProximity.lua's REPORT_M, which is where the mod stops sending
// at all. Setting it higher does not extend the instrument — it makes it fall silent at
// REPORT_M with no fade and no explanation, because the feed simply stops.
/// beyond this the instrument is silent
pub const DOCK_MAX_RANGE_M: f64 = 6.0;
// Inside this, the vertical beat pair fades in.
//
// Originally 2 m, on the reasoning that the vertical does not matter yet while you are still
// steering. That was armchair reasoning and play-testing killed it: a loader manoeuvres at
// something like walking pace, so 2 m of gap is one to two seconds — nowhere near enough to
// set the height AND close the distance. The operator needs the vertical cue while there is
// still room to act on it, which means well before the point where the approach is committed.
pub const DOCK_BEAT_RANGE_M: f64 = 3.5;
/// carrier of the panned range/lateral voice
pub const DOCK_PULSE_FREQ_HZ: f64 = 700.0;
/// pulses per second at DOCK_MAX_RANGE_M
pub const DOCK_PULSE_MIN_HZ: f64 = 1.2;
/// ...and at contact
pub const DOCK_PULSE_MAX_HZ: f64 = 12.0;
/// fraction of each period the pulse sounds for
pub const DOCK_PULSE_DUTY: f64 = 0.35;
/// attack skirt, as a fraction of the pulse; kills the gate click
pub const DOCK_PULSE_EDGE: f64 = 0.15;
pub const DOCK_PULSE_DB: f64 = -18.0;
// Same ceiling and the same reason as HYDRO_STEER_MAX_AZIMUTH_DEG: the KEMAR set's interaural
// level difference peaks near 67.5 degrees and then FALLS about 5 dB out to 90, so mapping
// full offset to 90 makes being further off read as coming back toward centre.
pub const DOCK_MAX_AZIMUTH_DEG: f64 = 75.0;
/// lateral offset that maps to full azimuth
pub const DOCK_LATERAL_FULL_M: f64 = 1.2;
// Vertical error as a detune against a fixed reference. Alignment is UNISON: the beating
// falls to zero and the texture goes smooth. Chosen over mapping error to absolute pitch
// because unison is self-calibrating — there is no reference frequency to learn, and the ear
// resolves a beat far finer than it resolves pitch. It is also texturally unmistakable
// against the two FM voices and the pure-sine tilt scale already on this machine.
pub const DOCK_REF_HZ: f64 = 330.0;
// The detune is specified as a BEAT RATE in Hz, not as an interval in cents, and the two
// tones are placed symmetrically either side of DOCK_REF_HZ rather than one being held fixed.
// Both of those are corrections to a first version that failed in listening:
//
//   * Holding one tone fixed and sliding the other means the pair's *average* frequency moves
//     with the error. Two tones that close fuse into a single pitch percept sitting at that
//     average, so what the listener actually hears is a tone gliding up and down — a pitch
//     cue that was never intended and that completely masks the beating. Moving both by half
//     the detune in opposite directions keeps the centre pitch nailed to DOCK_REF_HZ, so the
//     ONLY thing that changes is the beat rate. That is how tuning by beats actually works.
//
//   * Specifying cents makes the audible result a function of DOCK_REF_HZ, and the first
//     cap (200 cents, ~40 Hz apart at 330 Hz) was far outside the range where beating is
//     heard as beating at all — above roughly 12-15 Hz of separation the percept turns into
//     roughness and then into two separate pitches. Naming the beat rate directly makes the
//     thing being tuned the thing being specified, and keeps it in range by construction.
/// flutter fast enough to read as "well out", still clearly a beat
pub const DOCK_BEAT_MAX_HZ: f64 = 12.0;
/// so half a metre of error saturates it
pub const DOCK_BEAT_HZ_PER_M: f64 = 24.0;
pub const DOCK_BEAT_DB: f64 = -22.0;
// Beating is signless, so above-versus-below rides on a slow tremolo whose RATE differs. The
// two rates are far enough apart to name by ear and both are well below the beat rates they
// coexist with, so tremolo and roughness stay perceptually separate.
/// implement is above the band: come down
pub const DOCK_AM_ABOVE_HZ: f64 = 5.5;
/// implement is below the band: come up
pub const DOCK_AM_BELOW_HZ: f64 = 2.0;
pub const DOCK_AM_DEPTH: f64 = 0.45;
// Implement-mode unison lock thresholds, in beat-rate terms rather than cents. Ramp mode uses
// the explicit two-axis thresholds below; this lock waveform is shared by both modes.
pub const DOCK_LOCK_BEAT_HZ: f64 = 0.5;
pub const DOCK_LOCK_REARM_BEAT_HZ: f64 = 2.5;
pub const DOCK_LOCK_DB: f64 = -8.0;
/// a descending pair; nothing else in the mod uses that figure,
pub const DOCK_LOCK_HI_HZ: f64 = 1568.0;
/// which keeps it clear of the FM clicks and the noise-burst click
pub const DOCK_LOCK_LO_HZ: f64 = 1046.0;
pub const DOCK_LOCK_DUR_MS: f64 = 110.0;
// ------------------------------------------------------------
// Ramp mode. The same four voices, driven from a different question: lining an ordinary vehicle
// up with the drive-in mouth of a ramp (the stock large_cannon) rather than lining an implement
// up with a load. Nothing about the synthesis changes — the pulse shaping, the symmetric partner
// placement, the tremolo, the lock latch and the centre rendering of the pair are all
// mode-independent by construction, which is the point of reusing the instrument at all. Only
// the scaling constants below, and what the third scalar means, vary.
//
// Must not exceed implementProximity.lua's RAMP_REPORT_M, for the same reason as above: past
// the mod's ceiling the feed stops and the instrument cuts out with no fade. The margin is now
// large (45 m feed against 25 m of tones) and that gap is deliberate rather than slack — the mod
// keeps feeding well past where anything is sonified, so the spoken readout still answers after
// the tones have faded, which is precisely the moment someone reaches for the key. 25 rather
// than the original 18 because a ~16 m machine has to be acquirable from beside it, and at 18 m
// to the MOUTH you can be three metres from the barrel and hear nothing at all.
// ramp_resolve_sim.lua scenario 10 asserts the invariant, which the implement one went years
// without.
pub const DOCK_RAMP_MAX_RANGE_M: f64 = 25.0;
// Where the yaw channel fades in. Scaled up from the implement's 3.5-of-6.0 but not
// proportionally, and for the same reason that value moved from 2.0 to 3.5 after play-testing:
// the cue has to arrive while there is still room to act on it. Below about four metres a yaw
// error can no longer be corrected without reversing out and starting again, so arriving at
// four metres is arriving too late. MEASURE THIS IN-GAME — the failure signature is finding
// yourself at the mouth already un-squarable.
pub const DOCK_RAMP_BEAT_RANGE_M: f64 = 8.0;
// The lateral offset that maps to full HRTF azimuth. The implement's 1.2 m is a bucket-width
// number and is far too tight here: on a ramp approach you are routinely three metres off centre
// at fifteen metres out, so at 1.2 the pan would be pinned hard over for the whole of the acquire
// phase — the one phase in which the pulse is the ONLY voice and the pan is therefore carrying
// everything.
pub const DOCK_RAMP_LATERAL_FULL_M: f64 = 3.0;
// On the approach line, lateral offset belongs only to pulse position. The 15 cm dead zone
// matches the spoken "centred" state; the remaining 0.15..3.0 m spans the full 75 degrees.
// During acquisition the pulse keeps the mouth's true bearing instead.
pub const DOCK_RAMP_LATERAL_DEADZONE_M: f64 = 0.15;
// Ramp squareness drives the beat pair, reaching its 12 Hz ceiling at 24 degrees. Its sign
// keeps the established vocabulary: slow tremolo means turn left, fast means turn right.
pub const DOCK_RAMP_BEAT_HZ_PER_DEG: f64 = 0.5;
// Alignment is composite, so its lock thresholds are stated directly in both axes rather
// than inferred from the beat-rate thresholds used by implement mode.
pub const DOCK_RAMP_LOCK_LATERAL_M: f64 = 0.15;
pub const DOCK_RAMP_LOCK_YAW_DEG: f64 = 3.0;
pub const DOCK_RAMP_REARM_LATERAL_M: f64 = 0.5;
pub const DOCK_RAMP_REARM_YAW_DEG: f64 = 8.0;
// ------------------------------------------------------------

pub const DOCK_ATTACK_TAU: f64 = 0.04;
pub const DOCK_REL_TAU: f64 = 0.2;
/// articulation ducked hard, not merely trimmed, while docking
pub const DOCK_HYDRO_DUCK_DB: f64 = -12.0;
/// no packet for this long and the instrument fades out
pub const DOCK_STALE_S: f64 = 0.6;
// Slam gate. Three discrete states rather than a continuous readout, because nulling is the
// wrong shape for dropping a bucket on something: you want to be decisively above it, then
// over it, then commit. Ramming is also forgiving in a way that threading tines into a pallet
// is not, so this profile is deliberately coarser and quieter than the docking one — three
// earcons and nothing sustained.
//
// Pitched so the three read as a sequence: clear is high and short, over is lower and short,
// committed rises between the two as a pair. The rise is the point — the docking lock chime
// DESCENDS, and these two are the only two-note figures in the mod, so opposite contours keep
// "arrived at the null" and "armed to drop" from ever being confused.
pub const SLAM_CLEAR_HZ: f64 = 1320.0;
pub const SLAM_OVER_HZ: f64 = 660.0;
pub const SLAM_TICK_DUR_MS: f64 = 55.0;
pub const SLAM_COMMIT_LO_HZ: f64 = 523.0;
pub const SLAM_COMMIT_HI_HZ: f64 = 1046.0;
pub const SLAM_COMMIT_DUR_MS: f64 = 150.0;
pub const SLAM_CUE_DB: f64 = -12.0;
// Entry gate. Fires once when a band the tines could not go into becomes one they can, so
// "you can go in now" arrives while the hand is still on the joystick rather than only when
// the operator thinks to tap for a readout.
//
// A SINGLE tone, deliberately: the two two-note figures in the mod are both taken and both
// mean something else (the docking lock chime descends at the vertical null, the slam commit
// rises when armed to drop), and a third contour would be one distinction too many to hold
// mid-manoeuvre. The pitch has to dodge the single-tone slots as well — the compass and cam
// clicks are FM near 900 Hz, the scanner and coupler beeps are sines near 1 kHz, and the slam
// ticks sit at 660 and 1320 — so this sits well above all of them, where nothing else lives.
// Longer than the slam ticks and with a slower decay, which is what keeps a bare sine from
// reading as another click.
pub const ENTRY_CUE_HZ: f64 = 1760.0;
pub const ENTRY_CUE_DUR_MS: f64 = 90.0;
pub const ENTRY_CUE_DB: f64 = -14.0;
// Ramp acquisition handoff: two centred 40 ms pips with 50 ms of silence between them.
pub const DOCK_APPROACH_CUE_HZ: f64 = 880.0;
pub const DOCK_APPROACH_CUE_PIP_MS: f64 = 40.0;
pub const DOCK_APPROACH_CUE_GAP_MS: f64 = 50.0;
pub const DOCK_APPROACH_CUE_DB: f64 = -12.0;
// Node Grabber Hover Beep
/// Base frequency at height 0.5
pub const NODE_BEEP_BASE_FREQ_HZ: f64 = 800.0;
/// Frequency at underside (height 0.0)
pub const NODE_BEEP_LOW_FREQ_HZ: f64 = 600.0;
/// Frequency at top (height 1.0)
pub const NODE_BEEP_HIGH_FREQ_HZ: f64 = 1400.0;
/// Duration of the beep
pub const NODE_BEEP_DUR_MS: i64 = 40;
/// Volume level
pub const NODE_BEEP_AMP_DB: f64 = -14.0;
// Clickspot Hover Beep (FM click sound)
/// Carrier frequency
pub const CLICKSPOT_BEEP_FREQ_HZ: f64 = 770.0;
/// Modulator frequency (2x carrier for metallic click)
pub const CLICKSPOT_BEEP_MOD_HZ: f64 = 1540.0;
/// FM modulation index (higher = more harmonics)
pub const CLICKSPOT_BEEP_MOD_INDEX: f64 = 4.0;
/// Duration
pub const CLICKSPOT_BEEP_DUR_MS: i64 = 18;
/// Volume
pub const CLICKSPOT_BEEP_AMP_DB: f64 = -12.0;
// Road Detection (off-road guidance beep) — F#5 carrier with FM funk modulation
/// F#5 carrier
pub const ROAD_BEEP_FREQ_HZ: f64 = 739.99;
/// sub-octave modulator (1:2 ratio — warm, harmonic)
pub const ROAD_BEEP_MOD_HZ: f64 = 369.99;
/// FM β — moderate warmth, not paint-peeling
pub const ROAD_BEEP_MOD_INDEX: f64 = 1.8;
/// pulse length
pub const ROAD_BEEP_DUR_MS: i64 = 140;
/// default volume (overridable via config)
pub const ROAD_BEEP_AMP_DB: f64 = -14.0;
/// pulses/sec when far from road
pub const ROAD_BEEP_MIN_RATE_HZ: f64 = 0.7;
/// pulses/sec when right next to road
pub const ROAD_BEEP_MAX_RATE_HZ: f64 = 4.0;
/// exponential half-distance for rate scaling
pub const ROAD_BEEP_HALF_DIST_M: f64 = 20.0;
// Route beacon (crow-flies homing on the map route's destination) — C5 carrier with an
// octave-above modulator, giving a clean bell rather than the road beep's warm growl.
//
// The timbre has to be distinct from the road beacon above because BOTH CAN SOUND AT
// ONCE and they mean opposite things: off-road, the road beep says "the nearest road is
// that way" while this says "your destination is that way", and those are frequently
// different directions. C5 also clears the occupied slots — compass/cam FM near 900 Hz,
// scanner/coupler near 1 kHz, slam ticks at 660, and the road beep itself at F#5.
/// C5 carrier
pub const ROUTE_BEACON_FREQ_HZ: f64 = 523.25;
/// octave-above modulator (2:1 — bell-like, harmonic)
pub const ROUTE_BEACON_MOD_HZ: f64 = 1046.5;
/// FM β — gentler than the road beep; this one is ambient
pub const ROUTE_BEACON_MOD_INDEX: f64 = 0.9;
/// pulse length
pub const ROUTE_BEACON_DUR_MS: i64 = 110;
/// default volume; quieter than the road beep, it runs longer
pub const ROUTE_BEACON_AMP_DB: f64 = -16.0;
// The real difference from the road beacon is not the timbre but the SCALE. The road
// beep's 20 m half-distance is meaningless over a route: every destination would sit
// pinned at the minimum rate until the last few car lengths. A route is hundreds of
// metres to kilometres, so the half-distance is two orders of magnitude larger and the
// rate floor is lower — a beacon that ticks once every two seconds is a background fact,
// which is what it should be when the destination is still a mile off.
/// pulses/sec when the destination is far away
pub const ROUTE_BEACON_MIN_RATE_HZ: f64 = 0.5;
/// pulses/sec when on top of the destination
pub const ROUTE_BEACON_MAX_RATE_HZ: f64 = 4.0;
/// exponential half-distance for rate scaling
pub const ROUTE_BEACON_HALF_DIST_M: f64 = 400.0;
// Road correction is deliberately a different auditory object from the off-road
// beacon. Short directional pips say "apply steering"; their rate reports how much
// correction remains. A directional double pip says "ease off" while retaining
// the remaining correction direction.
// This leaves silence between instructions and gives recovery a definite action point.
pub const ROAD_CORRECTION_CARRIER_HZ: f64 = 440.0;
pub const ROAD_CORRECTION_MID_HZ: f64 = 523.25;
pub const ROAD_CORRECTION_HIGH_HZ: f64 = 659.25;
pub const ROAD_CORRECTION_PIP_MS: i64 = 80;
pub const ROAD_CORRECTION_RATE_MIN_HZ: f64 = 0.85;
pub const ROAD_CORRECTION_RATE_MAX_HZ: f64 = 3.5;
pub const ROAD_CORRECTION_UNWIND_HZ: f64 = 659.25;
pub const ROAD_CORRECTION_UNWIND_PIP_MS: i64 = 55;
pub const ROAD_CORRECTION_UNWIND_GAP_MS: i64 = 55;
pub const ROAD_CORRECTION_UNWIND_REPEAT_S: f64 = 0.8;
pub const ROAD_CORRECTION_SETTLED_START_HZ: f64 = 659.25;
pub const ROAD_CORRECTION_SETTLED_END_HZ: f64 = 880.0;
pub const ROAD_CORRECTION_SETTLED_MS: i64 = 130;
pub const ROAD_CORRECTION_SETTLED_REFRACTORY_S: f64 = 2.0;
pub const ROAD_CORRECTION_MAX_AZIMUTH_DEG: f64 = 35.0;
pub const ROAD_CORRECTION_MIN_AZIMUTH_DEG: f64 = 25.0;
pub const ROAD_CORRECTION_ATTACK_S: f64 = 0.015;
pub const ROAD_CORRECTION_RELEASE_S: f64 = 0.025;
// Road Orientation Chime (one-shot two-tone cue when transitioning onto a road).
// First tone is panned to the smaller-|bearing| travel direction (more "ahead-ish") at the
// higher pitch; second tone is panned to the opposite direction at the lower pitch.
// Chosen as a clean perfect fourth so the chime is recognizable and pleasant.
/// G5 — played first, panned to forward-ish road direction
pub const ROAD_CHIME_FWD_FREQ_HZ: f64 = 783.99;
/// C5 — played second, panned to backward-ish road direction
pub const ROAD_CHIME_BACK_FREQ_HZ: f64 = 523.25;
pub const ROAD_CHIME_DUR_MS: i64 = 180;
/// silence between the two tones (raw, before HRTF tail)
pub const ROAD_CHIME_GAP_MS: i64 = 120;
pub const ROAD_CHIME_AMP_DB: f64 = -12.0;
/// silence between chime repetitions when it loops
pub const ROAD_CHIME_REPEAT_GAP_MS: i64 = 320;
// FM modulator profile: sharp click at the attack, decaying to a low sustained index. The
// modulator runs at the carrier frequency (1:1 ratio → harmonic spectrum). The peak gives the
// tone a percussive "clack" attack; the floor adds a hint of warmth/buzz on the sustain.
pub const ROAD_CHIME_MOD_INDEX_PEAK: f64 = 5.5;
pub const ROAD_CHIME_MOD_INDEX_FLOOR: f64 = 1.1;
/// 1/e decay constant — ~11 ms time constant, ~50 ms to settle
pub const ROAD_CHIME_MOD_DECAY: f64 = 90.0;
// A centred double pip confirms the near-junction decision zone. Speech carries
// the topology; this cue intentionally carries proximity only.
pub const ROAD_JUNCTION_FREQ_HZ: f64 = 1046.5;
pub const ROAD_JUNCTION_PIP_MS: i64 = 55;
pub const ROAD_JUNCTION_GAP_MS: i64 = 55;
pub const ROAD_JUNCTION_AMP_DB: f64 = -14.0;
pub const ROAD_JUNCTION_ENTRY_START_HZ: f64 = 659.25;
pub const ROAD_JUNCTION_ENTRY_END_HZ: f64 = 440.0;
pub const ROAD_JUNCTION_ENTRY_MS: i64 = 180;
// Stop bar: the line before a junction, a stop point, a gated road or a dead end.
// D#4 with a sub an octave down (D#3) at half amplitude. It pulses faster as the
// nose closes on the bar, holds solid while the nose is on it, and drops 18 dB
// once the car has stopped there. The bar runs from REACH before the line to
// CLEAR past it; the mod reports the same CLEAR as STOP_BAR_CLEAR_M. Past the
// line it only sounds while stopped: moving past it is departing, and the tone
// releases over RELEASE_S, just long enough not to click.
/// `440.0 * 2.0 ** (-6 / 12)`, the value Python computes.
pub const STOP_BAR_HZ: f64 = 311.1269837220809;
pub const STOP_BAR_SUB_HZ: f64 = STOP_BAR_HZ / 2.0;
pub const STOP_BAR_SUB_LEVEL: f64 = 0.5;
pub const STOP_BAR_REACH_M: f64 = 0.5;
pub const STOP_BAR_CLEAR_M: f64 = 2.5;
// Front-node distance and speed both jitter. Without these margins a car creeping
// or parked at the edge of the bar flips between pulse, solid and silence.
/// once on the bar, it takes this much backing off to leave
pub const STOP_BAR_HOLD_M: f64 = 1.0;
pub const STOP_BAR_STOPPED_MPS: f64 = 0.2;
pub const STOP_BAR_MOVING_MPS: f64 = 0.5;
/// `10.0 ** (-18.0 / 20.0)`, the value Python computes.
pub const STOP_BAR_STOPPED_GAIN: f64 = 0.12589254117941673;
pub const STOP_BAR_FADE_S: f64 = 0.6;
pub const STOP_BAR_EDGE_S: f64 = 0.008;
pub const STOP_BAR_RELEASE_S: f64 = 0.02;
// 90 ms on, 90 ms off at the fastest: any quicker and it blurs into the solid tone.
pub const STOP_BAR_MIN_PERIOD_S: f64 = 0.18;
pub const STOP_BAR_MAX_PERIOD_S: f64 = 1.5;
// Stopped short of a bar: one short pulse at the stopped level every 2 s, so the
// bar ahead is not forgotten, without the urgency of the approach pulses.
pub const STOP_BAR_WAIT_PERIOD_S: f64 = 2.0;
pub const STOP_BAR_WAIT_ON_S: f64 = 0.15;
// Coordinate Guidance FM tone
/// Carrier Hz when on course (amp is ~0 anyway)
pub const COORD_GUIDE_FC_ONCOURSE_HZ: f64 = 440.0;
/// Carrier Hz when 180° off course
pub const COORD_GUIDE_FC_OFFCOURSE_HZ: f64 = 220.0;
/// fm/fc ratio when on course (quasi-harmonic)
pub const COORD_GUIDE_FM_RATIO_MIN: f64 = 0.25;
/// fm/fc ratio when 180° off (golden ratio → inharmonic)
pub const COORD_GUIDE_FM_RATIO_MAX: f64 = 1.618;
/// FM β (modulation index) at 180° off course
pub const COORD_GUIDE_MOD_INDEX_MAX: f64 = 3.0;
/// Peak amplitude at 180° off course
pub const COORD_GUIDE_AMP_DB: f64 = -12.0;
/// Silent within this bearing error
pub const COORD_GUIDE_DEADZONE_DEG: f64 = 5.0;
// Free-camera line-up guide (camera_lineup.py computes the parameters; see
// docs/camera-lineup.md). Peak level at full error, parameter smoothing, and the age-out
// that silences a feed that stopped arriving (the GE side sends at 20 Hz).
pub const LINEUP_AMP_DB: f64 = -12.0;
pub const LINEUP_SMOOTH_S: f64 = 0.12;
// Position glides slower than loudness: the bearing follows the steering directly and a
// head that is being swung around every few frames cannot be localised at all.
pub const LINEUP_AZ_SMOOTH_S: f64 = 0.25;
pub const LINEUP_STALE_S: f64 = 0.5;
// Placement Pings (vehicle spawner 3D placement editor).
// Five rungs, one per step magnitude (1 / 10 / 100 / 1000 / 10000 units). The carrier
// descends and the modulation gets rougher as the step grows, so a bigger jump literally
// sounds heavier. The fm/fc ratios are deliberately far apart — 1:1 harmonic, 2:1 harmonic,
// irrational (inharmonic clang), 3:1 harmonic, and a sub-octave — so the five timbres stay
// distinguishable even when they fire back-to-back while a key is held.
// (fc_hz, fm_ratio, beta_start, beta_end, dur_ms)
/// ones tens hundreds thousands ten-thousands (beta sweeps down over the note)
/// (fc_hz, fm_ratio, beta_start, beta_end, dur_ms)
pub const PLACEMENT_PING_RUNGS: [(f64, f64, f64, f64, f64); 5] = [
    (1600.0, 1.0, 0.4, 0.4, 22.0),  // ones
    (1200.0, 2.0, 1.2, 1.2, 28.0),  // tens
    (880.0, 1.414, 2.4, 2.4, 34.0), // hundreds
    (620.0, 3.0, 3.5, 3.5, 42.0),   // thousands
    (440.0, 0.5, 5.0, 1.0, 55.0),   // ten-thousands (beta sweeps down over the note)
];
pub const PLACEMENT_PING_AMP_DB: f64 = -12.0;
// ------------------------------------------------------------
//  Terrain sonification scanner
// ------------------------------------------------------------
//
// One-shot picture of the landscape, rather than a warning about it. The obstacle detector
// says what you are about to hit and the docking instrument says what you are lining up
// with; neither tells you that you are in a valley, on a ridge, or beside a lake. A press
// fires one scan of the forward hemisphere and it plays as a granular cloud.
//
// Three mappings, and they are deliberately on three different axes so none of them has to
// share a channel with another:
//   PITCH is elevation. The ground under your own wheels is middle C, and everything else
//     is read against it. That is only usable if the listener has a reference immediately
//     beforehand, which is what the opening ping is for.
//   TIME is distance. Every scan takes exactly SCAN_DURATION_S no matter how far it
//     reached, so "half a second in" always means the same fraction of the way out. A fixed
//     duration is what makes the mapping learnable; it is a tuning constant and must never
//     become a runtime variable.
//   PAN is bearing, and nothing else. Level carries no distance information at all -- that
//     job belongs to time here, and doubling it up on level would only make the far half of
//     every scan quiet for no added information.
//   TIMBRE is what the ground IS. It is the only channel left once the other three are spoken
//     for, which is why surface families are tone colours and never a pitch, a level or a
//     position. The families are separated on three INDEPENDENT axes rather than on five
//     shades of one -- brightness (loose < grass < paved < ice < water), roughness (the FM
//     sidebands, dirt and gravel only) and shimmer (ice's detuned octaves only) -- so a
//     listener places a surface on scales rather than memorising five sounds. Because the
//     axes are independent they are also loudness-matched: see _scan_terrain_grain.
//   RHYTHM belongs to the POI doublet alone. Nothing else in the instrument repeats, and a
//     repeat is categorical where a timbre is graded -- you never have to decide how
//     bell-like a thing sounded, only whether it happened once or twice.
//
// --- Tuning ---------------------------------------------------------------------------------
// Pythagorean rather than equal temperament. A landscape is full of repeated intervals --
// a hillside climbing at a steady grade lands on the same step again and again -- and in
// 12-TET the fifths those produce are 2 cents narrow, which at this register beats at about
// 2 Hz. Pure 3:2 and 4:3 do not beat at all, so a constant slope reads as a stable chord
// instead of a wobble. The degrees are the classic chain-of-fifths chromatic on C.
/// level ground, and the reference ping
pub const SCAN_MIDDLE_C_HZ: f64 = 261.626;
/// C C#  (limma) D Eb E   (ditone) F   (pure fourth) F# G   (pure fifth) Ab A Bb B
pub const SCAN_PYTHAGOREAN_RATIOS: [f64; 12] = [
    1.0,           // C
    256.0 / 243.0, // C#  (limma)
    9.0 / 8.0,     // D
    32.0 / 27.0,   // Eb
    81.0 / 64.0,   // E   (ditone)
    4.0 / 3.0,     // F   (pure fourth)
    729.0 / 512.0, // F#
    3.0 / 2.0,     // G   (pure fifth)
    128.0 / 81.0,  // Ab
    27.0 / 16.0,   // A
    16.0 / 9.0,    // Bb
    243.0 / 128.0, // B
];
// Fixed metres-per-octave rather than auto-scaling each scan to the range it happens to
// find. Auto-scaling would always use the full pitch range, but then a pitch would mean
// nothing on its own and flat ground would sound dramatic. Fixed, the same hill sounds the
// same every time and the listener learns absolute heights.
/// metres of elevation per octave
pub const SCAN_M_PER_OCTAVE: f64 = 20.0;
/// +/- octaves, i.e. +/-40 m, past which the pitch pins
pub const SCAN_OCTAVE_CLAMP: f64 = 2.0;
// --- Timing ---------------------------------------------------------------------------------
/// the whole cloud, always, whatever the scan reached
pub const SCAN_DURATION_S: f64 = 2.0;
/// must match SCAN_MAX_RANGE_M in terrainScanner.lua
pub const SCAN_MAX_RANGE_M: f64 = 200.0;
/// the calibration ping at middle C
pub const SCAN_REF_PING_MS: f64 = 250.0;
pub const SCAN_REF_PING_DB: f64 = -16.0;
/// cloud starts here; must exceed the ping's own length
pub const SCAN_REF_LEAD_S: f64 = 0.35;
// The grain is LONG and its decay is CONTINUOUS, and those are two different jobs. The old
// grain was a flat-topped 90 ms window: 8 ms up, 74 ms at full level, 8 ms down. Overlap was
// never what it lacked -- at 90 ms it already reached past a ring's 83 ms spacing -- what made
// six hundred grains read as six hundred events is that each one SAT at full level and then
// stopped. A cubic decay removes the plateau (at half its length the envelope is at 0.125
// where the flat top is at 1.000), and the extra length then buys the tail that laps the next
// ring. Measured at 48 kHz: energy centroid 30.1 ms against the old form's 45.0 ms, i.e. the
// time-is-distance axis gets SHARPER, not blurrier, because the energy moves toward the onset
// while only the inaudible tail extends. Still above -20 dB of its own peak at 107.2 ms.
/// one terrain/water sample
pub const SCAN_GRAIN_MS: f64 = 190.0;
/// the only hard edge left in a grain
pub const SCAN_GRAIN_ATTACK_MS: f64 = 12.0;
// (1-u)^3 reaches zero WITH ZERO DERIVATIVE at the end of the grain, so the decay is
// click-free by construction and needs no release taper. Measured last sample 0.0, final
// slope 1.3e-12. Adding a taper back would be undoing the reason this exponent was chosen.
pub const SCAN_GRAIN_DECAY_POW: f64 = 3.0;
/// the ping voices' attack; grains no longer have a release edge
pub const SCAN_GRAIN_FADE_MS: f64 = 8.0;
// The reference ping keeps a flat BODY -- it is the pitch everything else is judged against and
// a listener cannot judge an absolute pitch against a transient. What it gains is the same cubic
// curve as a RELEASE, so it decays into the cloud instead of stopping dead on an 8 ms cosine.
// Whole-envelope cubic here would put its energy centroid at 31 ms and turn a 250 ms calibration
// tone into a click, and at a fixed peak constant would quietly cost it 7.9 dB of RMS.
pub const SCAN_REF_RELEASE_MS: f64 = 90.0;
/// one vehicle
pub const SCAN_PING_MS: f64 = 55.0;
/// one prop -- see the kind tag in the O row; bigger thing rings longer
pub const SCAN_PROP_PING_MS: f64 = 22.0;
// Onset jitter, and it is not decoration. Without it every spoke of a given ring fires on
// the same sample at the same pitch, so flat ground sums perfectly coherently and comes out
// as twenty-five rhythmic pulses -- the rings themselves become audible as a beat, which is
// the exact opposite of the melded surface this is supposed to be. Jittering decorrelates
// them into a cloud. It MUST stay under half of one ring's spacing in time, or two rings can
// swap and the time-is-distance mapping stops being true: at SCAN_DURATION_S of 2.0 over the
// mod's 25 rings that spacing is 83 ms, so the usable ceiling is about 41 ms. Shortening the
// scan tightens this -- terrain_scan_sim.py scenario 11 reads the mod's ring count and
// asserts the margin rather than leaving it to be rediscovered.
pub const SCAN_TIME_JITTER_MS: f64 = 25.0;
/// seeded, so a scan of the same ground renders identically twice
pub const SCAN_RNG_SEED: i64 = 23716;
// --- Voices -----------------------------------------------------------------------------------
// Terrain is near-sine: it is the bed the whole picture sits on and is up to sixteen voices
// deep, so anything richer turns to mud. Water is the same pitch rule on a much brighter
// timbre -- the depth is carried by the LAKE BED, so "deeper is lower" falls out of the one
// elevation map with no second rule, and the water stays pitch-continuous with its shore.
/// second harmonic, relative to the fundamental
pub const SCAN_TERRAIN_H2_DB: f64 = -14.0;
pub const SCAN_TERRAIN_H3_DB: f64 = -20.0;
/// 1/k saw -- unmistakably brighter than the terrain grain
pub const SCAN_WATER_HARMONICS: i64 = 8;
/// a little movement, so water reads as a surface not a pad
pub const SCAN_WATER_VIBRATO_HZ: f64 = 5.5;
pub const SCAN_WATER_VIBRATO_CENTS: f64 = 12.0;
/// odd only: hollow and bright, cuts through the bed
/// Odd only: hollow and bright, cuts through the bed.
pub const SCAN_OBJECT_HARMONICS: [i64; 4] = [1, 3, 5, 7];
/// 1/e decays across the ping
pub const SCAN_OBJECT_DECAY: f64 = 5.0;
/// transposed up, so pings sit above the bed rather than in it
pub const SCAN_OBJECT_OCTAVE: f64 = 1.0;
// --- Surface families ------------------------------------------------------------------------
// Pitch is elevation, time is distance, pan is bearing, and level carries nothing (see above).
// All four channels are spoken for, so what the ground IS rides on TIMBRE alone -- on three
// axes that are independent, so the families are not five shades of one thing:
//   BRIGHTNESS  paved bright, grass darker, dirt/gravel darkest
//   ROUGHNESS   FM sidebands, dirt and gravel ONLY -- this is what separates them from grass
//   SHIMMER     inharmonic partials and a slow beat, ice ONLY
// Paved and water are unchanged, so nothing already learned is invalidated, and an unknown
// surface IS paved's tone -- a map with no TerrainBlock renders exactly as it did before.
// Grass is a MIDDLE tier and has to be audibly one: at the first values tried it landed on a
// spectral centroid of 1.11 f0 against gravel's 1.10, i.e. indistinguishable from the loose
// family on brightness, with only the FM left to separate them. Measured now -- loose 1.04
// (dirt) and 1.10 (gravel), grass 1.21, paved 1.31 -- so the three tiers are ordered and
// roughly evenly spaced, and grass is still the only one of the three carrying no FM at all.
pub const SCAN_GRASS_H2_DB: f64 = -16.0;
pub const SCAN_GRASS_H3_DB: f64 = -26.0;
// Kept at -24 rather than pushed darker: h2 is one of the two partials whose beating IS the
// roughness (see SCAN_LOOSE_FM_RATIO), so darkening the loose family by dropping it would
// quietly cost it the cue that distinguishes it from grass.
pub const SCAN_LOOSE_H2_DB: f64 = -24.0;
// The modulator sits BELOW the carrier, so the sidebands land subharmonically and the grain
// darkens as it roughens. Phase modulation with the modulator locked to the carrier, the same
// technique _scan_water_grain uses for its vibrato, so the grain keeps ONE pitch -- the
// elevation channel must not be smeared by the surface channel.
// 5/6 rather than the obvious 1/2, and the choice is load-bearing. Sidebands land at
// fc*(1 +/- r): at r = 1/2 the lower one is fc/2, an octave BELOW the fundamental, which a
// listener reads as a lower note -- i.e. the surface channel leaking into the elevation
// channel, the one thing that must not happen. At 5/6 it is fc/6, two and a half octaves down
// and well under the fundamental in level, so it is an under-buzz rather than a pitch. The
// roughness itself comes from the upper sideband at 11fc/6 beating against the h2 at 2fc, at
// fc/6 -- 43.6 Hz at middle C, squarely in the psychoacoustic roughness band, which is also
// why h2 is kept here at all.
pub const SCAN_LOOSE_FM_RATIO: f64 = 0.8333333333333334;
// Dirt and gravel share the ratio and differ in INDEX, because "gravel is rougher than dirt"
// is a scale a listener can place a surface on, where two unrelated ratios are two things to
// memorise. Both are well under 1: J1(I) < J0(I) always, so the sidebands can never overtake
// the fundamental however the index is nudged.
pub const SCAN_DIRT_FM_INDEX: f64 = 0.35;
pub const SCAN_GRAVEL_FM_INDEX: f64 = 0.7;
// Octaves ONLY -- no odd partials at all. That is what separates ice from WATER, which carries
// every integer harmonic including a strong 3f; brightness alone does not separate them well
// enough to rely on. The upper pairs are detuned against each other so they beat: shimmer as a
// consequence of the spectrum rather than as an amplitude envelope bolted on top.
/// (multiple of f0, level dB)
pub const SCAN_ICE_PARTIALS: [(f64, f64); 4] =
    [(1.0, 0.0), (2.0, -10.0), (4.0, -16.0), (8.0, -24.0)];
/// 4.8 Hz beat at middle C, 9.7 Hz on the 8f pair: under 20 Hz,
pub const SCAN_ICE_DETUNE_CENTS: f64 = 8.0;
// i.e. shimmer, at every pitch inside the +/-2 octave clamp
/// ice blooms where the others strike
pub const SCAN_ICE_ATTACK_MULT: f64 = 2.5;
// The wire codes terrainScanner.lua sends. Omission means paved, which is also what an
// unresolved material means -- see the parse in beamtel.py's terrain_scan_listener.
// "r" is the nav-graph road overlay and renders IDENTICALLY to paved. It is a separate code
// only so terrainScanner.lua's diag can answer "how many cells did the overlay override" --
// a radius-against-the-road-graph rule is a heuristic, and a heuristic whose effect cannot be
// counted is one nobody can debug.
/// Python's dict, in its insertion order: (wire code, family).
pub const SCAN_FAMILY_CODES: [(&str, &str); 5] = [
    ("d", "dirt"),
    ("v", "gravel"),
    ("g", "grass"),
    ("i", "ice"),
    ("r", "paved"),
];
pub const SCAN_PAVED: &str = "paved";
// The set _scan_terrain_grain will actually branch on. Anything else on the wire falls back
// to paved rather than raising, so a newer mod half degrades instead of breaking the scan.
/// A frozenset in Python; membership is all that is observable.
pub const SCAN_FAMILY_VOICES: [&str; 5] = [SCAN_PAVED, "dirt", "gravel", "grass", "ice"];
// --- POI doublets ------------------------------------------------------------------------------
// Two pings rather than one, because RHYTHM is categorical where timbre is graded: a listener
// never has to decide how bell-like a thing was, only whether it happened once or twice. Same
// argument the slam gate's rising earcon makes against the docking lock's falling one.
// Onset-to-onset is PING + GAP and must stay under one ring's time spacing
// (SCAN_DURATION_S / (rings - 1) = 83.3 ms today), or the two halves can be heard as two
// cells one ring apart rather than as one doubled event -- which is the whole cue. 30 + 40
// is 70 ms, a fast double-tick with 13 ms of margin.
pub const SCAN_POI_PING_MS: f64 = 30.0;
/// silence between the two halves
pub const SCAN_POI_GAP_MS: f64 = 40.0;
// Ideal-bar modes, i.e. a bell. Inharmonic, so it cannot be mistaken for the object ping's odd
// harmonics, the water grain's saw, or anything else in the mod.
pub const SCAN_POI_PARTIALS: [f64; 4] = [1.0, 2.76, 5.40, 8.93];
pub const SCAN_POI_DECAY: f64 = 6.0;
/// same register as the object ping: only timbre and rhythm differ
pub const SCAN_POI_OCTAVE: f64 = 1.0;
// BOTH halves are at the SAME pitch. Making the second a fifth above is the obvious "nice"
// design and it is a violation: pitch is the elevation channel and nothing else may use it.
// The rhythm is the signature -- nothing else in the instrument repeats.

// The configured level sets the whole cloud; the other two voices are fixed OFFSETS from it
// rather than levels of their own, so no mix exists in which the terrain bed is inaudible
// under the pings or the pings vanish into the bed. Same argument the docking pair makes.
/// cloud peak; scan_tone_dbfs overrides this
pub const SCAN_TERRAIN_DB: f64 = -20.0;
/// water is already brighter, so it needs less level to read
pub const SCAN_WATER_DB_OFFSET: f64 = -3.0;
/// pings are the sparse layer and have to stand out
pub const SCAN_OBJECT_DB_OFFSET: f64 = 3.0;
/// POIs are the sparse layer too
pub const SCAN_POI_DB_OFFSET: f64 = 3.0;
/// replacing an in-flight scan, so a double tap does not click
pub const SCAN_CROSSFADE_MS: f64 = 15.0;

// ---------------------------------------------------------------------------------------
// Module-level pure functions
// ---------------------------------------------------------------------------------------

/// Python `max(a, b)`: `a` unless `b > a` (so a NaN `b` loses, a NaN `a` wins).
#[inline]
pub fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// Python `min(a, b)`: `a` unless `b < a`.
#[inline]
pub fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// Map one selected hazard to its state-bounded pulse cadence.
///
/// `state` is Python's `int(state or 0)` and `urgency` its `float(urgency or 0.0)`; the
/// caller does those coercions.
pub fn obstacle_cue_rate_hz(state: i64, urgency: f64) -> f64 {
    let urgency = py_min(255.0, py_max(0.0, urgency));
    let (amount, (lo, hi)) = if state <= 1 {
        (py_min(1.0, urgency / 169.0), OBSTACLE_ADVISORY_RATE)
    } else {
        (
            py_min(1.0, py_max(0.0, (urgency - 170.0) / 85.0)),
            OBSTACLE_URGENT_RATE,
        )
    };
    lo + (hi - lo) * amount
}

/// Gain applied only to non-safety continuous guidance voices.
pub fn obstacle_duck_gain(state: i64) -> f64 {
    if state >= 3 {
        return 10.0f64.powf(OBSTACLE_EMERGENCY_DUCK_DB / 20.0);
    }
    if state >= 2 {
        return 10.0f64.powf(OBSTACLE_URGENT_DUCK_DB / 20.0);
    }
    1.0
}

/// `stop_bar_state`'s result: Python's `("pulse", period_s)`, `("waiting", period_s)`,
/// `("solid", None)` or `("silent", None)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StopBarState {
    Pulse(f64),
    Waiting(f64),
    Solid,
    Silent,
}

impl StopBarState {
    /// The Python tag string.
    pub fn name(&self) -> &'static str {
        match self {
            StopBarState::Pulse(_) => "pulse",
            StopBarState::Waiting(_) => "waiting",
            StopBarState::Solid => "solid",
            StopBarState::Silent => "silent",
        }
    }

    /// The period, `None` where Python returns `None`.
    pub fn period(&self) -> Option<f64> {
        match self {
            StopBarState::Pulse(p) | StopBarState::Waiting(p) => Some(*p),
            _ => None,
        }
    }
}

/// `on_bar` says the nose was already on this bar, which widens it behind the line. Past
/// the bar, or past the line while moving, the cue has done its job. `stopped` `None`
/// means "derive it from speed", as Python's default does.
pub fn stop_bar_state(
    signed_distance: f64,
    speed: f64,
    stopped: Option<bool>,
    on_bar: bool,
) -> StopBarState {
    let stopped = stopped.unwrap_or(speed < STOP_BAR_STOPPED_MPS);
    if signed_distance < 0.0 && !stopped {
        return StopBarState::Silent;
    }
    let reach = STOP_BAR_REACH_M + if on_bar { STOP_BAR_HOLD_M } else { 0.0 };
    if -STOP_BAR_CLEAR_M <= signed_distance && signed_distance <= reach {
        return StopBarState::Solid;
    }
    if signed_distance < -STOP_BAR_CLEAR_M {
        return StopBarState::Silent;
    }
    if stopped {
        return StopBarState::Waiting(STOP_BAR_WAIT_PERIOD_S);
    }
    let to_bar = signed_distance - STOP_BAR_REACH_M;
    // Distance keeps it useful while creeping; time to the bar adds urgency at speed.
    let inner = py_min(
        py_min(STOP_BAR_MAX_PERIOD_S, to_bar / 30.0),
        to_bar / py_max(1.0, speed) / 4.0,
    );
    StopBarState::Pulse(py_max(STOP_BAR_MIN_PERIOD_S, inner))
}

/// Pure off-road pulse-rate map shared by callback and diagnostics.
pub fn road_beacon_rate_hz(distance: f64) -> f64 {
    let rate_norm = (-py_max(0.0, distance) / ROAD_BEEP_HALF_DIST_M).exp();
    ROAD_BEEP_MIN_RATE_HZ + (ROAD_BEEP_MAX_RATE_HZ - ROAD_BEEP_MIN_RATE_HZ) * rate_norm
}

/// Same shape as [`road_beacon_rate_hz`], deliberately (one learned relationship between
/// rate and closeness), but on the route's own distance scale.
pub fn route_beacon_rate_hz(distance: f64) -> f64 {
    let rate_norm = (-py_max(0.0, distance) / ROUTE_BEACON_HALF_DIST_M).exp();
    ROUTE_BEACON_MIN_RATE_HZ + (ROUTE_BEACON_MAX_RATE_HZ - ROUTE_BEACON_MIN_RATE_HZ) * rate_norm
}

/// The directional correction-pip rate for a normalized demand.
pub fn road_correction_rate_hz(severity: f64) -> f64 {
    let amount = py_min(1.0, py_max(0.0, severity));
    ROAD_CORRECTION_RATE_MIN_HZ
        + (ROAD_CORRECTION_RATE_MAX_HZ - ROAD_CORRECTION_RATE_MIN_HZ) * amount
}
