// Simulation of the radial menu's speech from bnvdaRuntime.js: the centre-label
// hook, the pointer tracking that catches a release passing over a neighbouring
// wedge, and the level (submenu / category) announcement. Replayed against a
// fake clock and a model of Radial.vue's pointToItem, so it runs without the
// game.
//
// As in navburst_sim.js, the functions are LIFTED out of the source by name
// rather than copied, so the sim cannot drift from what ships.
//
//   node diagnostic/radial_sim.js

const fs = require('fs');
const path = require('path');

const SRC = path.join(__dirname, '..', 'bng_mod', 'ui', 'ui-vue', 'mods', 'bnvda', 'bnvdaRuntime.js');
const source = fs.readFileSync(SRC, 'utf8').replace(/\r\n/g, '\n');

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
    if (c === '/' && source[i - 1] === '(') {
      // A regex literal argument, e.g. split(/\s+\/\s+/): skip to its end.
      for (i++; i < source.length && source[i] !== '/'; i++) if (source[i] === '\\') i++;
      continue;
    }
    if (c === open) depth++;
    else if (c === close) { depth--; if (depth === 0) return source.slice(start, i + 1); }
  }
  throw new Error('unbalanced ' + name);
}

function sliceLine(name) {
  const start = source.indexOf('\n          var ' + name + ' = ');
  if (start === -1) throw new Error('could not find var ' + name);
  return source.slice(start, source.indexOf('\n', start + 1));
}

const LINES = [
  '_radialMenuWasOpen', '_radialAnnouncedOpen', '_radialFocusedText', '_radialLastSpokenItem',
  '_radialLastSegments', '_radialLastSig', '_radialPendingSig', '_radialSettleQueued',
  '_radialSawDefault', '_radialReleaseTimer', '_radialSvg', '_radialUnwrapEvents', '_radialStick', '_radialDpad',
  '_radialInputFalling', 'RADIAL_RELEASE_SETTLE_MS', 'RADIAL_FALL_EPSILON',
  '_vueHintTimer', '_vueHintGeneration',
];
const FNS = [
  'radialCenterText', 'radialNoteInput', 'radialDpadMagnitude', 'radialCancelRelease',
  'radialSpeak', 'onRadialCenterState', 'radialSettle', 'radialSegments',
  'radialLevelAnnouncement', 'radialMenuCheckLevel', 'radialMenuOnClose', 'navigationValuePressed',
  'vueHintNoteActivity', 'radialItemLabel', 'radialItemState', 'radialWrapEvents',
  'installRadialCenterHook',
];

const lifted = LINES.map(sliceLine).join('') + '\n' + FNS.map(n => sliceBalanced(n, 'fn')).join('\n');

// ---------- harness ----------
const flush = () => new Promise(r => setImmediate(r));

function makeSim() {
  let now = 0, seq = 0, timers = [];
  const spoken = [];
  function setT(fn, d) { const id = ++seq; timers.push({ id, at: now + d, fn }); return id; }
  function clrT(id) { timers = timers.filter(t => t.id !== id); }
  async function advance(ms) {
    await flush();
    const end = now + ms;
    for (;;) {
      timers.sort((a, b) => a.at - b.at || a.id - b.id);
      if (!timers.length || timers[0].at > end) break;
      const t = timers.shift();
      now = t.at;
      t.fn();
      await flush();
    }
    now = end;
  }

  // The DOM the module reads: open flag, title, breadcrumb trail, selected
  // category, and whether any wedge is drawn.
  const dom = { open: true, title: 'Radial Menu', crumbs: '', category: '', wedges: true };
  const el = text => ({ textContent: text });
  const document = {
    querySelector(sel) {
      if (!dom.open) return null;
      if (sel === '.radial-menu') return {};
      if (/radial-title$/.test(sel)) return el(dom.title);
      if (/radial-breadcrumbs$/.test(sel)) return el(dom.crumbs);
      if (/radial-category-label$/.test(sel)) return dom.category ? el(dom.category) : null;
      if (/svg > g path$/.test(sel)) return dom.wedges ? {} : null;
      return null;
    },
  };
  const cleanText = s => String(s == null ? '' : s).replace(/[-]/g, '').replace(/\s+/g, ' ').trim();
  const navBurst = { timer: null, active: true, text: 'stale row', src: 3, fresh: 2, ema: 200 };

  // A model of 0.40's RadialSVG, as far as the hook touches it: update() lays
  // the wedges out (re-focusing one already marked focused), and each wedge's
  // focus/blur call the instance's centre callbacks only on an actual change.
  class RadialSVG {
    constructor(events) { this.events = events; this.buttons = null; }
    update(items) {
      const events = this.events;
      this.buttons = items.map((item, index) => {
        const btn = { item, _focused: false };
        btn.focus = () => { if (item.focused && btn._focused) return; item.focused = btn._focused = true; events.centerFocus(item, index); };
        btn.blur = () => { if (!item.focused && !btn._focused) return; item.focused = btn._focused = false; events.centerBlur(item, index); };
        if (item.focused) btn.focus();
        return btn;
      });
    }
  }
  const cleanups = [];
  // $ctx_t: a { txt, context } descriptor, keys looked up in a tiny table.
  const ctxT = (d, translateContext) => {
    const table = { 'ui.radialmenu2.exitVehicle': 'Exit vehicle' };
    const txt = d && typeof d === 'object' ? d.txt : d;
    return (table[txt] || txt) + (translateContext ? ' [ctx translated]' : '');
  };

  const api = new Function(
    'document', 'trackedSetTimeout', 'clearTimeout', 'nowTS', 'emitSpeak', 'log', 'cleanText', '_navBurst',
    'RadialSVG', 'onCleanup', 'gameContextTranslate', 'findTranslateFunc',
    'var P = { POINTER: 1, KEYBOARD: 2, CONTROLLER: 3, SYSTEM: 4 };' +
    'var speakTimer = null, loadingActive = false, lastControllerTs = 0;' +
    lifted +
    '\nreturn { note: radialNoteInput, center: onRadialCenterState, poll: radialMenuCheckLevel,' +
    ' close: radialMenuOnClose, settleMs: RADIAL_RELEASE_SETTLE_MS, install: installRadialCenterHook,' +
    // Stands in for updateVuePauseHints arming the idle hint.
    ' armHint: function (ms) { _vueHintTimer = trackedSetTimeout(function () { _vueHintTimer = null; emitSpeak("HINT", 3); }, ms); },' +
    ' hintPending: function () { return !!_vueHintTimer; } };'
  )(document, setT, clrT, () => now, txt => spoken.push({ t: now, txt }), () => {}, cleanText, navBurst,
    RadialSVG, fn => cleanups.push(fn), () => ctxT, () => k => k);

  // Model of Radial.vue: wedges laid out clockwise from straight up, and
  // radialsvg's focus/blur, which only call back on an actual change.
  let canvas = {};
  let items = [];
  let focusedIdx = -1;
  let stick = { x: 0, y: 0 }, stickActive = false;
  const defaultState = { label: 'Select an option', focused: false };
  function setItems(names) { items = names; }
  function itemAt(x, y) {
    if (x === 0 && y === 0) return -1;
    const cw = (Math.atan2(x, y) / (2 * Math.PI) + 1) % 1; // 0 = up, clockwise
    return Math.floor((cw + 0.5 / items.length) % 1 * items.length);
  }
  function pointToItem(x, y) {
    const idx = itemAt(x, y);
    if (focusedIdx !== -1 && focusedIdx !== idx) { focusedIdx = -1; api.center(canvas, defaultState); }
    if (idx !== -1 && focusedIdx !== idx) { focusedIdx = idx; api.center(canvas, { label: items[idx], focused: true }); }
  }
  // One axis event: the runtime's ui_nav capture sees it, then Radial.vue does.
  function axis(name, v) {
    api.note(name, v);
    stick[name === 'focus_lr' ? 'x' : 'y'] = v;
    const before = stickActive;
    stickActive = Math.hypot(stick.x, stick.y) > 0.5;
    if (stickActive) pointToItem(stick.x, stick.y);
    if (!stickActive && before) pointToItem(0, 0);
  }
  // requestData(): new data, default centre, wedges redrawn, refocus.
  function rebuild() {
    api.center(canvas, defaultState);
    if (focusedIdx !== -1) api.center(canvas, { label: items[focusedIdx], focused: true });
  }
  function remount() { canvas = {}; focusedIdx = -1; }

  return {
    api, dom, spoken, navBurst, advance, axis, rebuild, setItems, remount, RadialSVG,
    uninstall: () => { while (cleanups.length) cleanups.pop()(); },
    now: () => now,
    texts: () => spoken.map(x => x.txt),
    clear: () => { spoken.length = 0; },
  };
}

let failures = 0;
function check(label, cond, detail) {
  console.log((cond ? '  PASS  ' : '  FAIL  ') + label + (cond || !detail ? '' : '\n        ' + detail));
  if (!cond) failures++;
}

const TOP = ['Boost', 'Exit vehicle', 'Camera', 'Vehicle Actions', 'Freeroam Actions', 'Favorites', 'AI', 'Fun Stuff'];

// Open the top level the way the game does: mount with no data, then data.
async function openTop(s) {
  s.setItems(TOP);
  s.dom.wedges = false;
  s.dom.title = 'No Actions Available';
  s.rebuild();
  await s.advance(5);
  s.dom.wedges = true;
  s.dom.title = 'Radial Menu';
  s.rebuild();
  await s.advance(5);
}

(async () => {
  {
    console.log('Scenario 1: opening the menu');
    const s = makeSim();
    await openTop(s);
    check('announced once, from the data, not the placeholder', JSON.stringify(s.texts()) === '["Radial Menu"]', JSON.stringify(s.texts()));
    s.api.poll(); s.api.poll();
    check('the poll does not repeat it', s.texts().length === 1);
  }

  {
    console.log('\nScenario 2: slow rotation speaks every wedge as it is reached');
    const s = makeSim();
    await openTop(s); s.clear();
    const at = [];
    for (let i = 0; i <= 16; i++) {
      const a = i / 16 * 2 * Math.PI;
      s.axis('focus_lr', +Math.sin(a).toFixed(3));
      s.axis('focus_ud', +Math.cos(a).toFixed(3));
      await s.advance(0);
      at.push(s.now());
      await s.advance(180);
    }
    const t = s.texts();
    check('all eight wedges spoken', TOP.every(n => t.includes(n)), JSON.stringify(t));
    check('none spoken late', s.spoken.every(x => at.includes(x.t)), JSON.stringify(s.spoken));
    check('a pending burst is dropped, not flushed later', s.navBurst.text === '' && !s.navBurst.active);
  }

  {
    console.log('\nScenario 3: press, release, press the same item again quickly');
    const s = makeSim();
    await openTop(s); s.clear();
    s.axis('focus_ud', 1); await s.advance(250);
    s.axis('focus_ud', 0); await s.advance(150);
    s.axis('focus_ud', 1); await s.advance(200);
    check('spoken both times', JSON.stringify(s.texts()) === '["Boost","Boost"]', JSON.stringify(s.texts()));
  }

  {
    console.log('\nScenario 4: releasing a diagonal one axis at a time');
    const s = makeSim();
    await openTop(s); s.clear();
    s.axis('focus_lr', 0.7); s.axis('focus_ud', 0.7); await s.advance(400);
    s.axis('focus_lr', 0); await s.advance(20);
    s.axis('focus_ud', 0); await s.advance(300);
    check('the neighbour the release crossed is not spoken', JSON.stringify(s.texts()) === '["Exit vehicle"]', JSON.stringify(s.texts()));
  }

  {
    console.log('\nScenario 5: deliberately easing off one axis onto the neighbour');
    const s = makeSim();
    await openTop(s); s.clear();
    s.axis('focus_lr', 0.7); s.axis('focus_ud', 0.7); await s.advance(400);
    const eased = s.now();
    s.axis('focus_lr', 0); await s.advance(300);
    check('the neighbour is spoken', JSON.stringify(s.texts()) === '["Exit vehicle","Boost"]', JSON.stringify(s.texts()));
    check('after only the release settle', s.spoken[1].t - eased === s.api.settleMs, String(s.spoken[1].t - eased));
  }

  {
    console.log('\nScenario 6: D-pad diagonal released one button at a time');
    const s = makeSim();
    await openTop(s); s.clear();
    // The D-pad drives the same pointer; feed it as the axes Radial.vue builds.
    s.api.note('focus_u', 1); s.api.note('focus_r', 1);
    s.axis('focus_lr', 1); s.axis('focus_ud', 1); await s.advance(300);
    s.api.note('focus_r', 0); s.axis('focus_lr', 0); await s.advance(25);
    s.api.note('focus_u', 0); s.axis('focus_ud', 0); await s.advance(300);
    check('only the diagonal item', JSON.stringify(s.texts()) === '["Exit vehicle"]', JSON.stringify(s.texts()));
  }

  {
    console.log('\nScenario 7: menu rebuilt under the pointer');
    const s = makeSim();
    await openTop(s);
    s.axis('focus_ud', 1); await s.advance(100); s.clear();
    s.rebuild(); await s.advance(100);
    check('the same item is not repeated', s.texts().length === 0, JSON.stringify(s.texts()));
  }

  {
    console.log('\nScenario 8: submenu and category path');
    const s = makeSim();
    await openTop(s); s.clear();
    s.setItems(['Forces', 'Destruction', 'Boom!', 'Break Everything', 'Other']);
    s.dom.title = 'Freeroam Actions'; s.dom.crumbs = 'Freeroam Actions / Fun Stuff'; s.dom.category = 'Fun Stuff';
    s.rebuild(); await s.advance(5);
    check('entering names the new part once', JSON.stringify(s.texts()) === '["Freeroam Actions, Fun Stuff"]', JSON.stringify(s.texts()));
    s.clear();
    s.dom.crumbs = 'Freeroam Actions / Vehicle'; s.dom.category = 'Vehicle';
    s.rebuild(); await s.advance(5);
    check('switching category names only the category', JSON.stringify(s.texts()) === '["Vehicle"]', JSON.stringify(s.texts()));
    s.clear();
    s.axis('focus_ud', 1); await s.advance(5); s.clear();
    s.dom.crumbs = 'Freeroam Actions / Fun Stuff'; s.dom.category = 'Fun Stuff';
    s.rebuild(); await s.advance(5);
    check('a level change carries the item under the pointer', JSON.stringify(s.texts()) === '["Fun Stuff, Forces"]', JSON.stringify(s.texts()));
    s.clear(); s.axis('focus_ud', 0); await s.advance(5);
    s.setItems(TOP);
    s.dom.title = 'Radial Menu'; s.dom.crumbs = ''; s.dom.category = '';
    s.rebuild(); await s.advance(5);
    check('going back up names where it landed', JSON.stringify(s.texts()) === '["Radial Menu"]', JSON.stringify(s.texts()));
  }

  {
    console.log('\nScenario 9: reopened between polls');
    const s = makeSim();
    await openTop(s); s.clear();
    s.remount();
    await openTop(s);
    check('the new menu is announced again', JSON.stringify(s.texts()) === '["Radial Menu"]', JSON.stringify(s.texts()));
  }

  {
    console.log('\nScenario 10: a level with no items');
    const s = makeSim();
    s.setItems([]);
    s.dom.wedges = false; s.dom.title = 'No Actions Available';
    s.rebuild(); await s.advance(5);
    check('not read from the event', s.texts().length === 0);
    s.api.poll(); s.api.poll();
    check('the poll reads it once it holds', JSON.stringify(s.texts()) === '["Radial menu, No Actions Available"]', JSON.stringify(s.texts()));
  }

  {
    console.log('\nScenario 11: moving the stick holds back the idle hint');
    const s = makeSim();
    await openTop(s); s.clear();
    s.api.armHint(3000);
    for (let i = 0; i < 6; i++) {
      s.axis('focus_ud', i % 2 ? -1 : 1); await s.advance(450);
      s.axis('focus_ud', 0); await s.advance(200);
    }
    check('the pending hint is cancelled by the movement', !s.api.hintPending() && !s.texts().includes('HINT'), JSON.stringify(s.texts()));
  }

  {
    console.log('\nScenario 12: through the real hook on a model of 0.40\'s RadialSVG');
    const s = makeSim();
    const plainUpdate = s.RadialSVG.prototype.update;
    s.api.install();
    let origFocus = 0, origBlur = 0;
    const ev = { centerFocus() { origFocus++; }, centerBlur() { origBlur++; } };
    const plainFocus = ev.centerFocus;
    const svg = new s.RadialSVG(ev);
    const items = TOP.map(t => ({ title: t }));
    items[1].title = 'ui.radialmenu2.exitVehicle';
    items[2].price = { money: { amount: 250 } };
    svg.update(items); await s.advance(5);
    check('opening is announced from update()', JSON.stringify(s.texts()) === '["Radial Menu"]', JSON.stringify(s.texts()));
    s.clear();
    svg.buttons[1].focus(); await s.advance(5);
    check('a focused wedge is spoken, translated, context left alone', JSON.stringify(s.texts()) === '["Exit vehicle"]', JSON.stringify(s.texts()));
    s.clear();
    svg.buttons[1].blur(); svg.buttons[2].focus(); await s.advance(5);
    check('the next wedge carries its price', JSON.stringify(s.texts()) === '["Camera, costs 250"]', JSON.stringify(s.texts()));
    check('the game\'s own callbacks still run', origFocus === 2 && origBlur === 1, origFocus + '/' + origBlur);
    s.clear();
    svg.update(items); await s.advance(5);
    check('a rebuild under the pointer stays quiet', s.texts().length === 0, JSON.stringify(s.texts()));
    s.uninstall();
    check('uninstall restores update() and the callbacks',
          s.RadialSVG.prototype.update === plainUpdate && ev.centerFocus === plainFocus && !ev.__bnvdaWrapped);
  }

  {
    console.log('\nScenario 13: wiring');
    const code = source.split('\n').filter(l => !/^\s*\/\//.test(l)).join('\n');
    check('direction input reaches the pointer tracking',
          /function trackNavigationInput\(action, value\) \{\s*radialNoteInput\(action, value\);/.test(code));
    check('the update hook passes the RadialSVG instance',
          /onRadialCenterState\(this, null\);/.test(code) && /onRadialCenterState\(svg, radialItemState\(item\)\);/.test(code));
    const index = fs.readFileSync(path.join(path.dirname(SRC), 'index.js'), 'utf8');
    check('index.js imports radialsvg and hands it over as radialSvg',
          /import\('@\/modules\/radial\/radialsvg'\)/.test(index) && /^\s*radialSvg,\s*$/m.test(index));
    check('item names bypass scheduleSpeak',
          !/function radialSettle[\s\S]*?scheduleSpeak[\s\S]*?function installRadialCenterHook/.test(code));
  }

  console.log(failures === 0 ? '\nAll scenarios passed.' : `\n${failures} check(s) FAILED.`);
  process.exit(failures === 0 ? 0 : 1);
})();
