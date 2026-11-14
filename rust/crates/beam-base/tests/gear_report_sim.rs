//! diagnostic/gear_report_sim.py: the spoken phrase, the sport collapse, and the tone.
//! Same scenarios, same order, same names.

use beam_base::gear_report as gr;
use beam_base::pyval::{PyDict, PyValue};

const SR: u32 = 48000;

struct Sim {
    failures: Vec<String>,
}

impl Sim {
    fn check(&mut self, name: &str, ok: bool, detail: impl AsRef<str>) {
        let detail = detail.as_ref();
        println!(
            "{} {name}{}",
            if ok { "OK  " } else { "FAIL" },
            if detail.is_empty() {
                String::new()
            } else {
                format!(" -- {detail}")
            }
        );
        if !ok {
            self.failures.push(name.to_string());
        }
    }
}

/// Speak a sequence of extended gear strings as the telemetry loop would.
fn replay(sequence: &[&str], collapse_sport: bool, number_style: &str) -> Vec<String> {
    let mut spoken = Vec::new();
    let mut prev: Option<&str> = None;
    for &g in sequence {
        if Some(g) != prev
            && let Some(phrase) = gr::extended_change_phrase(g, prev, collapse_sport, number_style)
        {
            spoken.push(phrase);
        }
        prev = Some(g);
    }
    spoken
}

fn r(seq: &[&str]) -> Vec<String> {
    replay(seq, false, "ordinal")
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn tone_steps<'a>(gears: &[&'a str], collapse_sport: bool) -> Vec<&'a str> {
    let mut prev = "";
    let mut out = Vec::new();
    for &g in gears {
        if !gr::sport_step_silenced(g, prev, collapse_sport) {
            out.push(g);
        }
        prev = g;
    }
    out
}

/// `np.fft.rfft` magnitude of the buffer zero-padded to SR samples, and the bin
/// frequencies. A direct DFT with a rotating phasor; the buffer is short.
fn spectrum(buf: &[f32]) -> (Vec<f64>, Vec<f64>) {
    let n = SR as usize;
    let bins = n / 2 + 1;
    let mut freqs = Vec::with_capacity(bins);
    let mut mags = Vec::with_capacity(bins);
    for k in 0..bins {
        let w = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
        let (step_im, step_re) = w.sin_cos();
        let (mut pr, mut pi) = (1.0f64, 0.0f64);
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for &x in buf {
            re += x as f64 * pr;
            im += x as f64 * pi;
            let nr = pr * step_re - pi * step_im;
            pi = pr * step_im + pi * step_re;
            pr = nr;
        }
        freqs.push(k as f64 * SR as f64 / n as f64);
        mags.push((re * re + im * im).sqrt());
    }
    (freqs, mags)
}

fn peak_db(buf: &[f32]) -> f64 {
    let peak = buf.iter().map(|x| x.abs()).fold(0.0f32, f32::max) as f64;
    20.0 * peak.log10()
}

fn argmax(v: &[f64]) -> usize {
    let mut best = 0;
    for (i, x) in v.iter().enumerate() {
        if *x > v[best] {
            best = i;
        }
    }
    best
}

fn dict(pairs: &[(&str, PyValue)]) -> PyDict {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn gear_report_sim() {
    let mut sim = Sim {
        failures: Vec::new(),
    };

    // 1. The default style is byte-identical to the phrases spoken before this module.
    let seq = [
        "P", "R", "N", "1", "2", "3", "N", "D", "S3", "M5", "-1", "0", "80%",
    ];
    sim.check(
        "default speech unchanged",
        r(&seq)
            == strs(&[
                "park", "reverse", "neutral", "first", "second", "third", "neutral", "drive",
                "sport 3", "manual 5", "reverse",
            ]),
        format!("{:?}", r(&seq)),
    );

    // 2. Numbers style changes forward numeric gears only.
    sim.check(
        "numbers style",
        replay(&["N", "1", "2", "S3", "R"], false, "number")
            == strs(&["neutral", "1", "2", "sport 3", "reverse"]),
        "",
    );
    sim.check(
        "number style past the word table",
        gr::extended_gear_to_phrase("14", "number") == "14",
        "",
    );
    sim.check(
        "ordinal past the word table",
        gr::extended_gear_to_phrase("14", gr::DEFAULT_NUMBER_STYLE) == "14th",
        "",
    );

    // 3. Sport collapse: one "sport" on entry, silence while the CVT steps, and leaving
    //    sport is still spoken.
    let cvt = ["D", "S1", "S2", "S3", "S2", "S1", "D", "S1", "N"];
    sim.check(
        "sport said once per entry",
        replay(&cvt, true, "ordinal") == strs(&["drive", "sport", "drive", "sport", "neutral"]),
        format!("{:?}", replay(&cvt, true, "ordinal")),
    );
    sim.check(
        "collapse off still names each step",
        r(&cvt)
            == strs(&[
                "drive", "sport 1", "sport 2", "sport 3", "sport 2", "sport 1", "drive",
                "sport 1", "neutral",
            ]),
        "",
    );
    sim.check(
        "collapse leaves manual alone",
        replay(&["D", "M1", "M2"], true, "ordinal") == strs(&["drive", "manual 1", "manual 2"]),
        "",
    );

    // 3b. The same collapse silences the tone: only the steps inside sport are dropped,
    //     so audio plays entry and exit exactly where speech speaks them.
    sim.check(
        "tone collapse plays sport once per entry",
        tone_steps(&cvt, true) == ["D", "S1", "D", "S1", "N"],
        format!("{:?}", tone_steps(&cvt, true)),
    );
    sim.check(
        "tone collapse off plays every step",
        tone_steps(&cvt, false) == cvt,
        "",
    );
    sim.check(
        "tone collapse leaves manual alone",
        tone_steps(&["D", "M1", "M2"], true) == ["D", "M1", "M2"],
        "",
    );

    // 4. OutGauge keeps its own encoding.
    let og: Vec<Option<String>> = [0, 1, 2, 3]
        .iter()
        .map(|&b| gr::outgauge_change_phrase(b, "ordinal"))
        .collect();
    sim.check(
        "outgauge",
        og == ["reverse", "neutral", "first", "second"]
            .iter()
            .map(|s| Some(s.to_string()))
            .collect::<Vec<_>>(),
        "",
    );
    sim.check(
        "outgauge numbers",
        gr::outgauge_change_phrase(4, "number").as_deref() == Some("3"),
        "",
    );

    // 5. Degrees follow the lever: P < R < N < 1 < 2 ..., and both protocols agree.
    let degrees: Vec<Option<i64>> = ["P", "R", "N", "1", "2", "3"]
        .iter()
        .map(|g| gr::extended_gear_degree(g))
        .collect();
    let mut sorted = degrees.clone();
    sorted.sort();
    let mut uniq = degrees.clone();
    uniq.dedup();
    sim.check(
        "lever order",
        degrees == sorted && uniq.len() == 6 && degrees.iter().all(Option::is_some),
        format!("{degrees:?}"),
    );
    sim.check(
        "protocols agree",
        [0, 1, 2, 3]
            .iter()
            .map(|&b| gr::outgauge_gear_degree(b))
            .collect::<Vec<_>>()
            == ["R", "N", "1", "2"]
                .iter()
                .map(|g| gr::extended_gear_degree(g))
                .collect::<Vec<_>>(),
        "",
    );
    sim.check(
        "sport and manual share the number's degree",
        gr::extended_gear_degree("S3") == gr::extended_gear_degree("M3")
            && gr::extended_gear_degree("M3") == gr::extended_gear_degree("3"),
        "",
    );
    sim.check(
        "cannon strength has no degree",
        gr::extended_gear_degree("80%").is_none(),
        "",
    );
    // A manual box sits in "0" for as long as a shift button is held, so it must be as
    // silent in audio as it is in speech; an automatic's "N" keeps its tone.
    sim.check(
        "manual neutral has no tone",
        gr::extended_gear_degree("0").is_none(),
        "",
    );
    sim.check(
        "manual neutral is not spoken",
        r(&["2", "0", "3"]) == strs(&["second", "third"]),
        "",
    );
    sim.check(
        "automatic neutral keeps its tone",
        gr::extended_gear_degree("N") == Some(0),
        "",
    );

    // 6. Pitch: A4 is 440, neutral is the root, and every scale ascends with the gears.
    sim.check(
        "A4 = 440",
        (gr::root_frequency("A", 4) - 440.0).abs() < 1e-9,
        "",
    );
    sim.check(
        "C4 = middle C",
        (gr::root_frequency("C", 4) - 261.6256).abs() < 1e-3,
        "",
    );
    sim.check(
        "flat spelling",
        gr::note_semitone("Db") == gr::note_semitone("C#") && gr::note_semitone("C#") == Some(1),
        "",
    );
    for sc in gr::SCALES {
        let hz: Vec<f64> = (-2..19)
            .map(|d| gr::degree_frequency(d, "C", 3, sc.key))
            .collect();
        sim.check(
            &format!("ascending: {}", sc.label),
            hz.windows(2).all(|w| w[1] > w[0]),
            "",
        );
    }
    let qt: Vec<f64> = (0..4)
        .map(|d| gr::degree_semitones(d, "quarter_tone"))
        .collect();
    sim.check(
        "quarter-tone steps are 50 cents",
        qt == [0.0, 0.5, 1.0, 1.5],
        format!("{qt:?}"),
    );
    sim.check(
        "pentatonic wraps to the octave",
        gr::degree_semitones(5, "major_pentatonic") == 12.0,
        "",
    );
    sim.check(
        "negative degrees go below the root",
        gr::degree_semitones(-1, "major") == -1.0,
        "",
    );

    // 7. The tone: exact peak level, short, a gentle onset, and a cubic tail ending at zero.
    for db in [-3.0, -20.0] {
        let tone = gr::render_tone(440.0, db, SR, false);
        let peak = peak_db(&tone);
        sim.check(
            &format!("peak at {db:?} dBFS"),
            (peak - db).abs() < 0.01,
            format!("{peak:.3}"),
        );
    }
    let tone = gr::render_tone(440.0, -3.0, SR, false);
    let dur_ms = 1000.0 * tone.len() as f64 / SR as f64;
    let want_ms = gr::TONE_ATTACK_MS + gr::TONE_HOLD_MS + gr::TONE_DECAY_MS;
    sim.check(
        "duration matches the envelope",
        (dur_ms - want_ms).abs() < 0.1,
        format!("{dur_ms:.1} ms"),
    );
    let env = gr::tone_envelope(SR);
    sim.check("onset starts silent", env[0] == 0.0, "");
    let onset = &env[..(SR as f64 * 0.005) as usize];
    sim.check(
        "onset jump below a click",
        onset
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f64::max)
            < 0.02,
        "",
    );
    let decay = (SR as f64 * gr::TONE_DECAY_MS / 1000.0) as usize;
    let tail = &env[env.len() - decay..];
    sim.check(
        "tail is cubic",
        (tail[decay / 2] - 0.125).abs() < 0.01,
        format!("{:.3}", tail[decay / 2]),
    );
    sim.check(
        "tail ends at silence",
        (*tone.last().unwrap()).abs() < 1e-4,
        "",
    );
    sim.check(
        "high root drops aliasing partials",
        gr::render_tone(8000.0, -3.0, SR, false)
            .iter()
            .all(|x| x.is_finite()),
        "",
    );

    // 7b. Sport is the same pitch and peak as the plain tone, only brighter. Drive and
    //     sport 1 share a degree, so timbre is the only thing that can mark that shift.
    sim.check(
        "drive and sport 1 share a degree",
        gr::extended_gear_degree("D") == gr::extended_gear_degree("S1"),
        "",
    );
    let plain = gr::render_tone(330.0, -3.0, SR, false);
    let sport = gr::render_tone(330.0, -3.0, SR, true);
    let (f, mp) = spectrum(&plain);
    let (_, ms) = spectrum(&sport);
    let centroid = |m: &[f64]| {
        f.iter().zip(m).map(|(a, b)| a * b).sum::<f64>() / m.iter().sum::<f64>()
    };
    let c_plain = centroid(&mp);
    let c_sport = centroid(&ms);
    sim.check(
        "sport is brighter",
        c_sport > c_plain * 1.25,
        format!("centroid {c_plain:.0} -> {c_sport:.0} Hz"),
    );
    sim.check("sport is only a bit brighter", c_sport < c_plain * 2.0, "");
    sim.check(
        "sport keeps the pitch",
        (f[argmax(&ms)] - 330.0).abs() < 2.0 && (f[argmax(&mp)] - 330.0).abs() < 2.0,
        "",
    );
    sim.check(
        "sport keeps the peak",
        (peak_db(&sport) + 3.0).abs() < 0.01,
        "",
    );
    sim.check(
        "sport tone only for sport gears",
        ["D", "S", "S1", "S12", "M1", "3", "SPORT"]
            .iter()
            .map(|g| gr::is_sport(g))
            .collect::<Vec<_>>()
            == [false, true, true, true, false, false, false],
        "",
    );

    // 8. Config: the old boolean migrates, and garbage falls back to defaults.
    let mut cfg = dict(&[("announce_gear", PyValue::Bool(false))]);
    let changed = gr::migrate_config(&mut cfg);
    sim.check(
        "migrate off",
        changed && cfg == dict(&[("gear_report_mode", PyValue::from("off"))]),
        "",
    );
    let mut cfg = dict(&[("announce_gear", PyValue::Bool(true))]);
    gr::migrate_config(&mut cfg);
    sim.check(
        "migrate on",
        cfg == dict(&[("gear_report_mode", PyValue::from("speech"))]),
        "",
    );
    let mut cfg = dict(&[
        ("announce_gear", PyValue::Bool(false)),
        ("gear_report_mode", PyValue::from("audio")),
    ]);
    gr::migrate_config(&mut cfg);
    sim.check(
        "migrate keeps an explicit mode",
        cfg == dict(&[("gear_report_mode", PyValue::from("audio"))]),
        "",
    );
    let s = gr::settings_from_config(&dict(&[
        ("gear_report_mode", PyValue::from("loud")),
        ("gear_tone_root", PyValue::from("H")),
        ("gear_tone_octave", PyValue::from(11i64)),
        ("gear_tone_scale", PyValue::from("x")),
        ("gear_tone_level_dbfs", PyValue::from(9i64)),
    ]))
    .unwrap();
    sim.check(
        "garbage falls back",
        s.mode == "speech"
            && s.root == "C"
            && s.octave == 4
            && s.scale == "major"
            && s.level_db == 0.0,
        format!("{s:?}"),
    );
    sim.check(
        "default level -3 dBFS",
        gr::settings_from_config(&PyDict::new()).unwrap().level_db == -3.0,
        "",
    );

    assert!(sim.failures.is_empty(), "{} FAILED: {:?}", sim.failures.len(), sim.failures);
}
