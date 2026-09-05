// Replays bnvdaRuntime.js's binding-text extraction against a fake DOM built to
// the shape the game's own components emit (bngBinding.vue, Hint.vue), with
// known ground truth.
//
//   node diagnostic/binding_readout_sim.js
//
// The functions under test are LIFTED OUT OF THE SOURCE by name, never copied:
// this whole area's failure mode is a readout that is merely wrong rather than
// broken -- a button announced twice, or the wrong pad's button announced with
// full confidence -- so a sim carrying its own copy of the logic would keep
// passing across exactly the edit that breaks the mod. environment_row_sim.py
// lifts beamtel's functions by AST for the same reason.
//
// Every scenario also asserts what the PREVIOUS form answered, so a check
// cannot pass for free once the shape it guards against stops being reachable.

const fs = require('fs');
const path = require('path');

const SRC = path.join(__dirname, '..', 'bng_mod', 'ui', 'ui-vue', 'mods', 'bnvda', 'bnvdaRuntime.js');
const source = fs.readFileSync(SRC, 'utf8');

// ---------- lift ----------
// Slice a top-level `function name(...) { ... }` or `var name = [...]` out of
// the runtime by brace/bracket matching from its declaration.
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

const LIFT_VARS = ['ICON_FRIENDLY_NAMES', 'KEYBOARD_SYMBOLS', 'DEV_FAMILY_PREFIXES'];
const LIFT_FNS = [
  'iconDeviceFamily', 'buildGlyphMap', 'cleanKeyboardText', 'replaceKnownGlyphs',
  'deviceGlyphFamily', 'activeDeviceFamilies', 'containerGlyphFamily', 'pickBindingVariants',
  'resolveSingleBindingPart', 'bindingContainerFriendlyName', 'getBindingFriendlyName',
  'cleanText', 'toArray',
];

const lifted = []
  .concat(LIFT_VARS.map(n => sliceBalanced(n, 'var') + ';'))
  .concat(LIFT_FNS.map(n => sliceBalanced(n, 'fn')))
  .join('\n');

// The runtime declares these two lazily-built maps beside buildGlyphMap.
const prelude = [
  'var MAX_LEN = 160;',
  'var _glyphToName = null, _glyphToFamily = null;',
  'function log() {}',
].join('\n');

// ---------- fake DOM ----------
// Only the surface the extractors touch: textContent, querySelectorAll with the
// handful of selectors they use (tag, .class, ":scope > x"), matches, closest,
// contains, parentElement, tagName.
let GLYPH_NEXT = 0xE100;
const GLYPHS = {};
function glyphFor(iconName) {
  if (!GLYPHS[iconName]) GLYPHS[iconName] = String.fromCharCode(GLYPH_NEXT++);
  return GLYPHS[iconName];
}

class El {
  constructor(tag, classes, text) {
    this.tagName = tag.toUpperCase();
    this.classList = new Set(classes || []);
    this.ownText = text || '';
    this.children = [];
    this.parentElement = null;
  }
  add(child) { child.parentElement = this; this.children.push(child); return this; }
  get textContent() { return this.ownText + this.children.map(c => c.textContent).join(''); }
  get innerText() { return this.textContent; }
  descendants(out) { out = out || []; for (const c of this.children) { out.push(c); c.descendants(out); } return out; }
  matchesSimple(sel) {
    sel = sel.trim();
    if (sel.charAt(0) === '.') return this.classList.has(sel.slice(1));
    return this.tagName === sel.toUpperCase();
  }
  matches(sel) {
    return sel.split(',').some(part => this.matchesSimple(part.replace(':scope >', '').trim()));
  }
  querySelectorAll(sel) {
    const out = [];
    for (const rawPart of sel.split(',')) {
      const part = rawPart.trim();
      const scoped = part.indexOf(':scope >') === 0;
      const simple = part.replace(':scope >', '').trim();
      const pool = scoped ? this.children : this.descendants();
      for (const node of pool) if (node.matchesSimple(simple) && out.indexOf(node) === -1) out.push(node);
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

const el = (tag, classes, text) => new El(tag, classes, text);

// bngBinding.vue: <BngIcon class="bng-binding-icon"> renders a bare glyph.
const iconPart = iconName => el('span', ['bng-binding-icon', 'icon-base'], glyphFor(iconName));
// bngBinding.vue: <kbd><span><span>LABEL</span></span></kbd>
function kbdPart(label) {
  const kbd = el('kbd');
  const outer = el('span');
  outer.add(el('span', [], label));
  return kbd.add(outer);
}
const container = (...parts) => {
  const c = el('span', ['binding-container']);
  parts.forEach(p => c.add(p));
  return c;
};
const wrapper = (...containers) => {
  const w = el('span', ['binding-wrapper']);
  containers.forEach(c => w.add(c));
  return w;
};

// Hint.vue wraps the whole row in a div that ALSO carries .binding-container.
function hintOf(label, ...wrappers) {
  const hint = el('div', ['hint']);
  const outer = el('div', ['binding-container']);
  const rich = el('span', ['rich']);
  wrappers.forEach(w => rich.add(w));
  outer.add(rich);
  hint.add(outer);
  hint.add(el('span', ['hint-text'], label));
  return hint;
}

// ---------- environment ----------
const iconCatalog = {};
['xboxA', 'xboxB', 'xboxXAxis', 'xboxYAxis', 'psCross', 'mouseLMB'].forEach(name => {
  iconCatalog[name] = { glyph: glyphFor(name) };
});
// An icon in no family prefix and with no ICON_FRIENDLY_NAMES entry: the
// "device we cannot classify" case.
iconCatalog.wheelPaddleL = { glyph: glyphFor('wheelPaddleL') };

let LAST_DEVICES = [];
const Controls = { get lastDevices() { return LAST_DEVICES; } };
const fakeWindow = { bngVue: { icons: iconCatalog } };

const scope = new Function(
  'iconCatalog', 'Controls', 'window',
  prelude + '\n' + lifted +
  '\nreturn { getBindingFriendlyName: getBindingFriendlyName, pickBindingVariants: pickBindingVariants,' +
  ' activeDeviceFamilies: activeDeviceFamilies, containerGlyphFamily: containerGlyphFamily };'
)(iconCatalog, Controls, fakeWindow);

// ---------- the readers ----------
// What the fixed hint path does.
function hintButtonText(hint) {
  let roots = hint.querySelectorAll('.binding-wrapper');
  if (!roots.length) roots = hint.querySelectorAll('.binding-container');
  return scope.pickBindingVariants(roots).map(scope.getBindingFriendlyName).filter(Boolean).join(' or ');
}
// What it did before: every .binding-container, including Hint.vue's own outer
// wrapper div, each read whole.
function hintButtonTextOld(hint) {
  return hint.querySelectorAll('.binding-container').map(scope.getBindingFriendlyName).filter(Boolean).join(' or ');
}
// What getBindingFriendlyName did before: every variant, whatever device.
function allVariantsOld(bindingEl) {
  return bindingEl.querySelectorAll(':scope > .binding-container')
    .map(c => scope.getBindingFriendlyName(c)).filter(Boolean).join(' or ');
}

// ---------- scenarios ----------
let failures = 0;
function check(label, got, want) {
  const ok = got === want;
  if (!ok) failures++;
  console.log((ok ? '  PASS ' : '  FAIL ') + label + '  got=' + JSON.stringify(got) + (ok ? '' : '  want=' + JSON.stringify(want)));
}
function scenario(n, title, fn) { console.log('\n[' + n + '] ' + title); fn(); }

scenario(1, 'Hint.vue nesting: one button is announced once', () => {
  LAST_DEVICES = ['xinput0', 'keyboard0'];
  const hint = hintOf('Select', wrapper(container(iconPart('xboxA'))));
  check('fixed form', hintButtonText(hint), 'A button');
  check('old form doubled it', hintButtonTextOld(hint), 'A button or A button');
});

scenario(2, 'two pads: only the active one is announced', () => {
  const both = wrapper(container(iconPart('xboxA')), container(iconPart('psCross')));
  LAST_DEVICES = ['ps50', 'xinput0', 'keyboard0'];
  check('DualSense in hand', scope.getBindingFriendlyName(both), 'Cross');
  LAST_DEVICES = ['xinput0', 'ps50', 'keyboard0'];
  check('Xbox pad in hand', scope.getBindingFriendlyName(both), 'A button');
  check('old form read both', allVariantsOld(both), 'A button or Cross');
});

scenario(3, 'same-device variants are NOT duplicates and all survive', () => {
  LAST_DEVICES = ['xinput0', 'keyboard0'];
  const axes = wrapper(container(iconPart('xboxXAxis')), container(iconPart('xboxYAxis')));
  check('both axes kept', scope.getBindingFriendlyName(axes), 'Left stick X or Left stick Y');
});

scenario(4, 'keyboard: the glyphless container is a family, not an unknown', () => {
  const mixed = wrapper(container(kbdPart('M')), container(iconPart('xboxA')));
  LAST_DEVICES = ['keyboard0', 'mouse0', 'xinput0'];
  check('keyboard active', scope.getBindingFriendlyName(mixed), 'M');
  LAST_DEVICES = ['xinput0', 'keyboard0'];
  check('pad active', scope.getBindingFriendlyName(mixed), 'A button');
});

scenario(5, 'a pad-only binding is still announced while the keyboard is active', () => {
  LAST_DEVICES = ['keyboard0', 'mouse0', 'xinput0'];
  check('not silenced', scope.getBindingFriendlyName(wrapper(container(iconPart('xboxA')))), 'A button');
});

scenario(6, 'an unclassifiable device falls back to ONE variant, never to both', () => {
  LAST_DEVICES = ['wheel0', 'keyboard0'];
  const odd = wrapper(container(iconPart('wheelPaddleL')), container(iconPart('wheelPaddleL')));
  check('single answer', scope.getBindingFriendlyName(odd).indexOf(' or '), -1);
});

scenario(7, 'combos keep their parts joined with +, not treated as variants', () => {
  LAST_DEVICES = ['keyboard0'];
  // "L Ctrl" is the game's own CONTROL_LABELS spelling and passes through as-is;
  // what matters here is that two parts of ONE binding are joined with "+" and
  // are never mistaken for two device variants joined with "or".
  check('combo', scope.getBindingFriendlyName(wrapper(container(kbdPart('L Ctrl'), kbdPart('R')))), 'L Ctrl + R');
});

scenario(8, 'the radial centre readout no longer speaks a hotkey', () => {
  // Radial.vue's getHotkey() builds "<glyph> Btn_a" and the centre state carries
  // it for EVERY wedge; radialCenterText must not append it.
  const fn = source.slice(source.indexOf('function radialCenterText'), source.indexOf('function onRadialCenterState'));
  check('function was found', fn.length > 0, true);
  check('no hotkey clause', /parts\.push\(hotkey/.test(fn), false);
  check('still explains why', /state\.hotkey is deliberately NOT spoken/.test(fn), true);
});

console.log('\n' + (failures ? failures + ' FAILURE(S)' : 'All scenarios passed.'));
process.exit(failures ? 1 : 0);
