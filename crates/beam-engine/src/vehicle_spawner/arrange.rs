//! Arrangement presets (G): the type/variant/spacing screen, the spacing odometer, and the
//! two actions (spawn the queue arranged, or teleport the active vehicles into place).

use beam_base::pyval::{PyDict, PyValue};

use super::{Screen, Vs, pyjson, vehicles_word};

/// `(type_key, display_label, [(variant_key, variant_label), ...])`.
/// `(type_key, display_label, variants)`.
pub type ArrangeType = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

pub const ARRANGE_TYPES: [ArrangeType; 5] = [
    (
        "line",
        "Line",
        &[
            ("start", "Anchor at front"),
            ("end", "Anchor at back"),
            ("middle", "Anchor in middle"),
        ],
    ),
    (
        "side_by_side",
        "Side by side",
        &[
            ("left", "Anchor at left"),
            ("right", "Anchor at right"),
            ("middle", "Anchor in middle"),
        ],
    ),
    (
        "two_columns",
        "Two columns",
        &[
            ("front", "Anchor at front"),
            ("middle", "Anchor in middle"),
            ("back", "Anchor at back"),
        ],
    ),
    (
        "three_columns",
        "Three columns",
        &[
            ("front", "Anchor at front"),
            ("middle", "Anchor in middle"),
            ("back", "Anchor at back"),
        ],
    ),
    ("boxed_in", "Boxed in", &[("middle", "Anchor in center")]),
];

/// `_DigitField`: a five-digit odometer for distances in feet (0..99999). Left/Right move
/// the cursor (wrapping), Up/Down cycle the digit under it (wrapping). Position 0 is the
/// most significant digit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigitField {
    pub cursor: usize,
    pub digits: [u8; 5],
}

impl DigitField {
    pub const NDIGITS: usize = 5;

    pub fn new(value: i64) -> DigitField {
        let mut f = DigitField {
            cursor: 0,
            digits: [0; 5],
        };
        f.set_value(value);
        f
    }

    pub fn set_value(&mut self, value: i64) {
        let value = value.clamp(0, 99_999);
        let s = format!("{value:05}");
        for (i, c) in s.bytes().enumerate() {
            self.digits[i] = c - b'0';
        }
    }

    pub fn value(&self) -> i64 {
        self.digits.iter().fold(0, |n, d| n * 10 + i64::from(*d))
    }

    pub fn move_left(&mut self) {
        self.cursor = (self.cursor + Self::NDIGITS - 1) % Self::NDIGITS;
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1) % Self::NDIGITS;
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = Self::NDIGITS - 1;
    }

    pub fn inc(&mut self) {
        self.digits[self.cursor] = (self.digits[self.cursor] + 1) % 10;
    }

    pub fn dec(&mut self) {
        self.digits[self.cursor] = (self.digits[self.cursor] + 9) % 10;
    }

    pub fn current_digit(&self) -> u8 {
        self.digits[self.cursor]
    }

    /// Leading zeros dropped, so it reads "150 feet", not "zero zero one five zero".
    pub fn spoken_value(&self) -> String {
        self.value().to_string()
    }
}

/// `_compute_arrangement_offsets(arr_type, variant, n_others, spacing_m)`: (fwd_m,
/// right_m) for each of the others relative to the anchor. (The Lua computes the real
/// positions; nothing in the Python calls this.)
pub fn compute_arrangement_offsets(
    arr_type: &str,
    variant: &str,
    n_others: i64,
    spacing_m: f64,
) -> Vec<(f64, f64)> {
    let s = spacing_m;
    let n = n_others;
    let mut offsets: Vec<(f64, f64)> = Vec::new();
    let full = |o: &Vec<(f64, f64)>| (o.len() as i64) >= n;

    match arr_type {
        "line" => {
            if variant == "start" {
                for k in 1..=n {
                    offsets.push((-(k as f64) * s, 0.0));
                }
            } else if variant == "end" {
                for k in 1..=n {
                    offsets.push((k as f64 * s, 0.0));
                }
            } else {
                let n_front = n.div_euclid(2);
                let n_back = n - n_front;
                for k in 1..=n_front {
                    offsets.push((k as f64 * s, 0.0));
                }
                for k in 1..=n_back {
                    offsets.push((-(k as f64) * s, 0.0));
                }
            }
        }
        "side_by_side" => {
            if variant == "left" {
                for k in 1..=n {
                    offsets.push((0.0, k as f64 * s));
                }
            } else if variant == "right" {
                for k in 1..=n {
                    offsets.push((0.0, -(k as f64) * s));
                }
            } else {
                let n_right = n.div_euclid(2);
                let n_left = n - n_right;
                for k in 1..=n_right {
                    offsets.push((0.0, k as f64 * s));
                }
                for k in 1..=n_left {
                    offsets.push((0.0, -(k as f64) * s));
                }
            }
        }
        "two_columns" | "three_columns" => {
            let cols: i64 = if arr_type == "two_columns" { 2 } else { 3 };
            let n_total = n + 1;
            // math.ceil(n_total / cols)
            let n_rows = (n_total as f64 / cols as f64).ceil() as i64;
            let anchor_row = match variant {
                "front" => 0,
                "back" => n_rows - 1,
                _ => n_rows.div_euclid(2),
            };
            let anchor_col = if cols == 2 { 0 } else { 1 };
            'rows: for row in 0..n_rows {
                for col in 0..cols {
                    if row == anchor_row && col == anchor_col {
                        continue;
                    }
                    let fwd_m = (anchor_row - row) as f64 * s;
                    let right_m = if cols == 2 {
                        (col * 2 - 1) as f64 * (s / 2.0)
                    } else {
                        (col - 1) as f64 * s
                    };
                    offsets.push((fwd_m, right_m));
                    if full(&offsets) {
                        break;
                    }
                }
                if full(&offsets) {
                    break 'rows;
                }
            }
        }
        "boxed_in" => {
            let mut ring = 1i64;
            while (offsets.len() as i64) < n {
                let r = ring as f64 * s;
                let ring_slots = [
                    (r, 0.0),
                    (-r, 0.0),
                    (0.0, -r),
                    (0.0, r),
                    (r, r),
                    (r, -r),
                    (-r, r),
                    (-r, -r),
                ];
                for sl in ring_slots {
                    offsets.push(sl);
                    if full(&offsets) {
                        break;
                    }
                }
                ring += 1;
                if ring > 20 {
                    break;
                }
            }
        }
        _ => {}
    }
    // offsets[:N]
    let keep = if n < 0 {
        (offsets.len() as i64 + n).max(0) as usize
    } else {
        (n as usize).min(offsets.len())
    };
    offsets.truncate(keep);
    offsets
}

impl Vs {
    fn arrange_variant(&self) -> (&'static str, &'static str) {
        let variants = ARRANGE_TYPES[self.arrange_type_idx].2;
        variants[self.arrange_variant_idx.min(variants.len() - 1)]
    }

    /// `_arrange_validate()`: (queue_ok, active_ok, status_msg).
    pub(crate) fn arrange_validate(&self) -> (bool, bool, String) {
        let type_key = ARRANGE_TYPES[self.arrange_type_idx].0;
        let n_queue = self.to_spawn.len();
        let n_active = self.active_vehicles.len();
        let boxed = type_key == "boxed_in";
        let min_total = if boxed { 5 } else { 2 };
        let queue_ok = n_queue >= min_total;
        let active_ok = if boxed {
            n_active >= min_total && self.player_veh_id.is_some()
        } else {
            n_active >= min_total
        };
        let mut parts = vec![format!("Queue: {n_queue}, Active: {n_active}.")];
        if boxed {
            if n_queue < min_total || n_active < min_total {
                parts.push(format!(
                    "Boxed in needs at least {min_total} vehicles (1 anchor + 4 others)."
                ));
            }
            if n_active >= min_total && self.player_veh_id.is_none() {
                parts.push("Boxed in requires a current vehicle for active mode.".to_string());
            }
        } else if n_queue < 2 && n_active < 2 {
            parts.push("Need at least 2 vehicles.".to_string());
        }
        (queue_ok, active_ok, parts.join(" "))
    }

    /// `_speak_arrange_item(idx)`.
    pub(crate) fn speak_arrange_item(&mut self, idx: usize) {
        let type_label = ARRANGE_TYPES[self.arrange_type_idx].1;
        let variant_label = self.arrange_variant().1;
        let spacing_ft = self.spacing_field.value();
        let (queue_ok, active_ok, status) = self.arrange_validate();
        const ARRANGE_TOTAL: usize = 5;
        let content = match idx {
            0 => format!("Arrangement type: {type_label}"),
            1 => format!("Variant: {variant_label}"),
            2 => format!("Spacing: {spacing_ft} feet, press enter to edit"),
            3 => {
                let mut c = "Apply to spawn queue".to_string();
                if !queue_ok {
                    c.push_str(", unavailable");
                }
                c
            }
            _ => {
                let mut c = "Arrange active vehicles".to_string();
                if !active_ok {
                    c.push_str(", unavailable");
                }
                c
            }
        };
        if idx == 3 || idx == 4 {
            self.speak_position(idx, ARRANGE_TOTAL, &format!("{content}. {status}"));
        } else {
            self.speak_position(idx, ARRANGE_TOTAL, &content);
        }
    }

    /// `_on_g(event)`.
    pub(crate) fn on_g(&mut self) {
        if !matches!(self.screen, Screen::Main | Screen::ToSpawn | Screen::Manage) {
            return;
        }
        self.arrange_return_screen = self.screen;
        self.idx_arrange = 0;
        self.send("REQUEST_ACTIVE_VEHICLES");
        self.send("REQUEST_PLAYER_VEH_ID");
        self.enter_screen(
            Screen::Arrange,
            true,
            Some("Vehicle arrangement".to_string()),
        );
    }

    fn arrangement(&self, anchor: PyValue) -> PyDict {
        let type_key = ARRANGE_TYPES[self.arrange_type_idx].0;
        let spacing_m = self.spacing_field.value() as f64 * 0.3048;
        let mut a = PyDict::new();
        a.insert("type".into(), PyValue::from(type_key));
        a.insert("variant".into(), PyValue::from(self.arrange_variant().0));
        a.insert("spacingM".into(), PyValue::from(spacing_m));
        a.insert("anchorVehId".into(), anchor);
        a
    }

    /// `_do_arrange_queue()`.
    pub(crate) fn do_arrange_queue(&mut self) {
        let items = self.to_spawn.clone();
        let (queue_ok, _, status) = self.arrange_validate();
        if !queue_ok {
            self.say(format!("Cannot apply: {status}"));
            return;
        }
        let type_key = ARRANGE_TYPES[self.arrange_type_idx].0;
        let ordered: Vec<_> = items
            .iter()
            .filter(|it| it.replace_veh_id.is_some())
            .chain(items.iter().filter(|it| it.replace_veh_id.is_none()))
            .collect();
        let payload_items: Vec<PyValue> = ordered
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let mut e = PyDict::new();
                e.insert("queueIdx".into(), PyValue::from(i as i64));
                e.insert("model".into(), it.model.clone());
                e.insert("config".into(), it.config.clone());
                e.insert(
                    "replaceVehId".into(),
                    it.replace_veh_id.map_or(PyValue::None, PyValue::from),
                );
                PyValue::Dict(e)
            })
            .collect();
        let n = payload_items.len();
        let mut body = PyDict::new();
        body.insert("items".into(), PyValue::List(payload_items));
        body.insert(
            "arrangement".into(),
            PyValue::Dict(self.arrangement(PyValue::None)),
        );
        self.say(format!(
            "Spawning {n} {} in {} arrangement, please wait.",
            vehicles_word(n),
            type_key.replace('_', " ")
        ));
        self.send(format!("SPAWN:{}", pyjson::dumps(&PyValue::Dict(body))));
        self.to_spawn.clear();
        self.idx_to_spawn = 0;
        self.close_modal(true);
    }

    /// `_do_arrange_active()`.
    pub(crate) fn do_arrange_active(&mut self) {
        let avs = self.active_vehicles.clone();
        let (_, active_ok, status) = self.arrange_validate();
        if !active_ok {
            self.say(format!("Cannot arrange: {status}"));
            return;
        }
        let type_key = ARRANGE_TYPES[self.arrange_type_idx].0;
        let veh_ids: Vec<PyValue> = avs.iter().map(|(v, _)| PyValue::from(*v)).collect();
        let anchor = self
            .player_veh_id
            .clone()
            .map_or(PyValue::None, PyValue::Int);
        let n = veh_ids.len();
        let mut body = PyDict::new();
        body.insert(
            "arrangement".into(),
            PyValue::Dict(self.arrangement(anchor)),
        );
        body.insert("vehicleIds".into(), PyValue::List(veh_ids));
        self.say(format!(
            "Arranging {n} {} in {} arrangement, please wait.",
            vehicles_word(n),
            type_key.replace('_', " ")
        ));
        self.send(format!(
            "TELEPORT_ARRANGE:{}",
            pyjson::dumps(&PyValue::Dict(body))
        ));
        self.close_modal(true);
    }
}
