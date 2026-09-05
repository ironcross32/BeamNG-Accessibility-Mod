// Simulation of the held-navigation coalescing gate from bnvdaRuntime.js.
// Replicates the state machine verbatim against a fake clock so the scenarios
// can be checked without the game.

const NAV_BURST_GAP_MS = 350;
const NAV_SETTLE_MIN_MS = 90;
const NAV_SETTLE_MAX_MS = NAV_BURST_GAP_MS;
const NAV_SETTLE_FACTOR = 1.5;
const NAV_RELEASE_DRAIN_MS = 140;
const NAV_HOLD_STALE_MS = 500;
const DEBOUNCE_MS = 25;

// The repeat interval measured off the part selector with BNVDA_DEBUG on
// (reported ema 232-244ms). Scenarios below used to run only at 110ms, which sits
// under every plausible settle ceiling and so could never catch the regression
// where the settle fires between two repeats.
const OBSERVED_REPEAT_MS = 240;

function makeSim() {
  let now = 0;
  let timers = [];
  let seq = 0;
  const spoken = [];
  let navHoldSuppression = true;

  const P = { POINTER: 1, KEYBOARD: 2, CONTROLLER: 3, SYSTEM: 4 };
  let lastSpoken = '', lastSource = 0, lastSpeakTs = -1e9, speakTimer = null;
  let loadingActive = false;

  function setT(fn, d) { const id = ++seq; timers.push({ id, at: now + d, fn }); return id; }
  function clrT(id) { timers = timers.filter(t => t.id !== id); }
  function advance(ms) {
    const end = now + ms;
    for (;;) {
      timers.sort((a, b) => a.at - b.at);
      if (!timers.length || timers[0].at > end) break;
      const t = timers.shift();
      now = t.at;
      t.fn();
    }
    now = end;
  }

  const _navBurst = { lastTs: 0, ema: 0, active: false, text: '', src: 0, timer: null, held: Object.create(null) };

  function navBurstReset() {
    if (_navBurst.timer) clrT(_navBurst.timer);
    _navBurst.timer = null; _navBurst.active = false; _navBurst.ema = 0;
    _navBurst.held = Object.create(null);
  }
  function navBurstHeldCount() {
    let n = 0; for (const k in _navBurst.held) if (_navBurst.held[k]) n++; return n;
  }
  function emitSpeak(txt, src) {
    lastSpoken = txt; lastSource = src; lastSpeakTs = now;
    spoken.push({ t: now, txt });
  }
  function navBurstFlush() {
    if (_navBurst.timer) clrT(_navBurst.timer);
    _navBurst.timer = null; _navBurst.active = false;
    const txt = _navBurst.text, src = _navBurst.src;
    _navBurst.text = ''; _navBurst.src = 0;
    if (!txt || loadingActive) return;
    if (txt === lastSpoken && src <= lastSource && (now - lastSpeakTs) < 400) return;
    emitSpeak(txt, src);
  }
  function navSettleDelay() {
    const d = _navBurst.ema * NAV_SETTLE_FACTOR;
    if (!(d > NAV_SETTLE_MIN_MS)) return NAV_SETTLE_MIN_MS;
    return d > NAV_SETTLE_MAX_MS ? NAV_SETTLE_MAX_MS : d;
  }
  function navBurstArm(delay) {
    if (_navBurst.timer) clrT(_navBurst.timer);
    _navBurst.timer = setT(navBurstOnSettle, delay);
  }
  function navBurstOnSettle() {
    _navBurst.timer = null;
    if (navBurstHeldCount() > 0 && (now - _navBurst.lastTs) < NAV_HOLD_STALE_MS) {
      navBurstArm(navSettleDelay());
      return;
    }
    navBurstFlush();
  }
  function navBurstIsNavSource(src) {
    return src === P.KEYBOARD || src === P.CONTROLLER;
  }
  function navBurstNoteRedundant(txt, src, t) {
    if (!navHoldSuppression || !navBurstIsNavSource(src)) return;
    _navBurst.lastTs = t;
    if (!_navBurst.active) return;
    _navBurst.text = ''; _navBurst.src = 0;
  }
  function navBurstCapture(txt, src, t) {
    if (!navHoldSuppression) return false;
    if (!navBurstIsNavSource(src)) return false;
    const dt = t - _navBurst.lastTs;
    _navBurst.lastTs = t;
    if (!_navBurst.active && dt > NAV_BURST_GAP_MS) { _navBurst.ema = 0; return false; }
    _navBurst.active = true;
    _navBurst.ema = _navBurst.ema ? (_navBurst.ema * 0.6 + dt * 0.4) : dt;
    _navBurst.text = txt; _navBurst.src = src;
    navBurstArm(navSettleDelay());
    return true;
  }
  function scheduleSpeak(txt, src) {
    if (!txt || loadingActive) return;
    const t = now;
    if (txt === lastSpoken && src <= lastSource && (t - lastSpeakTs) < 400) {
      navBurstNoteRedundant(txt, src, t);
      return;
    }
    if (navBurstCapture(txt, src, t)) return;
    if (speakTimer) clrT(speakTimer);
    speakTimer = setT(() => emitSpeak(txt, src), DEBOUNCE_MS);
  }
  function press(name) { _navBurst.held[name] = true; }
  function release(name) {
    if (!_navBurst.held[name]) return;
    delete _navBurst.held[name];
    if (navBurstHeldCount() === 0 && _navBurst.active) navBurstArm(NAV_RELEASE_DRAIN_MS);
  }

  return {
    P, spoken, advance, scheduleSpeak, press, release, navBurstReset,
    now: () => now,
    setSuppression: v => { navHoldSuppression = v; },
  };
}

let failures = 0;
function check(name, cond, detail) {
  if (cond) { console.log(`  PASS  ${name}`); }
  else { failures++; console.log(`  FAIL  ${name}${detail ? ' -> ' + detail : ''}`); }
}

// --- Scenario 1: single tap, key released before speech arrives -------------
{
  console.log('\nScenario 1: single tap');
  const s = makeSim();
  s.advance(1000);
  s.press('down'); s.advance(80); s.release('down');
  s.advance(165);            // poll + processFocusChange latency
  s.scheduleSpeak('Item 5', s.P.CONTROLLER);
  s.advance(500);
  check('spoke exactly once', s.spoken.length === 1, JSON.stringify(s.spoken));
  check('spoke the item', s.spoken[0] && s.spoken[0].txt === 'Item 5');
  check('no added latency (<=50ms after request)', s.spoken[0] && s.spoken[0].t - 1245 <= 50,
        s.spoken[0] && String(s.spoken[0].t - 1245));
}

// --- Scenario 2: hold through 20 items, engine emits press/release ----------
{
  console.log('\nScenario 2: hold 20 items, with release event');
  const s = makeSim();
  s.advance(1000);
  s.press('down');
  const REPEAT = 110;
  for (let i = 1; i <= 20; i++) { s.advance(REPEAT); s.scheduleSpeak('Item ' + i, s.P.CONTROLLER); }
  const releaseAt = s.now();
  s.release('down');
  s.advance(60); s.scheduleSpeak('Item 21', s.P.CONTROLLER);   // last move still draining
  s.advance(1000);
  check('spoke exactly twice (first + landing)', s.spoken.length === 2, JSON.stringify(s.spoken));
  check('first spoken is Item 1', s.spoken[0] && s.spoken[0].txt === 'Item 1');
  check('landing is the final item', s.spoken[1] && s.spoken[1].txt === 'Item 21', JSON.stringify(s.spoken));
  const lat = s.spoken[1] ? s.spoken[1].t - releaseAt : -1;
  check('landing within 200ms of release', lat >= 0 && lat <= 200, lat + 'ms');
}

// --- Scenario 3: hold with repeats only (no release event at all) -----------
{
  console.log('\nScenario 3: hold 20 items, engine emits no key state');
  const s = makeSim();
  s.advance(1000);
  const REPEAT = 110;
  for (let i = 1; i <= 20; i++) { s.advance(REPEAT); s.scheduleSpeak('Item ' + i, s.P.CONTROLLER); }
  const lastMove = s.now();
  s.advance(1000);
  check('spoke exactly twice', s.spoken.length === 2, JSON.stringify(s.spoken));
  check('first spoken is Item 1', s.spoken[0] && s.spoken[0].txt === 'Item 1');
  check('landing is Item 20', s.spoken[1] && s.spoken[1].txt === 'Item 20');
  const lat = s.spoken[1] ? s.spoken[1].t - lastMove : -1;
  check('landing within 200ms of last move', lat >= 0 && lat <= 200, lat + 'ms');
}

// --- Scenario 4: uneven cadence must not flush mid-run while still held -----
{
  console.log('\nScenario 4: uneven repeat while held');
  const s = makeSim();
  s.advance(1000);
  s.press('down');
  const gaps = [110, 110, 300, 110, 110, 260, 110];   // two stalls mid-sweep
  gaps.forEach((g, i) => { s.advance(g); s.scheduleSpeak('Item ' + (i + 1), s.P.CONTROLLER); });
  s.release('down');
  s.advance(1000);
  check('no mid-run announcements', s.spoken.length === 2, JSON.stringify(s.spoken));
  check('landing is Item 7', s.spoken[1] && s.spoken[1].txt === 'Item 7');
}

// --- Scenario 5: slow tapping stays fully verbose ---------------------------
{
  console.log('\nScenario 5: deliberate slow taps (400ms apart)');
  const s = makeSim();
  s.advance(1000);
  for (let i = 1; i <= 5; i++) { s.advance(400); s.scheduleSpeak('Item ' + i, s.P.CONTROLLER); }
  s.advance(1000);
  check('every tap spoken', s.spoken.length === 5, JSON.stringify(s.spoken.map(x => x.txt)));
}

// --- Scenario 6: feature disabled reverts to old behavior -------------------
{
  console.log('\nScenario 6: suppression disabled');
  const s = makeSim();
  s.setSuppression(false);
  s.advance(1000);
  for (let i = 1; i <= 10; i++) { s.advance(110); s.scheduleSpeak('Item ' + i, s.P.CONTROLLER); }
  s.advance(1000);
  check('speaks more than the gated case', s.spoken.length > 2, String(s.spoken.length));
}

// --- Scenario 7: landing on the same item the leading edge announced --------
{
  console.log('\nScenario 7: sweep returns to the starting item');
  const s = makeSim();
  s.advance(1000);
  s.press('down');
  s.advance(110); s.scheduleSpeak('Item 1', s.P.CONTROLLER);
  s.advance(110); s.scheduleSpeak('Item 2', s.P.CONTROLLER);
  s.advance(110); s.scheduleSpeak('Item 1', s.P.CONTROLLER);
  s.release('down');
  s.advance(1000);
  check('does not repeat the same text back-to-back', s.spoken.length === 1, JSON.stringify(s.spoken));
}

// --- Scenario 8: very long hold still gives periodic feedback ---------------
{
  console.log('\nScenario 8: 10-second continuous hold');
  const s = makeSim();
  s.advance(1000);
  s.press('down');
  for (let i = 1; i <= 100; i++) { s.advance(100); s.scheduleSpeak('Item ' + i, s.P.CONTROLLER); }
  s.release('down');
  s.advance(1000);
  // Silence through a long hold is the requirement, not a defect: "subsequent
  // changes should be ignored until the user stops adjusting".
  check('stays silent through the hold', s.spoken.length === 2, JSON.stringify(s.spoken.map(x => x.txt)));
  check('landing is Item 100', s.spoken[s.spoken.length - 1].txt === 'Item 100',
        JSON.stringify(s.spoken.map(x => x.txt)));
}

// --- Scenario 9: screen change must not leak a pending announcement ---------
{
  console.log('\nScenario 9: back out mid-sweep');
  const s = makeSim();
  s.advance(1000);
  s.press('down');
  for (let i = 1; i <= 6; i++) { s.advance(110); s.scheduleSpeak('Old ' + i, s.P.CONTROLLER); }
  s.navBurstReset();                 // what the screenKey-change branch does
  s.advance(1000);
  check('only the leading edge spoke', s.spoken.length === 1, JSON.stringify(s.spoken));
  check('nothing from the old screen leaked', !s.spoken.some(x => x.txt === 'Old 6'));
}

// --- Scenario 10: hold at the engine's REAL repeat rate --------------------
// Regression guard for the part-selector bug: with NAV_SETTLE_MAX_MS below the
// repeat interval, the settle timer fired in the gap between two repeats, so
// every row was announced AND every announcement paid the full settle delay.
// The old 110ms scenarios could not catch this -- 110 is under any plausible
// ceiling, so the timer never got a chance to fire early.
{
  console.log('\nScenario 10: hold 12 items at the observed 240ms repeat rate');
  const s = makeSim();
  s.advance(1000);
  for (let i = 1; i <= 12; i++) { s.advance(OBSERVED_REPEAT_MS); s.scheduleSpeak('Item ' + i, s.P.CONTROLLER); }
  const lastMove = s.now();
  s.advance(2000);
  check('coalesces instead of speaking all 12', s.spoken.length === 2,
        JSON.stringify(s.spoken.map(x => x.txt)));
  check('first spoken is Item 1', s.spoken[0] && s.spoken[0].txt === 'Item 1');
  check('landing is Item 12', s.spoken[1] && s.spoken[1].txt === 'Item 12');
  const lat = s.spoken[1] ? s.spoken[1].t - lastMove : -1;
  check('landing within NAV_BURST_GAP_MS of last move', lat >= 0 && lat <= NAV_BURST_GAP_MS, lat + 'ms');
}

// --- Scenario 11: system notifications are not navigation ------------------
// A toast, damage callout or camera change arriving mid-sweep must be spoken on
// its own schedule. Previously P.SYSTEM passed the `src < P.KEYBOARD` gate, so
// it was captured into the burst and then discarded when the next focus move
// overwrote _navBurst.text.
{
  console.log('\nScenario 11: system message during a held sweep');
  const s = makeSim();
  s.advance(1000);
  for (let i = 1; i <= 4; i++) { s.advance(OBSERVED_REPEAT_MS); s.scheduleSpeak('Item ' + i, s.P.CONTROLLER); }
  s.advance(20);
  s.scheduleSpeak('Camera: Driver', s.P.SYSTEM);
  s.advance(60);
  for (let i = 5; i <= 8; i++) { s.advance(OBSERVED_REPEAT_MS); s.scheduleSpeak('Item ' + i, s.P.CONTROLLER); }
  s.advance(2000);
  const texts = s.spoken.map(x => x.txt);
  check('system message was not swallowed', texts.indexOf('Camera: Driver') !== -1, JSON.stringify(texts));
  check('sweep still coalesced around it', s.spoken.length === 3, JSON.stringify(texts));
  check('landing is still Item 8', texts[texts.length - 1] === 'Item 8', JSON.stringify(texts));
}

console.log(failures === 0 ? '\nAll scenarios passed.' : `\n${failures} check(s) FAILED.`);
process.exit(failures === 0 ? 0 : 1);
