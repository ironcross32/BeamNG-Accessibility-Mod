// Replays bnvdaRuntime.js's tab/sub-tab readout against a fake DOM built to the
// shape the game's own components emit (tabList.vue, tabs.vue, bngTabs.vue,
// layoutMenu.vue, bngBreadcrumbs.vue), with known ground truth.
//
//   node diagnostic/tab_readout_sim.js
//
// The functions under test are LIFTED OUT OF THE SOURCE by name, never copied,
// for the reason binding_readout_sim.js gives: this area's failure mode is a
// readout that is merely wrong -- the wrong tab named, or a nameless one -- so a
// sim carrying its own copy would keep passing across exactly the edit that
// breaks the mod.
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

// TRANSLATION_KEY_RE is a regex literal, not a brace/bracket form, so it is
// lifted by line rather than by balancing.
function sliceLine(decl) {
  const start = source.indexOf('\n          ' + decl);
  if (start === -1) throw new Error('could not find ' + decl);
  return source.slice(start, source.indexOf('\n', start + 1));
}

const LIFT_FNS = [
  'cleanText', 'toArray', 'visibleVueElement',
  'vueProvided', 'vueTranslatedLabel', 'tabStripLabel', 'vueTabPath',
];

const lifted = [sliceLine('var TRANSLATION_KEY_RE =')]
  .concat(LIFT_FNS.map(n => sliceBalanced(n, 'fn')))
  .join('\n');

const prelude = [
  'var MAX_LEN = 160;',
  // The runtime's translator resolver, reduced to the one tier the sim drives.
  'function findTranslateFunc() { return window.__translate; }',
].join('\n');

// ---------- fake DOM ----------
// Only the surface the readers touch. visibleVueElement wants isConnected,
// getBoundingClientRect and window.getComputedStyle; the rest is class/text
// matching plus the Vue instance back-pointer.
class El {
  constructor(tag, classes, text, attrs) {
    this.tagName = tag.toUpperCase();
    this.classList = new Set(classes || []);
    this.ownText = text || '';
    this.attrs = attrs || {};
    this.children = [];
    this.parentElement = null;
    this.isConnected = true;
    this.hidden = false;
    this.__vueParentComponent = null;
  }
  add(child) { child.parentElement = this; this.children.push(child); return this; }
  get className() { return Array.from(this.classList).join(' '); }
  get textContent() { return this.ownText + this.children.map(c => c.textContent).join(''); }
  get innerText() { return this.textContent; }
  get title() { return this.attrs.title || ''; }
  getAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attrs, name) ? this.attrs[name] : null; }
  getBoundingClientRect() { return this.hidden ? { width: 0, height: 0 } : { width: 100, height: 20 }; }
  descendants(out) { out = out || []; for (const c of this.children) { out.push(c); c.descendants(out); } return out; }
  // Compound selectors matter here: ".tab-item.tab-active-tab" must not match a
  // button that merely carries .tab-item, or every strip reports its first tab.
  matchesSimple(sel) {
    sel = sel.trim();
    if (!sel) return false;
    const attr = sel.match(/^\[([\w-]+)="([^"]*)"\]$/);
    if (attr) return this.getAttribute(attr[1]) === attr[2];
    const parts = sel.split(/(?=[.\[])/);
    for (const part of parts) {
      if (!part) continue;
      if (part.charAt(0) === '.') { if (!this.classList.has(part.slice(1))) return false; continue; }
      if (part.charAt(0) === '[') {
        const a = part.match(/^\[([\w-]+)="([^"]*)"\]$/);
        if (!a || this.getAttribute(a[1]) !== a[2]) return false;
        continue;
      }
      if (this.tagName !== part.toUpperCase()) return false;
    }
    return true;
  }
  // Descendant combinators ("a b") are resolved right-to-left, which is all the
  // breadcrumb selector needs.
  matchesCompound(sel) {
    const steps = sel.trim().split(/\s+/);
    if (!this.matchesSimple(steps[steps.length - 1])) return false;
    let node = this.parentElement;
    for (let i = steps.length - 2; i >= 0; i--) {
      while (node && !node.matchesSimple(steps[i])) node = node.parentElement;
      if (!node) return false;
      node = node.parentElement;
    }
    return true;
  }
  matches(sel) { return sel.split(',').some(part => this.matchesCompound(part)); }
  querySelectorAll(sel) {
    const out = [];
    for (const rawPart of sel.split(',')) {
      const part = rawPart.trim();
      if (!part) continue;
      for (const node of this.descendants()) {
        if (node.matchesCompound(part) && out.indexOf(node) === -1) out.push(node);
      }
    }
    return out;
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
}

const el = (tag, classes, text, attrs) => new El(tag, classes, text, attrs);

// ---------- component shapes ----------
// tabs.vue: provide("tabs", ref([{index, heading, icon, tooltip, active}])).
// tabList.vue renders the heading <span> only when the strip is NOT icon-only.
function tabStrip(entries, opts) {
  opts = opts || {};
  const list = el('div', ['tab-list']);
  const model = entries.map((e, index) => ({
    index,
    heading: e.heading,
    icon: e.icon || null,
    tooltip: e.tooltip || null,
    active: !!e.active,
  }));
  entries.forEach((e, i) => {
    const classes = ['tab-item'];
    if (e.active) { classes.push('tab-active-tab'); classes.push('no-hover'); }
    const button = el('button', classes);
    // A bngIcons glyph: Private Use Area, which cleanText strips.
    if (e.icon) button.add(el('span', ['icon-base'], String.fromCharCode(0xE100 + i)));
    if (!opts.iconOnly) button.add(el('span', [], e.heading));
    list.add(button);
  });
  // Vue's own instance back-pointer, carrying the provides chain. TabList is a
  // child of Tabs, so its provides object inherits the "tabs" key.
  if (!opts.noVueInstance) {
    const tabsInstance = { provides: Object.create(null), parent: null };
    tabsInstance.provides.tabs = { value: model };
    list.__vueParentComponent = { provides: Object.create(tabsInstance.provides), parent: tabsInstance };
  }
  return { list, model };
}

function breadcrumbs(labels) {
  const path = el('div', ['bng-path']);
  labels.forEach((label, i) => {
    const classes = ['bng-path-item'];
    if (i === labels.length - 1) classes.push('bng-path-last');
    if (i === 0) classes.push('bng-path-first');
    path.add(el('button', classes, label));
  });
  return path;
}

function screen(...nodes) {
  const root = el('div', ['vue-app']);
  nodes.forEach(n => root.add(n));
  return root;
}

// ---------- environment ----------
const TRANSLATIONS = {
  'ui.pause.vehicle': 'Vehicle',
  'ui.modtab.telemetry': 'Telemetry',
};
const fakeWindow = {
  __translate: key => (Object.prototype.hasOwnProperty.call(TRANSLATIONS, key) ? TRANSLATIONS[key] : key),
  getComputedStyle: () => ({ display: 'block', visibility: 'visible' }),
};

const scope = new Function(
  'window',
  prelude + '\n' + lifted +
  '\nreturn { vueTabPath: vueTabPath, tabStripLabel: tabStripLabel, visibleVueElement: visibleVueElement };'
)(fakeWindow);

// ---------- the previous form ----------
// What pollVueFocus fed into screenKey before: an ARIA/class shape tabList.vue
// has never emitted.
const OLD_TAB_SELECTOR = '[role="tab"][aria-selected="true"], .bng-tab.active, .bng-tab.selected';
function oldTabText(root) {
  const activeTab = root.querySelector(OLD_TAB_SELECTOR);
  return activeTab ? activeTab.innerText.trim() : '';
}
// The DOM-text tier alone, i.e. what a reader with no access to the Vue
// provides would manage.
function domOnlyLabel(listEl) {
  const active = listEl.querySelector('.tab-item.tab-active-tab');
  if (!active) return '';
  return active.textContent.replace(/[-]/g, '').replace(/\s+/g, ' ').trim();
}

// ---------- scenarios ----------
let failures = 0;
function check(name, actual, expected) {
  const ok = actual === expected;
  if (!ok) failures++;
  console.log((ok ? '  ok   ' : '  FAIL ') + name +
    (ok ? '' : '\n         expected ' + JSON.stringify(expected) + '\n         actual   ' + JSON.stringify(actual)));
}
function scenario(n, title) { console.log('\n[' + n + '] ' + title); }

// 1 -- the top-level pause tabs, which DO render their heading as text.
scenario(1, 'labelled pause tab strip names the active tab');
{
  const strip = tabStrip([
    { heading: 'System' },
    { heading: 'Activities' },
    { heading: 'Vehicle', active: true },
    { heading: 'Environment' },
  ]);
  const root = screen(strip.list);
  check('active tab is named', scope.vueTabPath(root), 'Vehicle');
  check('previous form found nothing at all', oldTabText(root), '');
}

// 2 -- the pause vehicle sub-tabs. bngTabs passes icon-only, so tabList.vue
// skips the heading span and the button holds a glyph and nothing else.
scenario(2, 'icon-only sub-tab strip is named from the Vue tabs list');
{
  const strip = tabStrip([
    { heading: 'Parts', icon: 'engine', tooltip: 'Parts', active: true },
    { heading: 'Tuning', icon: 'powerGauge03', tooltip: 'Tuning' },
    { heading: 'Paint', icon: 'sprayCan', tooltip: 'Paint' },
  ], { iconOnly: true });
  const root = screen(strip.list);
  check('sub-tab is named', scope.vueTabPath(root), 'Parts');
  check('DOM text alone yields nothing', domOnlyLabel(strip.list), '');
  check('previous form found nothing at all', oldTabText(root), '');
}

// 3 -- both strips on screen at once, which is the pause vehicle config screen.
scenario(3, 'nested strips read outermost first');
{
  const outer = tabStrip([{ heading: 'System' }, { heading: 'Vehicle', active: true }]);
  const inner = tabStrip([
    { heading: 'Parts', icon: 'engine' },
    { heading: 'Tuning', icon: 'powerGauge03', active: true },
  ], { iconOnly: true });
  const card = el('div', ['tab-content']);
  card.add(inner.list);
  outer.list.parentElement = null;
  const root = screen(outer.list, card);
  check('tab and sub-tab', scope.vueTabPath(root), 'Vehicle, Tuning');
}

// 4 -- a strip belonging to a tab that is not on screen must not contribute.
scenario(4, 'hidden strips are excluded');
{
  const shown = tabStrip([{ heading: 'Vehicle', active: true }]);
  const hidden = tabStrip([{ heading: 'Paint', active: true }], { iconOnly: true });
  hidden.list.hidden = true;
  const root = screen(shown.list, hidden.list);
  check('only the visible strip', scope.vueTabPath(root), 'Vehicle');
}

// 5 -- layoutMenu shows tabs and breadcrumbs as alternatives, so a screen one
// level deep has a trail and no strip at all.
scenario(5, 'breadcrumb-only screen still says where you are');
{
  const root = screen(breadcrumbs(['Vehicle', 'Save & Load']));
  check('last crumb', scope.vueTabPath(root), 'Save & Load');
}

// 6 -- a mod-contributed tab whose label never went through _tr.
scenario(6, 'a bare translation key is translated, not spoken');
{
  const strip = tabStrip([
    { heading: 'System' },
    { heading: 'ui.modtab.telemetry', active: true },
  ]);
  const root = screen(strip.list);
  check('translated', scope.vueTabPath(root), 'Telemetry');
}
{
  // ...and an ordinary label containing no dot is left exactly as it is.
  const strip = tabStrip([{ heading: 'Save & Load', active: true }]);
  check('plain label untouched', scope.vueTabPath(screen(strip.list)), 'Save & Load');
}

// 7 -- the Vue tier is an internal and may one day stop being reachable. The
// DOM tier has to keep answering for every strip that renders text.
scenario(7, 'DOM tier answers when the Vue instance is unreachable');
{
  const strip = tabStrip([
    { heading: 'System' },
    { heading: 'Vehicle', active: true },
  ], { noVueInstance: true });
  const root = screen(strip.list);
  check('falls back to the active button text', scope.vueTabPath(root), 'Vehicle');
}
{
  // The honest limit of that fallback: an icon-only strip has no text to read,
  // and reporting a wrong name would be worse than reporting none.
  const strip = tabStrip([{ heading: 'Parts', icon: 'engine', active: true }],
    { iconOnly: true, noVueInstance: true });
  check('icon-only without Vue yields nothing rather than a guess',
    scope.vueTabPath(screen(strip.list)), '');
}

// 8 -- a strip with no active tab must contribute nothing rather than the first.
scenario(8, 'a strip with no active tab is silent');
{
  const strip = tabStrip([{ heading: 'System' }, { heading: 'Vehicle' }]);
  check('no active tab', scope.vueTabPath(screen(strip.list)), '');
}

// 9 -- greps. The wiring these functions depend on lives in the poll loop and
// cannot be exercised through a fake DOM, so it is asserted textually.
scenario(9, 'poll wiring');
{
  const poll = source.slice(source.indexOf('function pollVueFocus('));
  const pollBody = poll.slice(0, poll.indexOf('\n            scheduleVuePoll(0);'));
  check('the dead ARIA tab selector is gone from the poll',
    /\[role="tab"\]\[aria-selected="true"\]/.test(pollBody), false);
  check('screenKey carries the resolved tab path',
    /var screenKey =[\s\S]{0,200}?tabPath/.test(pollBody), true);
  check('a tab change is announced', /announceVueTab\(tabPath\)/.test(pollBody), true);
  // The whole point of not resetting: a held bumper sweeps tabs the way a held
  // direction sweeps a list, and most pause tabs push a route, so an
  // unconditional navBurstReset here would make every tab in the sweep speak.
  check('a tab change does not reset the nav burst',
    /if \(!tabChanged\) navBurstReset\(\);/.test(pollBody), true);
  // The tab announcement and the focus announcement must resolve the SAME
  // element, or the combined utterance describes something else.
  check('the poll uses the shared focus resolver',
    /var focused = focusedVueElement\(root, bindingPopup, activatedDropdownFocused\);/.test(pollBody), true);
  check('leaving the screen clears tab tracking',
    /resetVueTabTracking\(\);/.test(pollBody), true);
  // Entry is not a tab change. Without the _vueTabSeen gate every arrival on a
  // Vue screen announces twice -- worst on the options screen, where the
  // category announcement has just spoken.
  check('first sight of a screen is not announced',
    /var tabChanged = _vueTabSeen && tabPath !== _vueTabPath;/.test(pollBody), true);
  // The binding editor carries a tab strip of its own; blanking rather than
  // freezing would make dismissing it read as a move back onto the pause tab.
  check('tab tracking is frozen, not blanked, under the binding popup',
    /var tabPath = bindingPopup \? _vueTabPath : vueTabPath\(root\);/.test(pollBody), true);
  check('the reset re-arms the first-sight gate',
    /_vueTabSeen = false;/.test(source), true);
}

// 10 -- the queue window. The tab name is spoken alone and the landing focus
// announcement has to arrive BEHIND it, not on top of it.
scenario(10, 'a tab name is not cut off by the focus that lands after it');
{
  let CLOCK = 1000;
  const sent = [];
  const speakScope = new Function(
    'send', 'nowTS', 'isDebug', 'log', 'state',
    'var P = { POINTER: 1, KEYBOARD: 2, CONTROLLER: 3, SYSTEM: 4 };' +
    'var lastSpoken = "", lastSource = 0, lastSpeakTs = 0;' +
    'var VUE_TAB_QUEUE_MS = ' + (source.match(/var VUE_TAB_QUEUE_MS = (\d+);/) || [])[1] + ';' +
    'var VUE_CONFLICT_QUEUE_MS = ' + (source.match(/var VUE_CONFLICT_QUEUE_MS = (\d+);/) || [])[1] + ';' +
    'var _speakQueueAfterText = "", _speakQueueAfterMs = 0, _speakQueueAfterMax = 0;' +
    'var _speakQueueUntil = 0, _speakQueueLeft = 0;' +
    '\n' + sliceBalanced('navBurstIsNavSource', 'fn') +
    '\n' + sliceBalanced('armSpeakQueue', 'fn') +
    '\n' + sliceBalanced('clearSpeakQueue', 'fn') +
    '\n' + sliceBalanced('emitSpeak', 'fn') +
    '\nreturn { emitSpeak: emitSpeak, arm: armSpeakQueue, clear: clearSpeakQueue,' +
    ' TAB_MS: VUE_TAB_QUEUE_MS, CONFLICT_MS: VUE_CONFLICT_QUEUE_MS,' +
    ' P: P, queued: function () { return _speakQueueUntil; } };'
  )(o => sent.push(o), () => CLOCK, () => false, () => {}, {});

  // A tab change: the name is armed, then goes out through the normal timer.
  speakScope.arm('Vehicle', speakScope.TAB_MS, 1);
  speakScope.emitSpeak('Vehicle', speakScope.P.CONTROLLER);
  check('the tab name itself interrupts', sent[0].interrupt, true);

  // The engine lands focus a quarter second later and the focus watcher speaks.
  CLOCK += 250;
  speakScope.emitSpeak('Parts, Engine', speakScope.P.CONTROLLER);
  check('the landing item queues behind it', sent[1].interrupt, false);

  // Anything after that is a fresh movement and must interrupt normally, or a
  // held sweep would stack up behind a tab name nobody is waiting on.
  CLOCK += 100;
  speakScope.emitSpeak('Tuning, Springs', speakScope.P.CONTROLLER);
  check('only ONE utterance queues', sent[2].interrupt, true);

  // Past the window, the focus landing interrupts like anything else.
  speakScope.arm('Environment', speakScope.TAB_MS, 1);
  speakScope.emitSpeak('Environment', speakScope.P.CONTROLLER);
  CLOCK += 5000;
  speakScope.emitSpeak('Time of day', speakScope.P.CONTROLLER);
  check('the window expires', sent[4].interrupt, true);

  // A system notification is not the thing the window exists to protect, and
  // must not be delayed behind a menu announcement.
  speakScope.arm('Vehicle', speakScope.TAB_MS, 1);
  speakScope.emitSpeak('Vehicle', speakScope.P.CONTROLLER);
  speakScope.emitSpeak('Engine damaged', speakScope.P.SYSTEM);
  check('a system message still interrupts', sent[6].interrupt, true);
}

console.log('\n' + (failures ? failures + ' FAILURE(S)' : 'all scenarios passed'));
process.exit(failures ? 1 : 0);
