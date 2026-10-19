// PARKED: the tuning slider commit debounce and tuning apply guard, removed
// from bnvdaRuntime.js on 2026-10-19 pending verification on BeamNG 0.40.
//
// 0.40's notes say "Tuning sliders no longer apply while you drag them" and
// fix d-pad holding on sliders, which is the stock race this worked around
// (docs/lua-ui-runtime-tuning.md has the full reasoning). If the car still
// reloads mid-sweep or mid-taps on the Tuning tab, restore it:
//
//   1. Paste PART 1 back into bnvdaRuntime.js just above
//      "// ---------- EVENT HOOKS AND INITIALIZATION ----------".
//   2. Put the call back in startVueFocusWatcher's poll, right after
//      "var root = vueScreenRoot();":
//          updateTuningSliderDebounce(root);
//   3. Paste PART 2 back into diagnostic/navburst_sim.js above Scenario 20,
//      and restore the sim's tuning hooks: the TUNING_* and _tuning* entries
//      in LINES, tuningAdjusting/releaseHeldTuningApply/hookTuningApply in
//      FNS, hookTuningApply and QUIET/FALLBACK/SLIDER in the returned api,
//      and the Scenario 20 check:
//          check('the tuning guard is installed from the config screen',
//                /if \(onConfigScreen\) hookTuningApply\(\);/.test(code));
//   4. Drop the "parked" note from docs/lua-ui-runtime-tuning.md.
//
// Verified on 0.40: the d-pad works without it. Kept for reference in case
// the race comes back in a later build.
//
// This file is not loaded by anything.

// ======================= PART 1: bnvdaRuntime.js =======================

          // ========== TUNING SLIDER COMMIT DEBOUNCE ==========
          // Holding a direction to sweep a value on the vehicle tuning page makes
          // the game reload the vehicle MID-SWEEP, and the value then bounces as
          // the rebuilt list re-reads it. That is stock behaviour, and it is a
          // race between three pieces of stock timing:
          //
          //   bngSlider.vue     debounces its valueChanged emit by 500ms
          //   Tuning.vue        debounces the apply (a vehicle reload) by 1000ms
          //   BngOnUiNavFocus   delivers the 2nd step of a held direction at
          //                     HOLD_DELAY(400) + REPEAT_INTERVAL(100) = 500ms
          //
          // So the slider's timer and the second step are scheduled for the same
          // millisecond -- and the nav path loses CONSISTENTLY, because it is two
          // chained timers each paying its own dispatch latency while the debounce
          // is a single one. Measured first-gaps over four runs: 511, 524, 510 and
          // 525ms, never below 500. The slider therefore commits during the hold,
          // which starts the 1000ms apply, which reloads the car a second later
          // while the value is still racing.
          //
          // Raising the emit debounce past the repeat gap is enough. Once the fast
          // run starts, the steps are ~100ms apart and clear the timer every time,
          // so nothing about the sweep itself changes; the only difference is that
          // the commit lands TUNING_SLIDER_DEBOUNCE_MS after release rather than
          // 500ms. Verified in game: one apply, after release, no mid-hold reload.
          var TUNING_SLIDER_DEBOUNCE_MS = 800;

          // Vue normalizes a component's props declaration ONCE and caches the
          // result in appContext.propsCache (a WeakMap). comp.props is never read
          // again, so writing the new default there alone does nothing -- that
          // cached copy is what resolvePropValue reads for every future instance.
          // (Patching only comp.props is exactly what silently failed first: the
          // value read back as 800 and every new slider still debounced at 500.)
          // Both are written, and the cache's own set() is hooked as well, because
          // the FIRST slider of a session is normalized the moment the Tuning tab
          // opens, when there is no instance yet for us to walk up from.
          //
          // The value is captured at setup into each slider's own debounce
          // closure, so this only reaches sliders built AFTER it is applied -- and
          // NOT ones already on screen. That is why it goes on when the config
          // screen opens: the tuning sliders are created later, when the Tuning
          // tab itself is opened. It is put back on the way out, so sliders
          // elsewhere in the UI keep stock timing.
          var _sliderComp = null;            // the bngSlider component definition
          var _sliderCtx = null;             // its Vue appContext
          var _sliderStockDebounce = null;   // whatever the game shipped
          var _sliderCacheHooked = false;
          var _sliderPatched = false;      // config screen is open
          var _sliderDebounceWritten = false;  // our value is currently installed

          function sliderDeclaresDebounce(comp) {
            return !!(comp && comp.props && comp.props.debounce);
          }

          function sliderNormalizedProps() {
            var cached = _sliderCtx && _sliderCtx.propsCache && _sliderComp &&
              _sliderCtx.propsCache.get(_sliderComp);
            return (cached && cached[0]) ? cached[0] : null;
          }

          // Keyed on the debounce prop rather than on the component name, so an
          // upstream rename disables the fix instead of silently patching some
          // other component. The walk starts inside the slider, so bngSlider is
          // the first ancestor that can match.
          function findTuningSliderComponent() {
            var row = document.querySelector('.innerTuningCard .input-container');
            if (!row) return null;
            var node = row.querySelector('input') || row;
            while (node) {
              var vc = node.__vueParentComponent, hops = 0;
              while (vc && hops < 8) {
                if (sliderDeclaresDebounce(vc.type)) { _sliderCtx = vc.appContext; return vc.type; }
                vc = vc.parent; hops++;
              }
              node = node.parentElement;
            }
            return null;
          }

          // The app does NOT mount on #app in 0.39 (measured: no such element), so
          // the Vue screen root the focus watcher already resolves is the reliable
          // handle; #app is only a fallback for other builds.
          function vueAppContext(probeRoot) {
            if (_sliderCtx) return _sliderCtx;
            var probe = probeRoot || document.querySelector('.pause-tab-combined, .innerTuningCard');
            while (probe) {
              if (probe.__vueParentComponent && probe.__vueParentComponent.appContext) {
                return probe.__vueParentComponent.appContext;
              }
              probe = probe.parentElement;
            }
            var host = document.querySelector('#app');
            if (host && host.__vue_app__ && host.__vue_app__._context) return host.__vue_app__._context;
            return null;
          }

          function hookSliderPropsCache(ctx) {
            if (_sliderCacheHooked || !ctx || !ctx.propsCache || typeof ctx.propsCache.set !== 'function') return;
            var cache = ctx.propsCache;
            var originalSet = cache.set;
            cache.set = function (comp, normalized) {
              try {
                // <script setup> components expose __name; others expose name.
                // Testing only one of them is how this silently matched nothing.
                var named = comp && (comp.__name === 'bngSlider' || comp.name === 'bngSlider');
                if (named && sliderDeclaresDebounce(comp) &&
                  normalized && normalized[0] && normalized[0].debounce) {
                  // Record the component even when we are not patching right now:
                  // this is the ONLY moment it can be captured before the first
                  // slider is built, and the whole point is to be ready in advance.
                  _sliderComp = comp; _sliderCtx = ctx;
                  if (_sliderStockDebounce === null) _sliderStockDebounce = normalized[0].debounce.default;
                  if (_sliderPatched) {
                    normalized[0].debounce.default = TUNING_SLIDER_DEBOUNCE_MS;
                    log('info', '[bnvda] Tuning slider commit debounce ' + _sliderStockDebounce +
                      'ms -> ' + TUNING_SLIDER_DEBOUNCE_MS + 'ms (at first normalize).');
                  }
                }
              } catch (e) {}
              return originalSet.call(this, comp, normalized);
            };
            onCleanup(function () { if (cache.set !== originalSet) cache.set = originalSet; });
            _sliderCacheHooked = true;
          }

          function writeSliderDebounce(value) {
            if (value === null || value === undefined || !_sliderComp) return false;
            var norm = sliderNormalizedProps();
            if (_sliderStockDebounce === null) {
              _sliderStockDebounce = (norm && norm.debounce) ? norm.debounce.default
                : (_sliderComp.props && _sliderComp.props.debounce ? _sliderComp.props.debounce.default : null);
            }
            if (_sliderComp.props && _sliderComp.props.debounce) _sliderComp.props.debounce.default = value;
            if (norm && norm.debounce) norm.debounce.default = value;
            return true;
          }

          // Hooked from the ordinary focus poll, and deliberately NOT only on the
          // screen transition. Two orderings have to be survived:
          //
          //   * The cache hook has to be armed LONG before the config screen, so
          //     it is installed on any Vue screen. bngSlider is normalized the
          //     first time one is rendered anywhere in the UI, which may well be
          //     a settings screen earlier in the session -- once that has
          //     happened set() never fires for it again.
          //   * At config-screen entry the PARTS tab is showing, so there is no
          //     tuning slider in the DOM to walk up from and the resolve fails.
          //     Retrying while the screen is open is what the first fix was
          //     missing: it tried exactly once, at the transition, and gave up.
          //
          // The DOM resolve is still only a fallback -- by the time a tuning
          // slider exists it was already built at the stock 500ms -- so it fixes
          // the NEXT rebuild. The cache hook is what makes the first visit right.
          // ---------- TUNING APPLY GUARD ----------
          // The slider debounce above stops a HELD sweep reloading the car. Taps
          // get through it by a different door. Tuning.vue restarts its 1000ms
          // apply timer only when a slider COMMITS, never when one is nudged, so
          // a single pause between taps long enough for the slider to commit --
          // the length of hearing the new value read back -- starts a reload
          // that nothing afterwards can postpone. It lands a second later, in the
          // middle of the taps that followed. Worse, the store applies its own
          // copy of the values, which only moves on a commit: the taps made since
          // are not in it, and the reload's data refresh then writes the older
          // value back over the slider.
          //
          // So an apply that arrives while a direction is down, or within the
          // slider's own commit window of the last press, is held. Normally
          // nothing more is needed: that same press has a commit of its own
          // coming, which starts the stock apply timer again, and by the time
          // THAT fires the user has stopped and it passes straight through with
          // everything in it -- one reload, at the stock moment. The timer below
          // is only for a press that changed nothing (a slider already at its
          // limit), which schedules no commit and so would leave the held apply
          // without a successor.
          var TUNING_APPLY_QUIET_MS = TUNING_SLIDER_DEBOUNCE_MS + 100;
          // Past the stock 1000ms apply delay on top of the commit, so the game's
          // own follow-up always gets there first when there is one.
          var TUNING_APPLY_FALLBACK_MS = TUNING_SLIDER_DEBOUNCE_MS + 1200;
          // A release that never arrives must not hold an apply back forever.
          var TUNING_HOLD_STALE_MS = 15000;
          var _tuningStore = null;
          var _tuningStockApply = null;
          var _tuningApplyHeld = false;
          var _tuningApplyTimer = null;

          function tuningAdjusting() {
            var t = nowTS();
            if ((t - _navLastPressTs) < TUNING_APPLY_QUIET_MS) return true;
            for (var k in _navBurst.held) {
              if (_navBurst.held[k] && (t - _navBurst.held[k]) < TUNING_HOLD_STALE_MS) return true;
            }
            return false;
          }

          function releaseHeldTuningApply() {
            _tuningApplyTimer = null;
            if (!_tuningApplyHeld) return;
            if (tuningAdjusting() || (nowTS() - _navLastPressTs) < TUNING_APPLY_FALLBACK_MS) {
              _tuningApplyTimer = trackedSetTimeout(releaseHeldTuningApply, 250);
              return;
            }
            _tuningApplyHeld = false;
            log('info', '[bnvda] Tuning apply released by fallback (no commit followed).');
            try { _tuningStockApply.call(_tuningStore); } catch (e) {}
          }

          // The store is reached through pinia's own registry (`_s`, the id -> store
          // map every useStore() call reads), the same class of internal as the
          // propsCache above. Tuning.vue calls tuningStore.apply() by property
          // lookup at call time, so replacing the property is enough; it is put
          // back on cleanup.
          function hookTuningApply() {
            if (_tuningStore) return;
            var app = window.bngVue && window.bngVue.app;
            var pinia = app && app.config && app.config.globalProperties && app.config.globalProperties.$pinia;
            var store = pinia && pinia._s && typeof pinia._s.get === 'function' ? pinia._s.get('tuning') : null;
            if (!store || typeof store.apply !== 'function') return;
            _tuningStore = store;
            _tuningStockApply = store.apply;
            var guarded = function () {
              if (tuningAdjusting()) {
                _tuningApplyHeld = true;
                if (!_tuningApplyTimer) _tuningApplyTimer = trackedSetTimeout(releaseHeldTuningApply, 250);
                if (isDebug()) log('info', '[bnvda] Tuning apply held: still adjusting.');
                return;
              }
              _tuningApplyHeld = false;
              if (_tuningApplyTimer) { clearTimeout(_tuningApplyTimer); _tuningApplyTimer = null; }
              return _tuningStockApply.apply(this, arguments);
            };
            store.apply = guarded;
            onCleanup(function () { if (store.apply === guarded) store.apply = _tuningStockApply; });
            log('info', '[bnvda] Tuning apply guard installed.');
          }

          function updateTuningSliderDebounce(root) {
            if (!_sliderCacheHooked && root) hookSliderPropsCache(vueAppContext(root));
            var onConfigScreen = !!(root && vehicleConfigRoot(root));
            if (onConfigScreen) hookTuningApply();
            if (onConfigScreen) {
              if (!_sliderPatched) _sliderPatched = true;
              if (!_sliderComp) _sliderComp = findTuningSliderComponent();
              if (_sliderComp && !_sliderDebounceWritten) {
                if (writeSliderDebounce(TUNING_SLIDER_DEBOUNCE_MS)) {
                  _sliderDebounceWritten = true;
                  log('info', '[bnvda] Tuning slider commit debounce ' + _sliderStockDebounce +
                    'ms -> ' + TUNING_SLIDER_DEBOUNCE_MS + 'ms.');
                }
              }
            } else if (_sliderPatched) {
              _sliderPatched = false;
              if (_sliderDebounceWritten && writeSliderDebounce(_sliderStockDebounce)) {
                _sliderDebounceWritten = false;
                log('info', '[bnvda] Tuning slider commit debounce restored to ' + _sliderStockDebounce + 'ms.');
              }
            }
          }

// ===================== PART 2: diagnostic/navburst_sim.js =====================

// ---------- tuning apply guard ----------
// A stand-in for Tuning.vue + bngSlider + the pinia store, with the stock
// timers: the slider commits `debounce` after its last nudge, and each commit
// restarts a 1000ms apply timer. Nudging does NOT touch the apply timer, which
// is the whole fault.
function makeTuning(s, guarded) {
  const applies = [];
  const store = { value: 0, apply() { applies.push({ t: s.now(), value: store.value }); } };
  s.fakeWindow.bngVue = { app: { config: { globalProperties: { $pinia: { _s: new Map([['tuning', store]]) } } } } };
  if (guarded) s.hookTuningApply();
  // The stock timers are stepped against the sim clock, so they interleave
  // with the guard's own exactly as in the page.
  let live = 0, commitAt = -1, applyAt = -1;
  function tick() {
    if (commitAt >= 0 && s.now() >= commitAt) { commitAt = -1; store.value = live; applyAt = s.now() + 1000; }
    if (applyAt >= 0 && s.now() >= applyAt) { applyAt = -1; store.apply(); }
  }
  return {
    applies, store,
    nudge(dir, changes) {
      s.input(dir > 0 ? 'focus_r' : 'focus_l', 1);
      if (changes !== false) { live += dir; commitAt = s.now() + s.K.SLIDER; }
      s.advance(1); tick();
      s.input(dir > 0 ? 'focus_r' : 'focus_l', 0);
    },
    run(ms) { for (let i = 0; i < ms; i += 10) { s.advance(10); tick(); } },
    live: () => live,
  };
}

// --- Scenario 17: taps with one pause, the reported fault ---------------------
{
  console.log('\nScenario 17: tuning taps with a pause to listen');
  const drive = t => {
    for (let i = 0; i < 3; i++) { t.nudge(1); t.run(300); }
    t.run(700);                                   // hear the value: slider commits
    for (let i = 0; i < 5; i++) { t.nudge(1); t.run(400); }
    const lastNudge = t.s.now() - 400;
    t.run(4000);
    return lastNudge;
  };

  const s0 = makeSim(); s0.advance(1000);
  const stock = makeTuning(s0, false); stock.s = s0;
  const stockLast = drive(stock);
  check('unguarded, the car reloads in the middle of the taps',
        stock.applies.length >= 1 && stock.applies[0].t < stockLast, JSON.stringify(stock.applies));
  check('and applies a value the slider had already moved past',
        stock.applies[0] && stock.applies[0].value < stock.live(), JSON.stringify(stock.applies[0]));

  const s1 = makeSim(); s1.advance(1000);
  const g = makeTuning(s1, true); g.s = s1;
  const lastNudge = drive(g);
  check('guarded, exactly one apply', g.applies.length === 1, JSON.stringify(g.applies));
  check('after the last tap', g.applies[0] && g.applies[0].t > lastNudge, JSON.stringify(g.applies));
  check('carrying the final value', g.applies[0] && g.applies[0].value === g.live(), JSON.stringify(g.applies));
  check('at the stock moment (commit + 1000ms), not later',
        g.applies[0] && Math.abs(g.applies[0].t - (lastNudge + s1.K.SLIDER + 1000)) <= 20,
        g.applies[0] && String(g.applies[0].t - lastNudge));
}

// --- Scenario 18: a held apply with no commit behind it -----------------------
// The taps after the pause change nothing (slider at its limit), so no commit
// follows and the stock path never calls apply again.
{
  console.log('\nScenario 18: tuning taps against the limit');
  const s = makeSim(); s.advance(1000);
  const t = makeTuning(s, true);
  t.nudge(1); t.run(900);                         // commits; apply due in 1000
  for (let i = 0; i < 4; i++) { t.nudge(1, false); t.run(300); }
  const lastNudge = s.now() - 300;
  t.run(5000);
  check('the held apply is not lost', t.applies.length === 1, JSON.stringify(t.applies));
  check('released by the fallback, after the user stopped',
        t.applies[0] && t.applies[0].t - lastNudge >= s.K.FALLBACK - 5, t.applies[0] && String(t.applies[0].t - lastNudge));
}

// --- Scenario 19: an apply with nobody adjusting is untouched -----------------
{
  console.log('\nScenario 19: tuning apply while idle');
  const s = makeSim(); s.advance(5000);
  const t = makeTuning(s, true);
  t.store.apply();
  check('passes straight through', t.applies.length === 1 && t.applies[0].t === s.now(), JSON.stringify(t.applies));
  s.cleanups.forEach(fn => fn());
  t.store.apply();
  check('and the stock apply is restored on cleanup', t.applies.length === 2);
}
