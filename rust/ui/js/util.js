// Small DOM helpers shared by the panels. These are the wx helpers from config_ui.py
// (_owns_focus, _enable, _set_row, _bind_spin_double_page_keys) and the bits of wx.Choice
// and wx.SpinCtrl behaviour the panels relied on.

export const $ = (id) => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`Missing element #${id}`);
  return el;
};

// ---------------------------------------------------------------------------
// Choice (wx.Choice) helpers. Options carry {value, label} items from ui_choices; the
// item list is kept so typed values (numbers) come back as they went in.
// ---------------------------------------------------------------------------

const selectItems = new WeakMap();

/** Replace a select's options. items: [{value, label}] or plain strings. */
export function fillSelect(select, items) {
  const list = items.map((it) =>
    typeof it === "object" && it !== null ? it : { value: it, label: String(it) },
  );
  selectItems.set(select, list);
  select.replaceChildren(
    ...list.map((it, i) => {
      const opt = document.createElement("option");
      opt.value = String(i);
      opt.textContent = it.label;
      return opt;
    }),
  );
}

/** The item list a select was filled with. */
export function itemsOf(select) {
  return selectItems.get(select) || [];
}

/** wx GetSelection(): -1 when nothing is selected. */
export function getSelection(select) {
  return select.selectedIndex;
}

/** wx SetSelection(). */
export function setSelection(select, index) {
  select.selectedIndex = index;
}

/** The selected item's value, or undefined. */
export function selectedValue(select) {
  const items = itemsOf(select);
  const i = select.selectedIndex;
  return i >= 0 && i < items.length ? items[i].value : undefined;
}

/** wx GetString(GetSelection()), or "" when nothing is selected. */
export function selectedLabel(select) {
  const i = select.selectedIndex;
  return i >= 0 ? select.options[i].textContent : "";
}

/** Index of the item whose value equals `value` (strict), or -1. */
export function indexOfValue(select, value) {
  return itemsOf(select).findIndex((it) => it.value === value);
}

/** wx FindString(): index of the option whose label equals `text` exactly, or -1. */
export function findString(select, text) {
  return itemsOf(select).findIndex((it) => it.label === text);
}

/** Select by value; when absent select index 0 (the backend normalises values first). */
export function selectValue(select, value) {
  const i = indexOfValue(select, value);
  select.selectedIndex = i >= 0 ? i : itemsOf(select).length ? 0 : -1;
}

// ---------------------------------------------------------------------------
// Focus helpers (_owns_focus, _enable, _set_row)
// ---------------------------------------------------------------------------

/** True if `el` has focus or contains the focused element. */
export function ownsFocus(el) {
  const active = document.activeElement;
  return !!active && (active === el || el.contains(active));
}

/**
 * _enable: enable or disable a control without stranding focus on it. Disabling the
 * focused control drops focus to the document and the screen reader loses its place, so
 * focus moves to `fallback` first.
 */
export function enable(ctrl, enabled, fallback = null) {
  if (!enabled && fallback && ownsFocus(ctrl)) fallback.focus();
  ctrl.disabled = !enabled;
}

/**
 * _set_row: show or hide a dependent row and enable or disable its controls. Focus is
 * rescued before the row is hidden, because a hidden control has already lost focus by the
 * time anything could check it.
 */
export function setRow(row, ctrls, enabled, fallback) {
  if (!enabled && fallback && (ownsFocus(row) || ctrls.some((c) => ownsFocus(c)))) {
    fallback.focus();
  }
  row.hidden = !enabled;
  for (const c of ctrls) c.disabled = !enabled;
}

// ---------------------------------------------------------------------------
// Spin controls (wx.SpinCtrl / wx.SpinCtrlDouble) on <input type="number" step="any">.
//
// The browser's own stepping snaps to multiples of `step` from `min`, so -19.5 stepped up
// would become -19 where wx gives -18.5. Arrow keys are therefore handled here: value plus
// or minus the wx increment, clamped. PageUp/PageDown step by ten increments
// (_bind_spin_double_page_keys). The value is kept rounded to the wx digit count, which
// is what the control displays and what the screen reader announces.
// ---------------------------------------------------------------------------

const spinState = new WeakMap();

function spinParams(input) {
  return {
    min: Number(input.min),
    max: Number(input.max),
    inc: Number(input.dataset.inc || "1"),
    digits: Number(input.dataset.digits || "0"),
    isInt: input.hasAttribute("data-int"),
  };
}

function roundTo(value, digits) {
  return Number(value.toFixed(digits));
}

function clampRound(input, value) {
  const p = spinParams(input);
  let v = Math.min(p.max, Math.max(p.min, value));
  v = roundTo(v, p.digits);
  if (p.isInt) v = Math.trunc(v);
  return v;
}

function showSpin(input, value) {
  const p = spinParams(input);
  input.value = value.toFixed(p.digits);
}

/** wx SetValue(): clamps to the control's range, as wx does. */
export function setSpin(input, value) {
  let n = Number(value);
  if (!Number.isFinite(n)) n = Number(input.min);
  const v = clampRound(input, n);
  spinState.set(input, v);
  showSpin(input, v);
}

/**
 * wx GetValue(): the parsed text, clamped and rounded. Text that does not parse (a lone
 * "-" mid-edit) reads as the last good value.
 */
export function getSpin(input) {
  const n = input.value.trim() === "" ? NaN : Number(input.value);
  if (Number.isFinite(n)) return clampRound(input, n);
  const last = spinState.get(input);
  return last === undefined ? clampRound(input, Number(input.min)) : last;
}

/**
 * Wire a spin control. onChange runs after every user change: arrow and page steps
 * (wx EVT_SPINCTRL / EVT_SPINCTRLDOUBLE), typing, and commit.
 */
export function initSpin(input, onChange) {
  if (!spinState.has(input)) spinState.set(input, clampRound(input, Number(input.min)));
  input.addEventListener("keydown", (e) => {
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    const p = spinParams(input);
    let delta = 0;
    if (e.key === "ArrowUp") delta = p.inc;
    else if (e.key === "ArrowDown") delta = -p.inc;
    else if (e.key === "PageUp") delta = p.inc * 10;
    else if (e.key === "PageDown") delta = -p.inc * 10;
    else return;
    e.preventDefault();
    const v = clampRound(input, getSpin(input) + delta);
    spinState.set(input, v);
    showSpin(input, v);
    onChange();
  });
  input.addEventListener("input", () => {
    const n = Number(input.value);
    if (input.value.trim() !== "" && Number.isFinite(n)) spinState.set(input, clampRound(input, n));
    onChange();
  });
  // Commit (focus loss or Enter): clamp and reformat, as wx does when the text is accepted.
  input.addEventListener("change", () => {
    setSpin(input, getSpin(input));
    onChange();
  });
  // A mouse wheel over a focused number field would otherwise change it with no wx analogue.
  input.addEventListener("wheel", (e) => {
    if (document.activeElement === input) e.preventDefault();
  }, { passive: false });
}

// ---------------------------------------------------------------------------
// Read-only text areas (log, console output)
// ---------------------------------------------------------------------------

/**
 * Append text without moving the caret or selection, so a screen reader's review cursor
 * stays where the user left it.
 */
export function appendPreserving(textarea, text) {
  const end = textarea.value.length;
  textarea.setRangeText(text, end, end, "preserve");
}

// ---------------------------------------------------------------------------
// Serial async queue: keeps saves in the order they were requested.
// ---------------------------------------------------------------------------

export function serialQueue() {
  let tail = Promise.resolve();
  return (fn) => {
    const run = tail.then(fn, fn);
    tail = run.catch(() => {});
    return run;
  };
}
