//! Catalog filtering, the speech for every screen, and the modal's key handlers.

use std::collections::BTreeSet;
use std::sync::Arc;

use beam_base::pyval::{PyDict, PyValue};

use super::place3d::{offset_phrase, ref_phrase, rotation_phrase};
use super::{
    ACTIVE_VEHICLES_EVENT, Item, PLACE_DEFAULTS, PlaceMode, Screen, Vs, choice_index, ctrl_pressed,
    deps, dget, py_index, py_len, py_lower, py_or, py_seq, pyjson, say_now, shift_pressed,
    vehicles_word, vget, with_vs,
};
use crate::keyboard::KeyEvent;

/// Filter categories, pulled from catalog metadata. "police" is synthetic: set to "Yes"
/// on a vehicle any of whose configs has Config Type "Police", so checking it filters
/// down to police-equipped vehicles.
pub const FILTER_CATEGORIES: [(&str, &str); 7] = [
    ("police", "Police"),
    ("type", "Type"),
    ("brand", "Brand"),
    ("bodyStyle", "Body style"),
    ("drivetrain", "Drivetrain"),
    ("propulsion", "Propulsion"),
    ("country", "Country"),
];

/// `_PAGES`: what Tab cycles through.
pub(crate) const PAGES: [Screen; 3] = [Screen::Main, Screen::ToSpawn, Screen::Manage];

fn page_header(s: Screen) -> &'static str {
    match s {
        Screen::Main => "Vehicle list",
        Screen::ToSpawn => "To be spawned",
        _ => "Vehicle control",
    }
}

/// `_PAGE_JUMP`.
const PAGE_JUMP: usize = 20;

/// `v.get("configs", [])`.
fn configs_of(v: &PyDict) -> PyValue {
    v.get("configs")
        .cloned()
        .unwrap_or_else(|| PyValue::List(Vec::new()))
}

/// `cfg.get("key", "")`.
fn key_or_empty(cfg: &PyValue) -> PyValue {
    match cfg {
        PyValue::Dict(d) => d.get("key").cloned().unwrap_or_else(|| PyValue::from("")),
        _ => vget(cfg, "key").clone(),
    }
}

/// `_vehicle_summary(v)`.
pub(crate) fn vehicle_summary(v: &PyDict) -> String {
    let unknown = PyValue::from("unknown");
    let empty = PyValue::from("");
    let name = py_or(&[dget(v, "name"), dget(v, "model"), &unknown]);
    let brand = py_or(&[dget(v, "brand"), &empty]);
    let mut parts = Vec::new();
    if brand.truthy() && !py_lower(name).contains(&py_lower(brand)) {
        parts.push(format!("{} {}", brand.py_str(), name.py_str()));
    } else {
        match name {
            PyValue::Str(s) => parts.push(s.clone()),
            _ => panic!("sequence item 0: expected str instance"),
        }
    }
    let ncfg = py_len(&configs_of(v));
    if ncfg > 1 {
        parts.push(format!("{ncfg} configurations"));
    }
    parts.join(", ")
}

/// `display = v.get("name") or v.get("model") or "vehicle"`, plus the configuration name
/// unless it is the default.
fn queued_display(v: &PyDict, cfg: &PyValue) -> String {
    let vehicle = PyValue::from("vehicle");
    let empty = PyValue::from("");
    let display = py_or(&[dget(v, "name"), dget(v, "model"), &vehicle]).py_str();
    let cfg_name = py_or(&[vget(cfg, "name"), vget(cfg, "key"), &empty]);
    if cfg_name.truthy() {
        let low = py_lower(cfg_name);
        if low != "default" && low != "(default)" {
            return format!("{display} \u{2014} {}", cfg_name.py_str());
        }
    }
    display
}

/// `_to_spawn_summary(item)`.
pub(crate) fn to_spawn_summary(item: &Item) -> String {
    let base = if !item.display_name.is_empty() {
        item.display_name.clone()
    } else if item.model.truthy() {
        item.model.py_str()
    } else {
        "vehicle".to_string()
    };
    if item.replace_veh_id.is_some() {
        let replaced = item.replace_veh_name.clone().unwrap_or_default();
        let target = match item.replace_slot {
            Some(slot) if slot != 0 && !replaced.is_empty() => {
                beam_base::pyfmt::py_strip(&format!("slot {slot} {replaced}")).to_string()
            }
            _ => "vehicle".to_string(),
        };
        return format!("Replace {target} with {base}");
    }
    format!(
        "{base}, {} from {}, {}",
        offset_phrase(&item.vals),
        ref_phrase(item),
        rotation_phrase(&item.vals)
    )
}

impl Vs {
    // ---- Catalog filtering ----------------------------------------------------------------

    /// `_filtered_catalog()`.
    pub(crate) fn filtered_catalog(&self) -> Vec<Arc<PyDict>> {
        if self.filters.is_empty() {
            return self.catalog.clone();
        }
        // With the Police filter on, a vehicle's configs are cut to its police configs
        // too, or drilling into a multi-config model would look unfiltered.
        let police_only = self.filters.get("police").is_some_and(|s| !s.is_empty());
        let mut out = Vec::new();
        for v in &self.catalog {
            let mut keep = true;
            for (key, allowed) in &self.filters {
                if allowed.is_empty() {
                    continue;
                }
                let val = v.get(key.as_str());
                let inside = match val {
                    Some(PyValue::Str(s)) => allowed.contains(s),
                    _ => false,
                };
                if !inside {
                    keep = false;
                    break;
                }
            }
            if !keep {
                continue;
            }
            if police_only {
                let police_cfgs: Vec<PyValue> = py_seq(&configs_of(v))
                    .into_iter()
                    .filter(|c| *vget(c, "configType") == PyValue::from("Police"))
                    .collect();
                if police_cfgs.is_empty() {
                    // Flagged hasPolice in Lua but no config carries the type: skip.
                    continue;
                }
                let mut d = (**v).clone();
                d.insert("configs".to_string(), PyValue::List(police_cfgs));
                out.push(Arc::new(d));
                continue;
            }
            out.push(Arc::clone(v));
        }
        out
    }

    /// `_filter_value_options(category)`: distinct non-empty values, sorted by `lower()`.
    /// (Case-only duplicates keep first-seen order; the Python's order there comes from
    /// set iteration.)
    fn filter_value_options(&self, category: &str) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut non_str = None;
        for v in &self.catalog {
            let val = dget(v, category);
            if val.truthy() {
                match val {
                    PyValue::Str(s) => {
                        if !seen.contains(s) {
                            seen.push(s.clone());
                        }
                    }
                    other => non_str = Some(other.clone()),
                }
            }
        }
        if let Some(bad) = non_str {
            py_lower(&bad); // raises, as the sort key does
        }
        let mut keyed: Vec<(String, String)> =
            seen.into_iter().map(|s| (s.to_lowercase(), s)).collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        keyed.into_iter().map(|(_, s)| s).collect()
    }

    /// `_filter_dialog_lines()`: (category_key, value, label_with_state).
    pub(crate) fn filter_dialog_lines(&self) -> Vec<(String, String, String)> {
        let mut rows = Vec::new();
        let empty = BTreeSet::new();
        for (cat_key, cat_label) in FILTER_CATEGORIES {
            let values = self.filter_value_options(cat_key);
            if values.is_empty() {
                continue;
            }
            let selected = self
                .filter_draft
                .as_ref()
                .and_then(|d| d.get(cat_key))
                .unwrap_or(&empty);
            for val in values {
                let mark = if selected.contains(&val) {
                    "checked"
                } else {
                    "unchecked"
                };
                let label = format!("{cat_label}: {val}, {mark}");
                rows.push((cat_key.to_string(), val, label));
            }
        }
        rows
    }

    // ---- Speech helpers -------------------------------------------------------------------

    /// `_ref_options()`: (label, key), filtered by the wizard target's queue position.
    pub(crate) fn ref_options(&self) -> Vec<(&'static str, &'static str)> {
        if self.place_mode == PlaceMode::Teleport {
            // No "current vehicle": it collapses into "itself" whenever the highlighted
            // vehicle is the player's. The player vehicle is still reachable by marking.
            return vec![
                ("itself", "self"),
                ("the ground below the camera", "ground"),
                ("mark a vehicle", "mark"),
            ];
        }
        let mut opts = vec![("current vehicle", "current"), ("mark a vehicle", "mark")];
        if let Some(idx) = self.wizard_target_idx {
            if idx > 0 {
                opts.push(("previous in queue", "prev"));
            }
            if (idx as i64) < self.to_spawn.len() as i64 - 1 {
                opts.push(("next in queue", "next"));
            }
        }
        opts
    }

    /// `_say_with_prefix(msg)`: speak with the staged header, consuming it.
    pub(crate) fn say_with_prefix(&mut self, msg: &str) {
        let prefix = self.pending_prefix.take().unwrap_or_default();
        if prefix.is_empty() {
            self.say(msg);
        } else {
            self.say(format!("{prefix}. {msg}"));
        }
    }

    /// `_speak_position(idx, total, content)`.
    pub(crate) fn speak_position(&mut self, idx: usize, total: usize, content: &str) {
        if total == 0 {
            self.say_with_prefix("Empty list.");
            return;
        }
        self.say_with_prefix(&format!("{content}. {} of {total}.", idx + 1));
    }

    fn drill_truthy(&self) -> Option<Arc<PyDict>> {
        self.drill_vehicle.clone().filter(|d| !d.is_empty())
    }

    /// `_speak_current()`: speak the item under the current screen's cursor.
    pub(crate) fn speak_current(&mut self) {
        match self.screen {
            Screen::Main => {
                let items = self.filtered_catalog();
                if items.is_empty() {
                    self.say_with_prefix("No vehicles match current filters.");
                    return;
                }
                self.idx_main = self.idx_main.min(items.len() - 1);
                let s = vehicle_summary(&items[self.idx_main]);
                self.speak_position(self.idx_main, items.len(), &s);
            }
            Screen::Configs => {
                let Some(drill) = self.drill_truthy() else {
                    self.say_with_prefix("No vehicle selected.");
                    return;
                };
                let cfgs_v = configs_of(&drill);
                if !cfgs_v.truthy() {
                    self.say_with_prefix("No configurations.");
                    return;
                }
                let cfgs = py_seq(&cfgs_v);
                self.idx_configs = self.idx_configs.min(cfgs.len() - 1);
                let cfg = &cfgs[self.idx_configs];
                let default = PyValue::from("default");
                let label_v = py_or(&[vget(cfg, "name"), vget(cfg, "key"), &default]);
                let label = if vget(cfg, "isDefault").truthy() {
                    match label_v {
                        PyValue::Str(s) => format!("{s}, default"),
                        _ => panic!("unsupported operand type(s) for +=: and 'str'"),
                    }
                } else {
                    label_v.py_str()
                };
                self.speak_position(self.idx_configs, cfgs.len(), &label);
            }
            Screen::ToSpawn => {
                if self.to_spawn.is_empty() {
                    self.say_with_prefix("To be spawned list is empty.");
                    return;
                }
                self.idx_to_spawn = self.idx_to_spawn.min(self.to_spawn.len() - 1);
                let s = to_spawn_summary(&self.to_spawn[self.idx_to_spawn]);
                self.speak_position(self.idx_to_spawn, self.to_spawn.len(), &s);
            }
            Screen::Manage => {
                let mv = self.manage_vehicles.clone();
                if mv.is_empty() {
                    self.say_with_prefix("No vehicles in the world.");
                    return;
                }
                self.idx_manage = self.idx_manage.min(mv.len() - 1);
                let (vid, name) = &mv[self.idx_manage];
                let marker = if self.manage_selected.contains(vid) {
                    "selected"
                } else {
                    "not selected"
                };
                self.speak_position(
                    self.idx_manage,
                    mv.len(),
                    &format!("{name} (id {vid}), {marker}"),
                );
            }
            Screen::Filter => {
                let rows = self.filter_dialog_lines();
                if rows.is_empty() {
                    self.say_with_prefix("No filter options available.");
                    return;
                }
                self.idx_filter = self.idx_filter.min(rows.len() - 1);
                let label = rows[self.idx_filter].2.clone();
                self.speak_position(self.idx_filter, rows.len(), &label);
            }
            Screen::Place3d => {
                let t = self.speak_place3d_text(true);
                self.say_with_prefix(&t);
            }
            Screen::SpacingEdit => {
                let t = format!("{} feet", self.spacing_field.spoken_value());
                self.say_with_prefix(&t);
            }
            Screen::Ref => {
                let opts = self.ref_options();
                if opts.is_empty() {
                    self.say_with_prefix("No reference options available.");
                    return;
                }
                self.idx_ref = self.idx_ref.min(opts.len() - 1);
                self.speak_position(self.idx_ref, opts.len(), opts[self.idx_ref].0);
            }
            Screen::MarkPicker => {
                let avs = self.active_vehicles.clone();
                if avs.is_empty() {
                    self.say_with_prefix("No active vehicles to mark.");
                    return;
                }
                self.idx_mark = self.idx_mark.min(avs.len() - 1);
                let (vid, name) = &avs[self.idx_mark];
                self.speak_position(self.idx_mark, avs.len(), &format!("{name} (id {vid})"));
            }
            Screen::ReplaceSlot => {
                let mut slot_nums: Vec<i64> = self.slots.keys().copied().collect();
                slot_nums.sort();
                if slot_nums.is_empty() {
                    self.say_with_prefix("No vehicles in slots.");
                    return;
                }
                self.idx_replace_slot = self.idx_replace_slot.min(slot_nums.len() - 1);
                let sn = slot_nums[self.idx_replace_slot];
                let name = self
                    .slots
                    .get(&sn)
                    .map(|i| i.name.clone())
                    .unwrap_or_default();
                let mut label = format!("Slot {sn}: {name}");
                if sn == 1 {
                    label.push_str(", player vehicle");
                }
                self.speak_position(self.idx_replace_slot, slot_nums.len(), &label);
            }
            Screen::Info => {
                if self.info_lines.is_empty() {
                    self.say_with_prefix("No vehicle information.");
                    return;
                }
                self.idx_info = self.idx_info.min(self.info_lines.len() - 1);
                let line = self.info_lines[self.idx_info].clone();
                self.speak_position(self.idx_info, self.info_lines.len(), &line);
            }
            Screen::Arrange => self.speak_arrange_item(self.idx_arrange),
        }
    }

    /// `_enter_screen(screen, announce, header)`. The header is staged so it is spoken
    /// with the first item as one utterance (interrupt=True would drop it otherwise).
    pub(crate) fn enter_screen(&mut self, screen: Screen, announce: bool, header: Option<String>) {
        self.screen = screen;
        if announce {
            self.pending_prefix = header;
            self.speak_current();
        }
    }

    // ---- Key handlers ---------------------------------------------------------------------

    /// The handler `_MODAL_KEYS` maps `name` to (all but `i`).
    pub(crate) fn on_key(&mut self, name: &str, _event: &KeyEvent) {
        match name {
            "up" => self.on_up(),
            "down" => self.on_down(),
            "left" => self.on_left(),
            "right" => self.on_right(),
            "home" => self.on_home(),
            "end" => self.on_end(),
            "page up" => self.on_page_up(),
            "page down" => self.on_page_down(),
            "tab" => self.on_tab(),
            "enter" => self.on_enter(),
            "esc" => self.on_escape(),
            "space" => self.on_space(),
            "delete" => self.on_delete(),
            "f" => self.on_f(),
            "c" => self.on_c(),
            "g" => self.on_g(),
            "w" => self.on_w(),
            "r" => self.on_r(),
            "v" => self.on_v(),
            "a" => self.on_a(),
            "x" => self.on_x(),
            "s" | "d" | "q" | "e" => self.on_place_letter(name),
            "1" | "2" | "3" | "4" => self.on_place_digit(name),
            _ => {}
        }
    }

    fn n_configs(&self) -> usize {
        match self.drill_truthy() {
            Some(d) => py_len(&configs_of(&d)),
            None => 0,
        }
    }

    fn sorted_slot_count(&self) -> usize {
        self.slots.len()
    }

    fn on_up(&mut self) {
        match self.screen {
            Screen::Place3d => return self.place_key_down("up"),
            Screen::Info => {
                self.idx_info = self.idx_info.saturating_sub(1);
                return self.speak_current();
            }
            Screen::Arrange => {
                self.idx_arrange = self.idx_arrange.saturating_sub(1);
                return self.speak_current();
            }
            Screen::SpacingEdit => {
                self.spacing_field.inc();
                let t = format!("{} feet", self.spacing_field.spoken_value());
                return self.say(t);
            }
            Screen::Main => self.idx_main = self.idx_main.saturating_sub(1),
            Screen::Configs => self.idx_configs = self.idx_configs.saturating_sub(1),
            Screen::ToSpawn => self.idx_to_spawn = self.idx_to_spawn.saturating_sub(1),
            Screen::Manage => self.idx_manage = self.idx_manage.saturating_sub(1),
            Screen::Filter => self.idx_filter = self.idx_filter.saturating_sub(1),
            Screen::Ref => self.idx_ref = self.idx_ref.saturating_sub(1),
            Screen::MarkPicker => self.idx_mark = self.idx_mark.saturating_sub(1),
            Screen::ReplaceSlot => self.idx_replace_slot = self.idx_replace_slot.saturating_sub(1),
        }
        self.speak_current();
    }

    fn on_down(&mut self) {
        let step = |idx: usize, n: usize| (idx + 1).min(n.saturating_sub(1));
        match self.screen {
            Screen::Place3d => return self.place_key_down("down"),
            Screen::Info => {
                self.idx_info = step(self.idx_info, self.info_lines.len());
                return self.speak_current();
            }
            Screen::Arrange => {
                self.idx_arrange = (self.idx_arrange + 1).min(4);
                return self.speak_current();
            }
            Screen::SpacingEdit => {
                self.spacing_field.dec();
                let t = format!("{} feet", self.spacing_field.spoken_value());
                return self.say(t);
            }
            Screen::Main => {
                let n = self.filtered_catalog().len();
                self.idx_main = step(self.idx_main, n);
            }
            Screen::Configs => self.idx_configs = step(self.idx_configs, self.n_configs()),
            Screen::ToSpawn => self.idx_to_spawn = step(self.idx_to_spawn, self.to_spawn.len()),
            Screen::Manage => self.idx_manage = step(self.idx_manage, self.manage_vehicles.len()),
            Screen::Filter => {
                let n = self.filter_dialog_lines().len();
                self.idx_filter = step(self.idx_filter, n);
            }
            Screen::Ref => self.idx_ref = step(self.idx_ref, self.ref_options().len()),
            Screen::MarkPicker => self.idx_mark = step(self.idx_mark, self.active_vehicles.len()),
            Screen::ReplaceSlot => {
                self.idx_replace_slot = step(self.idx_replace_slot, self.sorted_slot_count())
            }
        }
        self.speak_current();
    }

    fn arrange_cycle(&mut self, delta: i64) {
        if self.idx_arrange == 0 {
            let n = super::ARRANGE_TYPES.len() as i64;
            self.arrange_type_idx = (self.arrange_type_idx as i64 + delta).rem_euclid(n) as usize;
            self.arrange_variant_idx = 0;
        } else if self.idx_arrange == 1 {
            let n = super::ARRANGE_TYPES[self.arrange_type_idx].2.len() as i64;
            self.arrange_variant_idx =
                (self.arrange_variant_idx as i64 + delta).rem_euclid(n) as usize;
        }
        self.speak_current();
    }

    fn on_left(&mut self) {
        match self.screen {
            Screen::Place3d => self.place_key_down("left"),
            Screen::SpacingEdit => {
                self.spacing_field.move_left();
                let t = self.spacing_field.current_digit().to_string();
                self.say(t);
            }
            Screen::Arrange => self.arrange_cycle(-1),
            Screen::Configs => self.enter_screen(Screen::Main, true, None),
            _ => {}
        }
    }

    fn on_right(&mut self) {
        match self.screen {
            Screen::Place3d => self.place_key_down("right"),
            Screen::SpacingEdit => {
                self.spacing_field.move_right();
                let t = self.spacing_field.current_digit().to_string();
                self.say(t);
            }
            Screen::Arrange => self.arrange_cycle(1),
            Screen::Main => self.drill_into_selected(),
            _ => {}
        }
    }

    /// `_drill_into_selected()`.
    fn drill_into_selected(&mut self) {
        let items = self.filtered_catalog();
        if items.is_empty() {
            self.say("No vehicles.");
            return;
        }
        let drill = Arc::clone(py_index(&items, self.idx_main));
        self.drill_vehicle = Some(Arc::clone(&drill));
        self.idx_configs = 0;
        let header = vehicle_summary(&drill);
        self.enter_screen(Screen::Configs, true, Some(header));
    }

    /// `_on_tab(event)`.
    fn on_tab(&mut self) {
        let Some(cur) = PAGES.iter().position(|p| *p == self.screen) else {
            return;
        };
        let direction: i64 = if shift_pressed() { -1 } else { 1 };
        let nxt = PAGES[(cur as i64 + direction).rem_euclid(PAGES.len() as i64) as usize];
        if nxt == Screen::Manage {
            self.pending_manage_announce = true;
            ACTIVE_VEHICLES_EVENT.clear();
            self.send("REQUEST_ACTIVE_VEHICLES");
        }
        self.enter_screen(nxt, true, Some(page_header(nxt).to_string()));
    }

    fn new_item(&self, v: &PyDict, cfg: &PyValue) -> Item {
        Item {
            model: dget(v, "model").clone(),
            config: key_or_empty(cfg),
            display_name: queued_display(v, cfg),
            ref_mode: "auto".to_string(),
            ref_veh_id: None,
            replace_slot: None,
            replace_veh_id: None,
            replace_veh_name: None,
            vals: PLACE_DEFAULTS,
        }
    }

    /// `_on_enter(event)`.
    fn on_enter(&mut self) {
        match self.screen {
            Screen::Place3d => self.commit_place3d(),
            Screen::Arrange => {
                if self.idx_arrange == 2 {
                    self.spacing_field.home();
                    self.enter_screen(
                        Screen::SpacingEdit,
                        true,
                        Some("Spacing, in feet".to_string()),
                    );
                } else if self.idx_arrange == 3 {
                    self.do_arrange_queue();
                } else if self.idx_arrange == 4 {
                    self.do_arrange_active();
                }
            }
            Screen::SpacingEdit => self.enter_screen(Screen::Arrange, true, None),
            Screen::Main => self.drill_into_selected(),
            Screen::Configs => {
                let drill = self.drill_truthy();
                let cfgs_v = match &drill {
                    Some(d) => configs_of(d),
                    None => PyValue::List(Vec::new()),
                };
                if !cfgs_v.truthy() {
                    return;
                }
                let cfgs = py_seq(&cfgs_v);
                let cfg = py_index(&cfgs, self.idx_configs).clone();
                let v = drill.unwrap_or_default();
                // Default placement: PLACE_DEFAULTS relative to the previous item in the
                // batch (or the player vehicle, or the camera+ground snap).
                let item = self.new_item(&v, &cfg);
                if shift_pressed() {
                    if self.slots.is_empty() {
                        self.say("No slot data available yet.");
                        return;
                    }
                    self.pending_replace_item = Some(item);
                    self.idx_replace_slot = 0;
                    self.enter_screen(
                        Screen::ReplaceSlot,
                        true,
                        Some("Choose vehicle to replace".to_string()),
                    );
                    return;
                }
                let display = item.display_name.clone();
                self.to_spawn.push(item);
                let n = self.to_spawn.len();
                self.say(format!("Added {display}. {n} queued."));
            }
            Screen::ReplaceSlot => {
                let mut slot_nums: Vec<i64> = self.slots.keys().copied().collect();
                slot_nums.sort();
                if slot_nums.is_empty() {
                    return;
                }
                self.idx_replace_slot = self.idx_replace_slot.min(slot_nums.len() - 1);
                let sn = slot_nums[self.idx_replace_slot];
                let Some(info) = self.slots.get(&sn).cloned() else {
                    return;
                };
                if let Some(pending) = self.pending_replace_item.clone() {
                    let mut item = pending;
                    item.replace_slot = Some(sn);
                    item.replace_veh_id = Some(info.id);
                    item.replace_veh_name = Some(info.name.clone());
                    let display = item.display_name.clone();
                    self.to_spawn.push(item);
                    let n = self.to_spawn.len();
                    self.say(format!(
                        "Will replace slot {sn} {} with {display}. {n} queued.",
                        info.name
                    ));
                    self.pending_replace_item = None;
                    self.enter_screen(Screen::Configs, true, None);
                } else if let Some(idx) = self.replace_editing_idx {
                    if let Some(item) = self.to_spawn.get_mut(idx) {
                        item.replace_slot = Some(sn);
                        item.replace_veh_id = Some(info.id);
                        item.replace_veh_name = Some(info.name.clone());
                    }
                    self.say(format!("Will replace slot {sn} {}.", info.name));
                    self.replace_editing_idx = None;
                    self.enter_screen(Screen::ToSpawn, true, None);
                }
            }
            Screen::Filter => {
                // ENTER applies the draft and exits the dialog.
                let draft = self.filter_draft.take().unwrap_or_default();
                self.filters = draft.into_iter().filter(|(_, v)| !v.is_empty()).collect();
                self.say("Filters applied.");
                self.enter_screen(Screen::Main, true, None);
            }
            Screen::Ref => {
                let opts = self.ref_options();
                if opts.is_empty() {
                    return;
                }
                let key = py_index(&opts, self.idx_ref).1;
                match key {
                    "self" => {
                        let (id, name) = (self.tp_veh_id, self.tp_veh_name.clone());
                        self.set_wizard_ref("vehicle", id, name);
                    }
                    // "camera" is what Lua's resolveReferenceFrame calls the spot under
                    // the camera, facing the way the camera looks.
                    "ground" => self.set_wizard_ref("camera", None, Some("the ground".into())),
                    "current" => self.set_wizard_ref("vehicle", None, None),
                    "mark" => {
                        self.request_active_vehicles();
                        self.mark_target_idx = self.wizard_target_idx;
                        self.enter_screen(
                            Screen::MarkPicker,
                            true,
                            Some("Mark a vehicle".to_string()),
                        );
                    }
                    "prev" => self.set_wizard_ref("prev", None, Some("previous in queue".into())),
                    "next" => self.set_wizard_ref("next", None, Some("next in queue".into())),
                    _ => {}
                }
            }
            Screen::Manage => {
                if self.manage_vehicles.is_empty() {
                    return;
                }
                let (vid, name) = py_index(&self.manage_vehicles, self.idx_manage).clone();
                if self.manage_selected.contains(&vid) {
                    self.manage_selected.remove(&vid);
                    self.say(format!("{name} unmarked."));
                } else {
                    self.manage_selected.insert(vid);
                    self.say(format!("{name} marked."));
                }
            }
            Screen::MarkPicker => {
                if self.active_vehicles.is_empty() {
                    return;
                }
                let (vid, name) = py_index(&self.active_vehicles, self.idx_mark).clone();
                // Marks are real in-world vehicles, so a queued item can never end up
                // anchored to itself.
                self.set_wizard_ref("vehicle", Some(vid), Some(name));
            }
            Screen::ToSpawn | Screen::Info => {}
        }
    }

    /// `_on_escape(event)`.
    fn on_escape(&mut self) {
        match self.screen {
            Screen::SpacingEdit => self.enter_screen(Screen::Arrange, true, None),
            // Straight back to the screen and cursor `i` was pressed on: every index is
            // kept, so this is the whole restore.
            Screen::Info => self.enter_screen(self.info_return_screen, true, None),
            Screen::Arrange => self.enter_screen(self.arrange_return_screen, true, None),
            Screen::Configs => {
                self.drill_vehicle = None;
                self.enter_screen(Screen::Main, true, None);
            }
            Screen::Filter => {
                self.filter_draft = None;
                self.say("Filters cancelled.");
                self.enter_screen(Screen::Main, true, None);
            }
            Screen::Place3d => self.cancel_place3d(),
            Screen::Ref | Screen::MarkPicker => {
                let back = self.wizard_return_screen();
                self.clear_wizard();
                self.say("Placement cancelled.");
                self.enter_screen(back, true, None);
            }
            Screen::ReplaceSlot => {
                let came_from = if self.replace_editing_idx.is_some() {
                    Screen::ToSpawn
                } else {
                    Screen::Configs
                };
                self.pending_replace_item = None;
                self.replace_editing_idx = None;
                self.say("Replacement cancelled.");
                self.enter_screen(came_from, true, None);
            }
            // Top-level screens close the modal.
            Screen::Main | Screen::ToSpawn | Screen::Manage => self.close_modal(false),
        }
    }

    /// `_on_space(event)`.
    fn on_space(&mut self) {
        match self.screen {
            Screen::Place3d => {
                let t = self.speak_place3d_text(true);
                self.say(t);
            }
            Screen::Filter => {
                let rows = self.filter_dialog_lines();
                if rows.is_empty() {
                    return;
                }
                let (cat, val, _) = py_index(&rows, self.idx_filter).clone();
                if self.filter_draft.is_none() {
                    self.filter_draft = Some(self.filters.clone());
                }
                let draft = self.filter_draft.get_or_insert_with(Default::default);
                let sel = draft.entry(cat).or_default();
                if sel.contains(&val) {
                    sel.remove(&val);
                    self.say("Unchecked.");
                } else {
                    sel.insert(val);
                    self.say("Checked.");
                }
            }
            // On main, configs or to_spawn: spawn all queued.
            Screen::Main | Screen::Configs | Screen::ToSpawn => self.do_spawn_all(),
            _ => {}
        }
    }

    /// `_on_delete(event)`.
    fn on_delete(&mut self) {
        if self.screen == Screen::ToSpawn {
            if self.to_spawn.is_empty() {
                return;
            }
            if self.idx_to_spawn < self.to_spawn.len() {
                let removed = self.to_spawn.remove(self.idx_to_spawn);
                if self.idx_to_spawn >= self.to_spawn.len() {
                    self.idx_to_spawn = self.to_spawn.len().saturating_sub(1);
                }
                self.say(format!("Removed {}.", removed.display_name));
            }
            self.speak_current();
            return;
        }
        if self.screen == Screen::Manage {
            let ids: Vec<i64> = self.manage_selected.iter().copied().collect();
            if ids.is_empty() {
                self.say("No vehicles selected.");
                return;
            }
            self.say(format!(
                "Removing {} {}.",
                ids.len(),
                vehicles_word(ids.len())
            ));
            self.send(format!("REMOVE:{}", join_ids(&ids)));
            self.manage_selected.clear();
        }
    }

    /// `_on_r(event)`.
    fn on_r(&mut self) {
        match self.screen {
            Screen::Place3d => self.reset_place3d(),
            Screen::Manage => {
                let ids: Vec<i64> = self.manage_selected.iter().copied().collect();
                if ids.is_empty() {
                    self.say("No vehicles selected.");
                    return;
                }
                self.say(format!(
                    "Reloading {} {}.",
                    ids.len(),
                    vehicles_word(ids.len())
                ));
                self.send(format!("RELOAD:{}", join_ids(&ids)));
            }
            Screen::Main => {
                let items = self.filtered_catalog();
                if items.is_empty() {
                    self.say("No vehicles available.");
                    return;
                }
                let v = Arc::clone(&items[choice_index(items.len())]);
                let cfgs_v = configs_of(&v);
                if !cfgs_v.truthy() {
                    self.say("Selected vehicle has no configurations.");
                    return;
                }
                let cfgs = py_seq(&cfgs_v);
                let cfg = cfgs[choice_index(cfgs.len())].clone();
                self.add_random_config(&v, &cfg);
            }
            Screen::Configs => {
                let Some(drill) = self.drill_truthy() else {
                    self.say("No vehicle selected.");
                    return;
                };
                let cfgs_v = configs_of(&drill);
                if !cfgs_v.truthy() {
                    self.say("No configurations available.");
                    return;
                }
                let cfgs = py_seq(&cfgs_v);
                let cfg = cfgs[choice_index(cfgs.len())].clone();
                self.add_random_config(&drill, &cfg);
            }
            _ => {}
        }
    }

    /// `_add_random_config(v, cfg)`.
    fn add_random_config(&mut self, v: &PyDict, cfg: &PyValue) {
        let item = self.new_item(v, cfg);
        let display = item.display_name.clone();
        self.to_spawn.push(item);
        let n = self.to_spawn.len();
        self.say(format!("Added random {display}. {n} queued."));
    }

    /// `_on_v(event)`.
    fn on_v(&mut self) {
        if self.screen != Screen::Manage {
            return;
        }
        self.send("IGNITION_OFF");
        self.say("Ignition off requested.");
    }

    /// `_on_a(event)`: Ctrl+A on manage selects all; bare A is ignored there.
    fn on_a(&mut self) {
        if self.screen == Screen::Place3d {
            self.place_key_down("a");
            return;
        }
        if self.screen != Screen::Manage {
            return;
        }
        if !ctrl_pressed() {
            return;
        }
        self.manage_selected.clear();
        for (vid, _) in &self.manage_vehicles {
            self.manage_selected.insert(*vid);
        }
        let n = self.manage_selected.len();
        self.say(format!("Selected all, {n} {}.", vehicles_word(n)));
    }

    /// `_on_f(event)`.
    fn on_f(&mut self) {
        if self.screen != Screen::Main {
            return;
        }
        self.filter_draft = Some(self.filters.clone());
        self.idx_filter = 0;
        self.enter_screen(Screen::Filter, true, Some("Filters".to_string()));
    }

    /// `_on_c(event)`.
    fn on_c(&mut self) {
        if self.screen != Screen::Main {
            return;
        }
        self.filters = Default::default();
        self.say("Filters cleared.");
        self.speak_current();
    }

    /// `_on_home(event)`.
    fn on_home(&mut self) {
        match self.screen {
            Screen::Arrange => {
                self.idx_arrange = 0;
                return self.speak_current();
            }
            Screen::SpacingEdit => {
                self.spacing_field.home();
                let t = self.spacing_field.current_digit().to_string();
                return self.say(t);
            }
            Screen::Main => self.idx_main = 0,
            Screen::Configs => self.idx_configs = 0,
            Screen::ToSpawn => self.idx_to_spawn = 0,
            Screen::Manage => self.idx_manage = 0,
            Screen::Filter => self.idx_filter = 0,
            Screen::ReplaceSlot => self.idx_replace_slot = 0,
            _ => {}
        }
        self.speak_current();
    }

    /// `_on_end(event)`.
    fn on_end(&mut self) {
        match self.screen {
            Screen::Arrange => {
                self.idx_arrange = 4;
                return self.speak_current();
            }
            Screen::SpacingEdit => {
                self.spacing_field.end();
                let t = self.spacing_field.current_digit().to_string();
                return self.say(t);
            }
            Screen::Main => self.idx_main = self.filtered_catalog().len().saturating_sub(1),
            Screen::Configs => self.idx_configs = self.n_configs().saturating_sub(1),
            Screen::ToSpawn => self.idx_to_spawn = self.to_spawn.len().saturating_sub(1),
            Screen::Manage => self.idx_manage = self.manage_vehicles.len().saturating_sub(1),
            Screen::Filter => self.idx_filter = self.filter_dialog_lines().len().saturating_sub(1),
            Screen::ReplaceSlot => {
                self.idx_replace_slot = self.sorted_slot_count().saturating_sub(1)
            }
            _ => {}
        }
        self.speak_current();
    }

    /// `_on_page_up(event)`.
    fn on_page_up(&mut self) {
        match self.screen {
            Screen::Place3d => return self.place_key_down("page up"),
            Screen::Main => self.idx_main = self.idx_main.saturating_sub(PAGE_JUMP),
            Screen::Configs => self.idx_configs = self.idx_configs.saturating_sub(PAGE_JUMP),
            Screen::ToSpawn => self.idx_to_spawn = self.idx_to_spawn.saturating_sub(PAGE_JUMP),
            Screen::Manage => self.idx_manage = self.idx_manage.saturating_sub(PAGE_JUMP),
            Screen::Filter => self.idx_filter = self.idx_filter.saturating_sub(PAGE_JUMP),
            _ => {}
        }
        self.speak_current();
    }

    /// `_on_page_down(event)`.
    fn on_page_down(&mut self) {
        let jump = |idx: usize, n: usize| (idx + PAGE_JUMP).min(n.saturating_sub(1));
        match self.screen {
            Screen::Place3d => return self.place_key_down("page down"),
            Screen::Main => {
                let n = self.filtered_catalog().len();
                self.idx_main = jump(self.idx_main, n);
            }
            Screen::Configs => self.idx_configs = jump(self.idx_configs, self.n_configs()),
            Screen::ToSpawn => self.idx_to_spawn = jump(self.idx_to_spawn, self.to_spawn.len()),
            Screen::Manage => self.idx_manage = jump(self.idx_manage, self.manage_vehicles.len()),
            Screen::Filter => {
                let n = self.filter_dialog_lines().len();
                self.idx_filter = jump(self.idx_filter, n);
            }
            _ => {}
        }
        self.speak_current();
    }

    /// `_do_spawn_all()`.
    fn do_spawn_all(&mut self) {
        let items = self.to_spawn.clone();
        if items.is_empty() {
            self.say("Nothing to spawn.");
            return;
        }
        // Replacements first, so "current vehicle" refs in the additions resolve to the
        // post-replacement player vehicle. Order kept within each group.
        let ordered: Vec<&Item> = items
            .iter()
            .filter(|it| it.replace_veh_id.is_some())
            .chain(items.iter().filter(|it| it.replace_veh_id.is_none()))
            .collect();
        let mut payload_items = Vec::new();
        for (idx, it) in ordered.iter().enumerate() {
            let mut e = PyDict::new();
            e.insert("queueIdx".into(), PyValue::from(idx as i64));
            e.insert("model".into(), it.model.clone());
            e.insert("config".into(), it.config.clone());
            e.insert(
                "replaceVehId".into(),
                it.replace_veh_id.map_or(PyValue::None, PyValue::from),
            );
            if it.replace_veh_id.is_none() {
                // Every item carries a full placement (PLACE_DEFAULTS at queue time).
                it.vals.insert_into(&mut e);
                e.insert("refMode".into(), PyValue::from(it.ref_mode.clone()));
                e.insert(
                    "refVehId".into(),
                    it.ref_veh_id.map_or(PyValue::None, PyValue::from),
                );
            }
            payload_items.push(PyValue::Dict(e));
        }
        if payload_items.is_empty() {
            self.say("Nothing to spawn.");
            return;
        }
        let n = payload_items.len();
        self.say(format!("Spawning {n} {}, please wait.", vehicles_word(n)));
        let mut body = PyDict::new();
        body.insert("items".into(), PyValue::List(payload_items));
        self.send(format!("SPAWN:{}", pyjson::dumps(&PyValue::Dict(body))));
        // Clear the queue on dispatch so the user starts fresh next time.
        self.to_spawn.clear();
        self.idx_to_spawn = 0;
        // Close silently: SPAWN_OK is announced when the game responds.
        self.close_modal(true);
    }

    /// `_request_active_vehicles()`: don't block; the listener fills the list and the
    /// arrow keys re-speak it.
    fn request_active_vehicles(&mut self) {
        ACTIVE_VEHICLES_EVENT.clear();
        self.send("REQUEST_ACTIVE_VEHICLES");
    }
}

fn join_ids(ids: &[i64]) -> String {
    ids.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// `_on_info(event)`: the specification sheet for the vehicle under the cursor. The
/// catalog list means a model (its default configuration answers), the configs list that
/// exact configuration, and the queue the pair already chosen. Runs on the worker, never
/// the hook, so the bounded wait cannot trip the low-level-hook timeout.
pub(super) fn on_info() {
    let Some(d) = deps() else {
        say_now("Vehicle information is unavailable.");
        return;
    };
    let target = with_vs(|vs| -> Option<(String, String)> {
        let mut model = PyValue::None;
        let mut config = String::new();
        match vs.screen {
            Screen::Main => {
                let items = vs.filtered_catalog();
                if let Some(it) = items.get(vs.idx_main) {
                    model = dget(it, "model").clone();
                }
            }
            Screen::Configs => {
                if let Some(drill) = vs.drill_truthy() {
                    model = dget(&drill, "model").clone();
                    let cfgs_v = configs_of(&drill);
                    if cfgs_v.truthy() && vs.idx_configs < py_len(&cfgs_v) {
                        let cfgs = py_seq(&cfgs_v);
                        let k = vget(&cfgs[vs.idx_configs], "key");
                        if k.truthy() {
                            config = k.py_str();
                        }
                    }
                }
            }
            Screen::ToSpawn => {
                if let Some(it) = vs.to_spawn.get(vs.idx_to_spawn) {
                    model = it.model.clone();
                    if it.config.truthy() {
                        config = it.config.py_str();
                    }
                }
            }
            _ => {}
        }
        if !model.truthy() {
            vs.say("No vehicle information here.");
            return None;
        }
        Some((model.py_str(), config))
    });
    let Some((model, config)) = target else {
        return;
    };
    let (lines, failure) = (d.request_info_fn)(
        Some(model.as_str()),
        Some(config.as_str()),
        super::REQUEST_INFO_TIMEOUT,
    );
    with_vs(|vs| {
        if lines.is_empty() {
            // Every cause is named on the Lua side; a timeout arrives with an empty
            // sentence and is the one case that says nothing about the vehicle.
            match failure {
                Some((_, sentence)) if !sentence.is_empty() => vs.say(sentence),
                _ => vs.say("No vehicle information available."),
            }
            return;
        }
        vs.info_lines = lines;
        vs.idx_info = 0;
        vs.info_return_screen = vs.screen;
        vs.enter_screen(Screen::Info, true, Some("Vehicle information".to_string()));
    });
}
