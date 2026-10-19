// Simulation of the held-navigation coalescing gate from bnvdaRuntime.js,
// replayed against a fake clock so the scenarios can be checked without the
// game. (The tuning apply guard's scenarios are parked with it in
// parked_tuning_hold_workaround.js.)
//
// The functions are LIFTED out of the source by name rather than copied. This
// file used to carry its own copy of the state machine, and that is how it kept
// passing while the hold tracking it modelled was dead in the game: the sim
// called press('down'), and the real matcher never recognised the engine's
// actual event name, focus_d. A copy can only agree with itself.
//
//   node diagnostic/navburst_sim.js

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

// Scalars and regex literals are lifted by line.
function sliceLine(name) {
  const start = source.indexOf('\n          var ' + name + ' = ');
  if (start === -1) throw new Error('could not find var ' + name);
  return source.slice(start, source.indexOf('\n', start + 1));
}

const LINES = [
  'DEBOUNCE_MS', 'CONTROLLER_DOMINANCE_MS',
  'NAV_BURST_GAP_MS', 'NAV_SETTLE_MIN_MS', 'NAV_SETTLE_MAX_MS', 'NAV_SETTLE_FACTOR',
  'NAV_RELEASE_DRAIN_MS', 'NAV_HOLD_STALE_MS', 'NAV_PRESS_FRESH_MS',
  'NAV_ACTION_RE', 'NAV_AXIS_RE', 'NAV_AXIS_PRESS', 'NAV_AXIS_RELEASE', 'NAV_EDGE_ECHO_MS',
  'NAV_KICK_ACTIONS', '_navLastPressTs',
  'VUE_TAB_QUEUE_MS', 'VUE_CONFLICT_QUEUE_MS',
];
const FNS = [
  'navBurstReset', 'navBurstHeldCount', 'emitSpeak', 'navBurstFlush', 'navSettleDelay',
  'navBurstArm', 'navBurstOnSettle', 'navBurstIsNavSource', 'navBurstNoteRedundant',
  'navBurstCapture', 'scheduleSpeak',
  'navActionName', 'navigationValuePressed', 'trackNavigationInput',
];

const lifted =
  LINES.map(sliceLine).join('') +
  sliceBalanced('_navBurst', 'var') + ';' +
  FNS.map(n => sliceBalanced(n, 'fn')).join('\n');

function makeSim() {
  let now = 0, seq = 0, timers = [];
  const spoken = [];
  const kicks = { n: 0 };
  const cleanups = [];

  function setT(fn, d) { const id = ++seq; timers.push({ id, at: now + d, fn }); return id; }
  function clrT(id) { timers = timers.filter(t => t.id !== id); }
  function advance(ms) {
    const end = now + ms;
    for (;;) {
      timers.sort((a, b) => a.at - b.at || a.id - b.id);
      if (!timers.length || timers[0].at > end) break;
      const t = timers.shift();
      now = t.at;
      t.fn();
    }
    now = end;
  }

  const fakeWindow = { bngVue: null };
  const api = new Function(
    'window', 'trackedSetTimeout', 'clearTimeout', 'nowTS', 'send', 'log', 'isDebug',
    'onCleanup', 'kickVuePoll', 'looksLikeCss', 'hasNonAscii', 'charDump',
    'var P = { POINTER: 1, KEYBOARD: 2, CONTROLLER: 3, SYSTEM: 4 };' +
    'var lastSpoken = "", lastSource = 0, lastSpeakTs = -1e9, lastControllerTs = -1e9;' +
    'var speakTimer = null, loadingActive = false, navHoldSuppression = true;' +
    'var _speakQueueAfterText = "", _speakQueueAfterMs = 0, _speakQueueAfterMax = 0;' +
    'var _speakQueueUntil = 0, _speakQueueLeft = 0;' +
    // The radial menu's pointer tracking rides on trackNavigationInput; it is
    // covered by radial_sim.js.
    'function radialNoteInput() {}' +
    'var _vueHintTimer = null, _vueHintGeneration = 0;' +
    sliceBalanced('vueHintNoteActivity', 'fn') +
    lifted +
    '\nreturn { P: P, scheduleSpeak: scheduleSpeak, input: trackNavigationInput,' +
    ' navBurstReset: navBurstReset, navActionName: navActionName,' +
    ' burst: _navBurst,' +
    ' setSuppression: function (v) { navHoldSuppression = v; },' +
    ' K: { DEBOUNCE_MS: DEBOUNCE_MS, DRAIN: NAV_RELEASE_DRAIN_MS, STALE: NAV_HOLD_STALE_MS,' +
    '      GAP: NAV_BURST_GAP_MS, FRESH: NAV_PRESS_FRESH_MS, ECHO: NAV_EDGE_ECHO_MS } };'
  )(
    fakeWindow, setT, clrT, () => now,
    msg => { if (msg.type === 'speak') spoken.push({ t: now, txt: msg.text }); },
    () => {}, () => false, fn => { cleanups.push(fn); return fn; },
    () => { kicks.n++; }, () => false, () => false, () => ''
  );

  const s = Object.assign({}, api, {
    spoken, kicks, advance, fakeWindow, cleanups,
    now: () => now,
    texts: () => spoken.map(x => x.txt),
    move: txt => api.scheduleSpeak(txt, api.P.CONTROLLER),
  });
  // The engine moves focus ~40-80ms after the press (measured), and a tap is
  // down for roughly 100ms.
  s.tap = (name, txt) => {
    s.input(name, 1); advance(45);
    const at = now; s.move(txt);
    advance(50); s.input(name, 0);
    return at;
  };
  return s;
}

let failures = 0;
function check(name, cond, detail) {
  if (cond) { console.log(`  PASS  ${name}`); }
  else { failures++; console.log(`  FAIL  ${name}${detail ? ' -> ' + detail : ''}`); }
}

// What the engine really sends, and what the matcher used to look for.
const OLD_NAV_ACTION_RE = /(^|_)(up|down|left|right|prev|previous|next|scroll|tab|change)(_|$)/;
// Crossfire's own repeat timing for a held direction (crossfire.js).
const REPEAT_INITIAL_MS = 500;
const REPEAT_MS = 200;

// --- Scenario 1: the engine's event names are recognised --------------------
{
  console.log('\nScenario 1: engine event names');
  const s = makeSim();
  const names = ['focus_u', 'focus_d', 'focus_l', 'focus_r', 'focus_ud', 'focus_lr', 'tab_l', 'tab_r'];
  check('every direction the engine sends is tracked',
        names.every(n => s.navActionName(n) === n), names.filter(n => !s.navActionName(n)).join(','));
  check('the previous matcher recognised none of the D-pad',
        ['focus_u', 'focus_d', 'focus_l', 'focus_r'].every(n => !OLD_NAV_ACTION_RE.test(n)));
  check('one-shot actions are not directions',
        ['ok', 'back', 'context', 'action_2', 'menu', 'rotate_h_cam'].every(n => s.navActionName(n) === ''));
}

// --- Scenario 2: single tap --------------------------------------------------
{
  console.log('\nScenario 2: single tap');
  const s = makeSim();
  s.advance(1000);
  const at = s.tap('focus_d', 'Item 5');
  s.advance(500);
  check('spoke exactly once', s.spoken.length === 1, JSON.stringify(s.spoken));
  check('spoke the item', s.spoken[0] && s.spoken[0].txt === 'Item 5');
  check('no latency beyond DEBOUNCE_MS', s.spoken[0] && s.spoken[0].t - at <= s.K.DEBOUNCE_MS,
        s.spoken[0] && String(s.spoken[0].t - at));
}

// --- Scenario 3: brisk tapping speaks every row ------------------------------
// The measured failure: seven taps 220ms apart spoke the first row, nothing for
// the next five, and the seventh 400ms after it landed.
{
  console.log('\nScenario 3: seven taps 220ms apart');
  const s = makeSim();
  s.advance(1000);
  const lat = [];
  for (let i = 1; i <= 7; i++) {
    const start = s.now();
    const at = s.tap('focus_d', 'Item ' + i);
    s.advance(220 - (s.now() - start));
    const hit = s.spoken.find(x => x.txt === 'Item ' + i);
    lat.push(hit ? hit.t - at : -1);
  }
  s.advance(1000);
  check('every tap spoken', s.spoken.length === 7, JSON.stringify(s.texts()));
  check('each within DEBOUNCE_MS of its move', lat.every(l => l >= 0 && l <= s.K.DEBOUNCE_MS), lat.join(','));

  // The same moves with no key state reach the cadence path, which is the rule
  // that used to apply to everything. It must still coalesce -- that is what
  // shows the taps above were spoken because of the press, not by accident.
  const c = makeSim();
  c.advance(1000);
  for (let i = 1; i <= 7; i++) { c.move('Item ' + i); c.advance(220); }
  c.advance(1000);
  check('without key state the same cadence coalesces to first + landing',
        c.spoken.length === 2 && c.spoken[1].txt === 'Item 7', JSON.stringify(c.texts()));
}

// --- Scenario 4: a hold speaks the first row and the landing -----------------
{
  console.log('\nScenario 4: hold through 12 rows, then release');
  const s = makeSim();
  s.advance(1000);
  s.input('focus_d', 1); s.advance(45); s.move('Item 1');
  s.advance(REPEAT_INITIAL_MS - 45);
  for (let i = 2; i <= 12; i++) { s.move('Item ' + i); s.advance(REPEAT_MS); }
  const releaseAt = s.now();
  s.input('focus_d', 0);
  s.advance(1000);
  check('spoke exactly twice (first + landing)', s.spoken.length === 2, JSON.stringify(s.texts()));
  check('first spoken is Item 1', s.spoken[0] && s.spoken[0].txt === 'Item 1');
  check('landing is Item 12', s.spoken[1] && s.spoken[1].txt === 'Item 12', JSON.stringify(s.texts()));
  const lat = s.spoken[1] ? s.spoken[1].t - releaseAt : -1;
  check('landing within NAV_RELEASE_DRAIN_MS of release', lat >= 0 && lat <= s.K.DRAIN, lat + 'ms');
}

// --- Scenario 5: landing text that arrives just after the release ------------
// A slider's value has no focus event behind it, so the release-kicked poll is
// what reads the final value. It must be taken WITHOUT re-arming the timer.
{
  console.log('\nScenario 5: landing text catches up after release');
  const s = makeSim();
  s.advance(1000);
  s.input('focus_r', 1); s.advance(10); s.move('Camber, 1.0');
  s.advance(500);
  for (let i = 2; i <= 6; i++) { s.move('Camber, ' + i + '.0'); s.advance(100); }
  const kicksBefore = s.kicks.n;
  const releaseAt = s.now();
  s.input('focus_r', 0);
  check('the release kicks the poll', s.kicks.n === kicksBefore + 1, String(s.kicks.n - kicksBefore));
  s.advance(4); s.move('Camber, 7.0');
  s.advance(1000);
  check('spoke exactly twice', s.spoken.length === 2, JSON.stringify(s.texts()));
  check('landing is the value read after release', s.spoken[1] && s.spoken[1].txt === 'Camber, 7.0',
        JSON.stringify(s.texts()));
  const lat = s.spoken[1] ? s.spoken[1].t - releaseAt : -1;
  check('late text did not push the landing out', lat >= 0 && lat <= s.K.DRAIN, lat + 'ms');
}

// --- Scenario 6: a held sweep that runs into the end of the list -------------
{
  console.log('\nScenario 6: hold into the end of a list');
  const s = makeSim();
  s.advance(1000);
  s.input('focus_d', 1); s.advance(45); s.move('Item 1');
  s.advance(REPEAT_INITIAL_MS - 45);
  for (let i = 2; i <= 5; i++) { s.move('Item ' + i); s.advance(REPEAT_MS); }
  const lastMove = s.now() - REPEAT_MS;
  s.advance(2000);                      // still held, nothing left to move to
  check('landing announced without a release', s.spoken.length === 2 && s.spoken[1].txt === 'Item 5',
        JSON.stringify(s.texts()));
  check('after NAV_HOLD_STALE_MS of no movement',
        s.spoken[1] && s.spoken[1].t - lastMove === s.K.STALE, s.spoken[1] && String(s.spoken[1].t - lastMove));
  s.input('focus_d', 0); s.advance(1000);
  check('the eventual release adds nothing', s.spoken.length === 2, JSON.stringify(s.texts()));
}

// --- Scenario 7: a tap straight after a hold supersedes the landing ----------
{
  console.log('\nScenario 7: tap inside the release drain');
  const s = makeSim();
  s.advance(1000);
  s.input('focus_d', 1); s.advance(45); s.move('Item 1');
  s.advance(REPEAT_INITIAL_MS - 45);
  for (let i = 2; i <= 4; i++) { s.move('Item ' + i); s.advance(REPEAT_MS); }
  s.input('focus_d', 0);
  s.advance(10);
  s.input('focus_u', 1); s.advance(20); s.move('Item 3'); s.advance(50); s.input('focus_u', 0);
  s.advance(1000);
  check('the row left behind is never announced', s.texts().indexOf('Item 4') === -1, JSON.stringify(s.texts()));
  check('the new row is', s.texts()[s.spoken.length - 1] === 'Item 3', JSON.stringify(s.texts()));
}

// --- Scenario 8: one press answers one announcement --------------------------
// A second utterance from the same press must not ride in as "deliberate" and
// cut the first one off while the key is still down.
{
  console.log('\nScenario 8: two utterances from one press');
  const s = makeSim();
  s.advance(1000);
  s.input('focus_d', 1); s.advance(45); s.move('Body, expanded');
  s.advance(30); s.move('Body, expanded, changed');
  s.advance(15);
  check('second is held while the key is down', s.spoken.length === 1, JSON.stringify(s.texts()));
  const releaseAt = s.now();
  s.input('focus_d', 0);
  s.advance(1000);
  check('and spoken on release', s.spoken.length === 2 && s.spoken[1].t - releaseAt <= s.K.DRAIN,
        JSON.stringify(s.spoken));
}

// --- Scenario 9: stick axis ---------------------------------------------------
// An axis reports continuously. Only the crossing is a press, or a deflected
// stick would mark every repeat as deliberate and defeat the coalescing.
{
  console.log('\nScenario 9: thumbstick axis');
  const s = makeSim();
  s.advance(1000);
  [0.1, 0.3, 0.6].forEach(v => { s.input('focus_ud', v); s.advance(5); });
  s.advance(40); s.move('Item 1');
  [0.9, 1, 1, 0.95].forEach(v => { s.input('focus_ud', v); s.advance(100); });
  s.advance(100);
  for (let i = 2; i <= 5; i++) { s.input('focus_ud', 1); s.move('Item ' + i); s.advance(REPEAT_MS); }
  check('a deflected stick is one press', s.spoken.length === 1 && s.spoken[0].txt === 'Item 1',
        JSON.stringify(s.texts()));
  s.input('focus_ud', 0.4);
  s.advance(100);
  check('inside the hysteresis band it is still held', s.spoken.length === 1, JSON.stringify(s.texts()));
  s.input('focus_ud', 0.1);
  s.advance(1000);
  check('landing on release', s.spoken.length === 2 && s.spoken[1].txt === 'Item 5', JSON.stringify(s.texts()));
}

// --- Scenario 10: the same edge delivered twice -------------------------------
// ui_nav and the AngularJS broadcast can both carry one press.
{
  console.log('\nScenario 10: echoed press');
  const s = makeSim();
  s.advance(1000);
  s.input('focus_d', 1); s.advance(2); s.input('focus_d', 1);
  check('counted once', s.burst.fresh === 1, String(s.burst.fresh));
  s.advance(43); s.move('Item 1');
  s.advance(REPEAT_INITIAL_MS); s.move('Item 2');
  s.advance(15);
  check('the repeat is not mistaken for a second press', s.spoken.length === 1, JSON.stringify(s.texts()));
}

// --- Scenario 11: a release that never arrived --------------------------------
{
  console.log('\nScenario 11: lost release');
  const s = makeSim();
  s.advance(1000);
  s.input('focus_d', 1); s.advance(45); s.move('Item 1');
  s.advance(3000);                      // release lost (focus went elsewhere)
  const at = s.tap('focus_d', 'Item 2');
  s.advance(1000);
  const hit = s.spoken.find(x => x.txt === 'Item 2');
  check('the next press still speaks at once', hit && hit.t - at <= s.K.DEBOUNCE_MS, JSON.stringify(s.spoken));
}

// --- Scenario 12: a press that moved nothing expires --------------------------
{
  console.log('\nScenario 12: stale press');
  const s = makeSim();
  s.advance(1000);
  s.move('Item 1'); s.advance(100);
  s.input('focus_d', 1); s.advance(50); s.input('focus_d', 0);   // end of list
  s.advance(s.K.FRESH + 100);
  for (let i = 2; i <= 4; i++) { s.move('Value ' + i); s.advance(100); }
  s.advance(15);
  check('later movement is not credited to it', s.spoken.length === 2 && s.texts().indexOf('Value 4') === -1,
        JSON.stringify(s.texts()));
  s.advance(1000);
  check('and still coalesces on cadence', s.spoken.length === 3 && s.spoken[2].txt === 'Value 4',
        JSON.stringify(s.texts()));
}

// --- Scenario 13: suppression disabled ----------------------------------------
{
  console.log('\nScenario 13: suppression disabled');
  const s = makeSim();
  s.setSuppression(false);
  s.advance(1000);
  s.input('focus_d', 1);
  for (let i = 1; i <= 10; i++) { s.advance(110); s.move('Item ' + i); }
  s.advance(1000);
  check('every item spoken', s.spoken.length === 10, String(s.spoken.length));
}

// --- Scenario 14: screen change must not leak a pending announcement ----------
{
  console.log('\nScenario 14: back out mid-sweep');
  const s = makeSim();
  s.advance(1000);
  s.input('focus_d', 1); s.advance(45); s.move('Old 1');
  s.advance(REPEAT_INITIAL_MS);
  for (let i = 2; i <= 6; i++) { s.move('Old ' + i); s.advance(REPEAT_MS); }
  s.navBurstReset();                 // what the screenKey-change branch does
  s.input('focus_d', 0);
  s.advance(1000);
  check('only the leading edge spoke', s.spoken.length === 1, JSON.stringify(s.texts()));
  check('nothing from the old screen leaked', s.texts().indexOf('Old 6') === -1);
}

// --- Scenario 15: system notifications are not navigation ---------------------
{
  console.log('\nScenario 15: system message during a held sweep');
  const s = makeSim();
  s.advance(1000);
  s.input('focus_d', 1); s.advance(45); s.move('Item 1');
  s.advance(REPEAT_INITIAL_MS);
  for (let i = 2; i <= 4; i++) { s.move('Item ' + i); s.advance(REPEAT_MS); }
  s.scheduleSpeak('Camera: Driver', s.P.SYSTEM);
  s.advance(60);
  for (let i = 5; i <= 8; i++) { s.move('Item ' + i); s.advance(REPEAT_MS); }
  s.input('focus_d', 0);
  s.advance(1000);
  const texts = s.texts();
  check('system message was not swallowed', texts.indexOf('Camera: Driver') !== -1, JSON.stringify(texts));
  check('sweep still coalesced around it', s.spoken.length === 3, JSON.stringify(texts));
  check('landing is still Item 8', texts[texts.length - 1] === 'Item 8', JSON.stringify(texts));
}

// --- Scenario 16: cadence path at the engine's real repeat rate ---------------
// Kept from the previous sim: with no key state, NAV_SETTLE_MAX_MS below the
// repeat interval made the settle fire between two repeats.
{
  console.log('\nScenario 16: no key state, 240ms repeats');
  const s = makeSim();
  s.advance(1000);
  for (let i = 1; i <= 12; i++) { s.advance(240); s.move('Item ' + i); }
  const lastMove = s.now();
  s.advance(2000);
  check('coalesces instead of speaking all 12', s.spoken.length === 2, JSON.stringify(s.texts()));
  check('landing is Item 12', s.spoken[1] && s.spoken[1].txt === 'Item 12');
  const lat = s.spoken[1] ? s.spoken[1].t - lastMove : -1;
  check('landing within NAV_BURST_GAP_MS of last move', lat >= 0 && lat <= s.K.GAP, lat + 'ms');
}

// --- Scenario 20: wiring ------------------------------------------------------
// None of this is reachable through a fake clock, so it is asserted textually.
{
  console.log('\nScenario 20: wiring');
  const code = source.split('\n').filter(l => !/^\s*\/\//.test(l)).join('\n');
  check('the engine focus event runs the poll',
        /listen\(document, 'uinav-focus', kickVuePoll, true\);/.test(code));
  check('ui_nav feeds the key state',
        /function handlePartsUiNavCapture[\s\S]{0,400}?trackNavigationInput\(action, value\);/.test(code));
  check('an event-driven tick skips the focus debounce',
        /processFocusChange\(focused, P\.CONTROLLER, kicked\);/.test(code));
  check('the kick is deferred, never run inline',
        /_vuePollKick = function \(\) \{ scheduleVuePoll\(0\); \};/.test(code));
  check('the poll idles between events',
        /nextDelay = nowTS\(\) < _vuePollActiveUntil \? NAV_POLL_FAST_MS : NAV_POLL_IDLE_MS;/.test(code));
  check('the old hold tracker is gone', !/trackNavigationHold/.test(code));
}

console.log(failures === 0 ? '\nAll scenarios passed.' : `\n${failures} check(s) FAILED.`);
process.exit(failures === 0 ? 0 : 1);
