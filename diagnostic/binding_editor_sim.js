// Replays bnvdaRuntime.js's binding-EDITOR readout against a fake DOM built to
// the shape EditBindingBasicInfo.vue actually emits, with known ground truth.
//
//   node diagnostic/binding_editor_sim.js
//
// The functions under test are LIFTED OUT OF THE SOURCE by name, never copied,
// for the reason binding_readout_sim.js gives: this area's failure mode is a
// readout that is merely WRONG rather than broken -- the destructive button
// announced as the harmless one, or a conflict list sitting on screen unspoken
// -- so a sim carrying its own copy would keep passing across exactly the edit
// that breaks the mod.
//
// Every scenario also asserts what the PREVIOUS form answered, so no check can
// pass for free once the shape it guards against stops being reachable.

const fs = require('fs');
const path = require('path');

const SRC = path.join(__dirname, '..', 'bng_mod', 'ui', 'ui-vue', 'mods', 'bnvda', 'bnvdaRuntime.js');
const source = fs.readFileSync(SRC, 'utf8');

// ---------- lift ----------
function sliceBalanced(name, kind) {
  const needle = kind === 'fn' ? '\n          function ' + name + '(' : '\n          var ' + name + ' = ';
  const start = source.indexOf(needle);
  if (start === -1) throw new Error('could not find ' + kind + ' ' + name + ' in bnvdaRuntime.js');
  const from = source.indexOf(kind === 'fn' ? ')' : '=', start);
  let open = null;
  for (let j = from; j < source.length; j++) {
    if (source[j] === '{' || source[j] === '[') { open = source[j]; break; }
  }
  const close = open === '{' ? '}' : ']';
  let depth = 0, inStr = null, esc = false;
  for (let i = from; i < source.length; i++) {
    const c = source[i];
    if (inStr) {
      if (esc) esc = false;
      else if (c === '\\') esc = true;
      else if (c === inStr) inStr = null;
      continue;
    }
    if (c === '"' || c === "'") { inStr = c; continue; }
    if (c === '/' && source[i + 1] === '/') { i = source.indexOf('\n', i); continue; }
    if (c === open) depth++;
    else if (c === close) { depth--; if (depth === 0) return source.slice(start, i + 1); }
  }
  throw new Error('unbalanced ' + name);
}

// VUE_TARGET_SELECTOR is a plain string literal, so it is taken by line.
function sliceLineVar(name) {
  const m = new RegExp('\\n {10}var ' + name + ' = .*;').exec(source);
  if (!m) throw new Error('could not find var ' + name);
  return m[0];
}

const LIFT_VARS = ['ICON_FRIENDLY_NAMES', 'KEYBOARD_SYMBOLS', 'DEV_FAMILY_PREFIXES', 'P'];
const LIFT_FNS = [
  // binding-name formatting chain (shared with binding_readout_sim.js)
  'iconDeviceFamily', 'buildGlyphMap', 'cleanKeyboardText', 'replaceKnownGlyphs',
  'deviceGlyphFamily', 'activeDeviceFamilies', 'containerGlyphFamily', 'pickBindingVariants',
  'resolveSingleBindingPart', 'bindingContainerFriendlyName', 'getBindingFriendlyName',
  // generic helpers
  'cleanText', 'toArray', 'closest', 'scalarValue', 'vueOwnLabel', 'vueControlState',
  'iconGlyphOf',
  // the binding editor itself
  'bindingActionRole', 'assignedBindingName',
  'vueBindingEditorDisabled', 'vueBindingEditorRow', 'vueBindingEditorLabel',
  'vueBindingEditorValue', 'vueBindingEditorInfo',
  'vueBindingConflictNames', 'announceVueBindingConflicts', 'armSpeakQueue',
];

const lifted = []
  .concat(LIFT_VARS.map(n => sliceBalanced(n, 'var') + ';'))
  .concat([sliceLineVar('VUE_TARGET_SELECTOR')])
  .concat(LIFT_FNS.map(n => sliceBalanced(n, 'fn')))
  .join('\n');

const CONFLICT_QUEUE_MS = Number((source.match(/var VUE_CONFLICT_QUEUE_MS = (\d+);/) || [])[1]);
const TAB_QUEUE_MS = Number((source.match(/var VUE_TAB_QUEUE_MS = (\d+);/) || [])[1]);
const CONFLICT_QUEUE_MAX = Number((source.match(/var VUE_CONFLICT_QUEUE_MAX = (\d+);/) || [])[1]);

const prelude = [
  'var MAX_LEN = 160;',
  'var _glyphToName = null, _glyphToFamily = null;',
  'var _vueBindingConflictKey = "";',
  'var _speakQueueAfterText = "", _speakQueueAfterMs = 0, _speakQueueAfterMax = 0;' +
    'var _speakQueueUntil = 0, _speakQueueLeft = 0;',
  'var VUE_CONFLICT_QUEUE_MS = ' + CONFLICT_QUEUE_MS + ';',
  'var VUE_CONFLICT_QUEUE_MAX = ' + CONFLICT_QUEUE_MAX + ';',
  'function log() {}',
].join('\n');

// ---------- fake DOM ----------
// A small selector engine: tag, .class, [attr], [attr=v], [attr*=v], descendant
// combinators, and ":scope >". That is the whole surface the lifted functions
// touch, and [class*=...] in particular is load-bearing -- the old row walk this
// sim exists to pin down is spelled with it.
let GLYPH_NEXT = 0xE100;
const GLYPHS = {};
function glyphFor(iconName) {
  if (!GLYPHS[iconName]) GLYPHS[iconName] = String.fromCharCode(GLYPH_NEXT++);
  return GLYPHS[iconName];
}

const TOKEN_RE = /([a-zA-Z][\w-]*)|\.([\w-]+)|\[([\w-]+)(?:(\*?=)['"]?([^\]'"]*)['"]?)?\]/g;
function parseCompound(str) {
  const t = { tag: null, classes: [], attrs: [] };
  let m;
  TOKEN_RE.lastIndex = 0;
  while ((m = TOKEN_RE.exec(str))) {
    if (m[1]) t.tag = m[1].toUpperCase();
    else if (m[2]) t.classes.push(m[2]);
    else if (m[3]) t.attrs.push({ name: m[3], op: m[4] || null, val: m[5] || '' });
  }
  return t;
}

class ClassList {
  constructor(names) { this._ = (names || []).slice(); }
  contains(n) { return this._.indexOf(n) !== -1; }
  has(n) { return this.contains(n); }
  get value() { return this._.join(' '); }
}

class El {
  constructor(tag, classes, text, attrs) {
    this.tagName = tag.toUpperCase();
    this.classList = new ClassList(classes);
    this.className = (classes || []).join(' ');
    this.ownText = text || '';
    this.attrs = attrs || {};
    this.children = [];
    this.parentElement = null;
  }
  add(child) { child.parentElement = this; this.children.push(child); return this; }
  get textContent() { return this.ownText + this.children.map(c => c.textContent).join(''); }
  get innerText() { return this.textContent; }
  getAttribute(n) {
    if (n === 'class') return this.className;
    return Object.prototype.hasOwnProperty.call(this.attrs, n) ? this.attrs[n] : null;
  }
  descendants(out) { out = out || []; for (const c of this.children) { out.push(c); c.descendants(out); } return out; }

  _matchCompound(c) {
    if (c.tag && this.tagName !== c.tag) return false;
    for (const cls of c.classes) if (!this.classList.contains(cls)) return false;
    for (const a of c.attrs) {
      const v = a.name === 'class' ? this.className : this.getAttribute(a.name);
      if (v === null || v === undefined) return false;
      if (a.op === '=' && String(v) !== a.val) return false;
      if (a.op === '*=' && String(v).indexOf(a.val) === -1) return false;
    }
    return true;
  }
  // Descendant combinators only, matched right to left.
  _matchComplex(steps) {
    if (!this._matchCompound(steps[steps.length - 1])) return false;
    let node = this.parentElement;
    for (let i = steps.length - 2; i >= 0; i--) {
      while (node && !node._matchCompound(steps[i])) node = node.parentElement;
      if (!node) return false;
      node = node.parentElement;
    }
    return true;
  }
  matches(sel) {
    return sel.split(',').some(part => {
      const steps = part.replace(':scope >', '').trim().split(/\s+/).filter(Boolean).map(parseCompound);
      return steps.length ? this._matchComplex(steps) : false;
    });
  }
  querySelectorAll(sel) {
    const out = [];
    for (const rawPart of sel.split(',')) {
      const part = rawPart.trim();
      if (!part) continue;
      const scoped = part.indexOf(':scope >') === 0;
      const steps = part.replace(':scope >', '').trim().split(/\s+/).filter(Boolean).map(parseCompound);
      if (!steps.length) continue;
      const pool = scoped ? this.children : this.descendants();
      for (const node of pool) {
        if (scoped ? node._matchCompound(steps[0]) : node._matchComplex(steps)) {
          if (out.indexOf(node) === -1) out.push(node);
        }
      }
    }
    return out;
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
  closest(sel) {
    let node = this;
    while (node) { if (node.matches(sel)) return node; node = node.parentElement; }
    return null;
  }
  contains(node) {
    while (node) { if (node === this) return true; node = node.parentElement; }
    return false;
  }
}

const el = (tag, classes, text, attrs) => new El(tag, classes, text, attrs);
const NAV = { 'bng-nav-item': '', tabindex: '0' };

// ---------- the popup, built to the shape of the real 0.39 DOM dump ----------
const iconPart = name => el('span', ['bng-binding-icon', 'icon-base'], glyphFor(name));
function bindingWrapper(iconName) {
  const w = el('span', ['binding-wrapper', 'binding-theme-light']);
  const c = el('span', ['binding-container']);
  c.add(iconPart(iconName));
  return w.add(c);
}
// BngButton renders .bng-background plus an icon-only <BngIcon class="icon">.
function iconButton(iconName, classes, attrs) {
  const b = el('button', ['bng-button', 'empty', 'l-icon'].concat(classes || []), '', attrs || NAV);
  b.add(el('div', ['bng-background']));
  b.add(el('span', ['icon-base', 'icon'], iconName ? glyphFor(iconName) : ''));
  return b;
}

// EditBindingBasicInfo.vue, 0.39: .detail-controls-actions is a SIBLING of the
// .bng-row, both children of .detail-controls.
function buildPopup(opts) {
  opts = opts || {};
  const popup = el('div', ['binding-detail-shell', 'popup-content'], '', { 'bng-ui-scope': 'options-edit-binding-popup' });
  const container = el('div', ['binding-detail-container']);
  const scroll = el('div', ['detail-content-scroll']);
  const section = el('section', ['detail-section']);

  const controls = el('div', ['detail-controls']);
  const row = el('div', ['bng-row', 'options-item-row', 'detail-control-row'], '', NAV);
  row.add(el('div', ['bng-background', 'bng-row-background']));
  row.add(el('div', ['bng-row-label'], 'Assigned control'));
  const rowContent = el('div', ['bng-row-content']);
  const value = el('div', ['assigned-control-value']);
  value.add(el('span', ['assigned-device-name'], 'XBox Controller 1'));
  value.add(bindingWrapper(opts.assigned || 'xboxA'));
  rowContent.add(value);
  row.add(rowContent);
  controls.add(row);

  const actions = el('div', ['detail-controls-actions']);
  const editBtn = iconButton(opts.editIcon === undefined ? 'edit' : opts.editIcon);
  actions.add(editBtn);
  let deleteBtn = null;
  if (!opts.isNewBinding) {
    deleteBtn = iconButton('trashBin1');
    actions.add(deleteBtn);
  }
  controls.add(actions);
  section.add(controls);

  const conflictRows = [];
  if (opts.conflicts && opts.conflicts.length) {
    const conflicts = el('div', ['detail-conflicts']);
    const header = el('div', ['detail-conflicts-header']);
    header.add(el('span', ['info-label'], 'Info:'));
    header.add(el('span', ['info-text'], 'This control is also assigned to:'));
    conflicts.add(header);
    const list = el('div', ['detail-conflicts-list']);
    for (const name of opts.conflicts) {
      const cr = el('div', ['bng-row', 'options-item-row', 'conflict-row'], '', NAV);
      cr.add(el('div', ['bng-background', 'bng-row-background']));
      const lab = el('div', ['bng-row-label']);
      lab.add(el('span', [], name));
      cr.add(lab);
      const content = el('div', ['bng-row-content']);
      content.add(iconButton('trashBin1', ['no-focus-frame'], { 'bng-nav-item': '', 'bng-no-nav': 'true', tabindex: '-1' }));
      cr.add(content);
      list.add(cr);
      conflictRows.push(cr);
    }
    conflicts.add(list);
    section.add(conflicts);
  }

  scroll.add(section);
  container.add(scroll);
  popup.add(container);
  return { popup, row, editBtn, deleteBtn, conflictRows };
}

// The SAME popup as the game emitted before 0.39, with the two buttons INSIDE
// the assigned row -- the layout the deleted row walk was written against.
function buildPopup038() {
  const popup = el('div', ['binding-detail-shell'], '', { 'bng-ui-scope': 'options-edit-binding-popup' });
  const controls = el('div', ['detail-controls']);
  const row = el('div', ['bng-row', 'options-item-row'], '', NAV);
  row.add(el('div', ['bng-row-label'], 'Assigned control'));
  row.add(bindingWrapper('xboxA'));
  const editBtn = iconButton('edit');
  const deleteBtn = iconButton('trashBin1');
  row.add(editBtn);
  row.add(deleteBtn);
  controls.add(row);
  popup.add(controls);
  return { popup, row, editBtn, deleteBtn };
}

// ---------- environment ----------
const iconCatalog = {};
['xboxA', 'xboxB', 'psCross', 'edit', 'trashBin1'].forEach(n => { iconCatalog[n] = { glyph: glyphFor(n) }; });

let LAST_DEVICES = ['xinput0', 'keyboard0'];
const Controls = { get lastDevices() { return LAST_DEVICES; } };
const fakeWindow = { bngVue: { icons: iconCatalog } };

const SPOKEN = [];
function scheduleSpeak(text, src) { SPOKEN.push({ text, src }); }

const scope = new Function(
  'iconCatalog', 'Controls', 'window', 'scheduleSpeak',
  prelude + '\n' + lifted +
  '\nreturn { vueBindingEditorInfo: vueBindingEditorInfo, bindingActionRole: bindingActionRole,' +
  ' announceVueBindingConflicts: announceVueBindingConflicts,' +
  ' vueBindingConflictNames: vueBindingConflictNames, P: P, toArray: toArray, closest: closest,' +
  ' armedText: function () { return _speakQueueAfterText; },' +
  ' armedMs: function () { return _speakQueueAfterMs; } };'
)(iconCatalog, Controls, fakeWindow, scheduleSpeak);

// What the readout actually says for one element.
function say(element, popup) {
  const info = scope.vueBindingEditorInfo(element, popup);
  if (!info) return null;
  return [info.label, info.state].filter(Boolean).join(', ');
}
function roleOf(element, popup) {
  const info = scope.vueBindingEditorInfo(element, popup);
  return info ? info.role : null;
}

// The PREVIOUS form: walk up to the assigned row, index the buttons in it, and
// fall through to the generic icon-only-button fallback when that finds nothing.
function oldForm(button, popup) {
  const assignedRow = scope.closest(button, '.bng-row, [class*=row]');
  const binding = assignedRow && assignedRow.querySelector('.binding-container, .bng-binding, [class*=binding-container]');
  if (assignedRow && binding && popup.contains(assignedRow)) {
    const actions = scope.toArray(assignedRow.querySelectorAll('button, [role=button]'));
    const i = actions.indexOf(button);
    if (i >= 0) return i === 0 ? 'Reassign' : 'Delete';
  }
  return 'Button';
}

// ---------- scenarios ----------
let failures = 0;
function check(label, got, want) {
  const ok = got === want;
  if (!ok) failures++;
  console.log((ok ? '  PASS ' : '  FAIL ') + label + '  got=' + JSON.stringify(got) + (ok ? '' : '  want=' + JSON.stringify(want)));
}
function scenario(n, title, fn) { console.log('\n[' + n + '] ' + title); fn(); }

scenario(1, '0.39 layout: both action buttons are named', () => {
  const { popup, editBtn, deleteBtn } = buildPopup({ conflicts: [] });
  check('edit button', say(editBtn, popup), 'Reassign, currently A button');
  check('delete button', say(deleteBtn, popup), 'Delete binding, currently A button');
  check('edit role', roleOf(editBtn, popup), 'reassign');
  check('delete role', roleOf(deleteBtn, popup), 'delete');
  check('old form said Button (edit)', oldForm(editBtn, popup), 'Button');
  check('old form said Button (delete)', oldForm(deleteBtn, popup), 'Button');
});

scenario(2, 'the old row walk only ever worked on the 0.38 layout', () => {
  const { popup, editBtn, deleteBtn } = buildPopup038();
  check('old form on old layout (edit)', oldForm(editBtn, popup), 'Reassign');
  check('old form on old layout (delete)', oldForm(deleteBtn, popup), 'Delete');
  check('new form still names it (edit)', roleOf(editBtn, popup), 'reassign');
  check('new form still names it (delete)', roleOf(deleteBtn, popup), 'delete');
});

scenario(3, 'a NEW binding has no trash button, and index would be wrong', () => {
  const { popup, editBtn, deleteBtn } = buildPopup({ isNewBinding: true, conflicts: [] });
  check('no delete button rendered', deleteBtn, null);
  check('lone button is Reassign', roleOf(editBtn, popup), 'reassign');
  // A lone button that is the DELETE one is where an index rule goes wrong.
  const only = buildPopup({ isNewBinding: true, conflicts: [], editIcon: 'trashBin1' });
  check('icon wins over index', roleOf(only.editBtn, only.popup), 'delete');
  check('...where the old form said nothing at all', oldForm(only.editBtn, only.popup), 'Button');
});

scenario(4, 'an unrecognised glyph is NOT guessed at', () => {
  const { popup, editBtn } = buildPopup({ conflicts: [], editIcon: 'someUnknownIcon' });
  check('role', roleOf(editBtn, popup), 'binding-action');
  check('label', say(editBtn, popup), 'Binding action, currently A button');
});

scenario(5, 'the assigned row itself still announces', () => {
  const { popup, row } = buildPopup({ conflicts: [] });
  check('row', say(row, popup), 'Assigned control, A button');
});

scenario(6, 'the conflict summary speaks once, on a CHANGE of the set', () => {
  SPOKEN.length = 0;
  scope.announceVueBindingConflicts(null);
  const names = ['Previous Challenge Filter', 'Clutch', 'Subtab left'];
  const a = buildPopup({ conflicts: names });
  scope.announceVueBindingConflicts(a.popup);
  check('spoke once', SPOKEN.length, 1);
  check('text', SPOKEN[0].text,
    '3 conflicts. This control is also assigned to: Previous Challenge Filter, Clutch, Subtab left.');
  check('priority is SYSTEM', SPOKEN[0].src, scope.P.SYSTEM);
  scope.announceVueBindingConflicts(a.popup);
  check('unchanged set is silent', SPOKEN.length, 1);
  // Reassigning inside the same popup produces a different set.
  const b = buildPopup({ conflicts: ['Map zoom out'] });
  scope.announceVueBindingConflicts(b.popup);
  check('changed set re-announces', SPOKEN.length, 2);
  check('singular', SPOKEN[1].text, '1 conflict. This control is also assigned to: Map zoom out.');
});

scenario(7, 'closing the popup resets, so reopening the same set announces again', () => {
  SPOKEN.length = 0;
  scope.announceVueBindingConflicts(null);
  const a = buildPopup({ conflicts: ['Clutch'] });
  scope.announceVueBindingConflicts(a.popup);
  check('spoke', SPOKEN.length, 1);
  scope.announceVueBindingConflicts(null);
  scope.announceVueBindingConflicts(buildPopup({ conflicts: ['Clutch'] }).popup);
  check('spoke again after close', SPOKEN.length, 2);
});

scenario(8, 'no conflicts is SILENT, not narrated', () => {
  SPOKEN.length = 0;
  scope.announceVueBindingConflicts(null);
  const clean = buildPopup({ conflicts: [] });
  scope.announceVueBindingConflicts(clean.popup);
  check('said nothing', SPOKEN.length, 0);
  check('names empty', scope.vueBindingConflictNames(clean.popup).length, 0);
});

scenario(9, 'the seven conflicts from the real DOM dump are all read', () => {
  const real = ['Previous Challenge Filter', 'Clutch', 'Subtab left', 'Grab focused node',
    'Rotate Left', 'Move camera (or seat) down', 'Map zoom out'];
  const { popup } = buildPopup({ conflicts: real });
  check('names', scope.vueBindingConflictNames(popup).join('|'), real.join('|'));
});

scenario(10, 'the summary arms the speak queue, with its OWN longer window', () => {
  SPOKEN.length = 0;
  scope.announceVueBindingConflicts(null);
  const { popup } = buildPopup({ conflicts: ['Shift Up', 'Clutch'] });
  scope.announceVueBindingConflicts(popup);
  check('armed with the exact text it spoke', scope.armedText(), SPOKEN[0].text);
  check('armed with the conflict window', scope.armedMs(), CONFLICT_QUEUE_MS);
  check('armed with a cap above one', CONFLICT_QUEUE_MAX > 1, true);
  check('which is longer than the tab window', CONFLICT_QUEUE_MS > TAB_QUEUE_MS, true);
});

// The user-visible bug: the summary landed every time and was never heard,
// because the popup's own focus announcement arrived a moment later and
// interrupted it. Drives the REAL emitSpeak against a fake clock, the way
// tab_readout_sim.js scenario 10 does, since no fake DOM can reach this.
scenario(11, 'the focus move that lands after it QUEUES instead of cutting it off', () => {
  let CLOCK = 1000;
  const sent = [];
  const speak = new Function(
    'send', 'nowTS', 'isDebug', 'log', 'state',
    'var P = { POINTER: 1, KEYBOARD: 2, CONTROLLER: 3, SYSTEM: 4 };' +
    'var lastSpoken = "", lastSource = 0, lastSpeakTs = 0;' +
    'var VUE_TAB_QUEUE_MS = ' + TAB_QUEUE_MS + ';' +
    'var VUE_CONFLICT_QUEUE_MS = ' + CONFLICT_QUEUE_MS + ';' +
    'var _speakQueueAfterText = "", _speakQueueAfterMs = 0, _speakQueueAfterMax = 0;' +
    'var _speakQueueUntil = 0, _speakQueueLeft = 0;' +
    '\n' + sliceBalanced('navBurstIsNavSource', 'fn') +
    '\n' + sliceBalanced('armSpeakQueue', 'fn') +
    '\n' + sliceBalanced('emitSpeak', 'fn') +
    'var VUE_CONFLICT_QUEUE_MAX = ' + CONFLICT_QUEUE_MAX + ';' +
    '\nreturn { emitSpeak: emitSpeak, arm: armSpeakQueue, P: P,' +
    ' CONFLICT_MS: VUE_CONFLICT_QUEUE_MS, CONFLICT_MAX: VUE_CONFLICT_QUEUE_MAX };'
  )(o => sent.push(o), () => CLOCK, () => false, () => {}, {});

  const summary = '5 conflicts. This control is also assigned to: Map controller select, ' +
    'Challenge Accept Action, OK / Primary action, Shift Up, Use vehicle trigger (action 1).';

  // The REAL burst, taken from the mod's own speech log on a live reproduction:
  // the filter-type row at +29 ms, then the actual focus landing at +234 ms.
  // Both are P.CONTROLLER, so nothing about the source tells them apart -- which
  // is why a cap of one spent the window on the first and let the second through.
  speak.arm(summary, speak.CONFLICT_MS, speak.CONFLICT_MAX);
  speak.emitSpeak(summary, speak.P.SYSTEM);
  check('the summary itself interrupts', sent[0].interrupt, true);

  CLOCK += 29;
  speak.emitSpeak('Automatic', speak.P.CONTROLLER);
  check('the filter row queues', sent[1].interrupt, false);

  CLOCK += 205;
  speak.emitSpeak('Assigned control, A button', speak.P.CONTROLLER);
  check('the focus landing ALSO queues', sent[2].interrupt, false);

  // Past the window, a genuine move by the driver interrupts again. Measured at
  // +2891 ms after the landing in the same log.
  CLOCK += 2891;
  speak.emitSpeak('B button or Menu button, Back.', speak.P.CONTROLLER);
  check('a later real move interrupts', sent[3].interrupt, true);

  // The cap is what stops a held sweep stacking behind a summary nobody is
  // waiting on any more.
  sent.length = 0;
  CLOCK += 10000;
  speak.arm(summary, speak.CONFLICT_MS, speak.CONFLICT_MAX);
  speak.emitSpeak(summary, speak.P.SYSTEM);
  for (let i = 0; i < speak.CONFLICT_MAX + 3; i++) {
    CLOCK += 60;
    speak.emitSpeak('Item ' + i, speak.P.CONTROLLER);
  }
  const queued = sent.filter(x => x.interrupt === false).length;
  check('at most CONFLICT_MAX queue', queued, speak.CONFLICT_MAX);
  check('the rest interrupt normally', sent[sent.length - 1].interrupt, true);

  // The previous cap of ONE is what shipped and still cut the summary off.
  sent.length = 0;
  CLOCK += 10000;
  speak.arm(summary, speak.CONFLICT_MS, 1);
  speak.emitSpeak(summary, speak.P.SYSTEM);
  CLOCK += 29;
  speak.emitSpeak('Automatic', speak.P.CONTROLLER);
  CLOCK += 205;
  speak.emitSpeak('Assigned control, A button', speak.P.CONTROLLER);
  check('cap of one let the landing through (the bug)', sent[2].interrupt, true);

  // ...and with no arm at all, the very first follow-up cuts it off.
  sent.length = 0;
  CLOCK += 10000;
  speak.emitSpeak(summary, speak.P.SYSTEM);
  CLOCK += 300;
  speak.emitSpeak('Assigned control, A button', speak.P.CONTROLLER);
  check('un-armed, the original behaviour interrupts', sent[1].interrupt, true);
});

scenario(12, 'source contract: icons name the buttons, and order is load-bearing', () => {
  check('matches trashBin1 by catalog', /iconGlyphOf\('trashBin1'\)/.test(source), true);
  check('matches edit by catalog', /iconGlyphOf\('edit'\)/.test(source), true);
  const actionsAt = source.indexOf(".detail-controls-actions');");
  const rowWalkAt = source.indexOf("var assignedRow = closest(control, '.bng-row, [class*=row]');");
  check('actions branch exists', actionsAt > 0, true);
  check('row walk still exists', rowWalkAt > 0, true);
  check('actions branch runs FIRST', actionsAt < rowWalkAt, true);
  check('summary is wired into the poll', /announceVueBindingConflicts\(bindingPopup\);/.test(source), true);
  check('summary arms the speak queue', /armSpeakQueue\(msg, VUE_CONFLICT_QUEUE_MS, VUE_CONFLICT_QUEUE_MAX\)/.test(source), true);
  check('summary resets when the screen goes', /announceVueBindingConflicts\(null\);/.test(source), true);
});

console.log('\n' + (failures ? failures + ' FAILURE(S)' : 'all checks passed'));
process.exit(failures ? 1 : 0);
