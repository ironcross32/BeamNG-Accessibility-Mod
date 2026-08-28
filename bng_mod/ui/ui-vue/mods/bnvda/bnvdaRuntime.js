export function installBNVDA($rootScope, dependencies) {
  if (typeof window.__BNvDA_INSTALLED__ === 'function') return window.__BNvDA_INSTALLED__;

  var disposed = false;
  var cleanupTasks = [];
  var timeoutIds = new Set();
  var intervalIds = new Set();
  var animationIds = new Set();
  var observerInstances = [];
  function onCleanup(task) { if (typeof task === 'function') cleanupTasks.push(task); return task; }
  function trackedSetTimeout(callback, delay) {
    var args = Array.prototype.slice.call(arguments, 2);
    var id = window.setTimeout(function () { timeoutIds.delete(id); if (!disposed) callback.apply(this, args); }, delay);
    timeoutIds.add(id); return id;
  }
  function trackedSetInterval(callback, delay) {
    var args = Array.prototype.slice.call(arguments, 2);
    var id = window.setInterval(function () { if (!disposed) callback.apply(this, args); }, delay);
    intervalIds.add(id); return id;
  }
  function trackedRequestAnimationFrame(callback) {
    var id = window.requestAnimationFrame(function (timestamp) { animationIds.delete(id); if (!disposed) callback(timestamp); });
    animationIds.add(id); return id;
  }
  function trackedMutationObserver(callback) {
    var observer = new window.MutationObserver(function (mutations, instance) { if (!disposed) callback(mutations, instance); });
    observerInstances.push(observer); return observer;
  }
  function listen(target, type, handler, options) {
    target.addEventListener(type, handler, options);
    onCleanup(function () { target.removeEventListener(type, handler, options); });
    return handler;
  }
  function subscribe(scope, eventName, handler) {
    if (!scope || typeof scope.$on !== 'function') return function () {};
    return onCleanup(scope.$on(eventName, handler));
  }
  function uninstall() {
    if (disposed) return;
    disposed = true;
    timeoutIds.forEach(function (id) { window.clearTimeout(id); });
    intervalIds.forEach(function (id) { window.clearInterval(id); });
    animationIds.forEach(function (id) { window.cancelAnimationFrame(id); });
    observerInstances.forEach(function (observer) { observer.disconnect(); });
    timeoutIds.clear(); intervalIds.clear(); animationIds.clear(); observerInstances.length = 0;
    while (cleanupTasks.length) { try { cleanupTasks.pop()(); } catch (error) { console.error('[bnvda] Cleanup failed.', error); } }
    if (window.__BNvDA_INSTALLED__ === uninstall) delete window.__BNvDA_INSTALLED__;
    console.info('[bnvda] Runtime cleanup complete.');
  }
  window.__BNvDA_INSTALLED__ = uninstall;

  try {
    (function () {
          "use strict";

          dependencies = dependencies || {};
          var Controls = dependencies.controls || null;
          var iconCatalog = dependencies.icons || null;
          var loadingScreen = dependencies.loadingScreen || null;
          var RadialCenterCanvas = dependencies.radialCenterCanvas || null;
          var vueWatch = dependencies.watch || null;
          var loadingActive = false;
          var loadingSettleTimer = null;
          var suppressNextCameraEvent = true;
          var sawLoadingStart = false;

          // ---------- CONFIG ----------
          // The poll interval, FOCUS_DEBOUNCE_MS and DEBOUNCE_MS are serial and do
          // not overlap, so they add up on every announcement. Keep them small.
          var DEBOUNCE_MS = 25;
          // Coalesces the three producers that fire for a single focus change --
          // the MutationObserver, focusin, and the Vue poll. Those land within a
          // few ms of each other, so this does not need to be large.
          var FOCUS_DEBOUNCE_MS = 30;
          var CONTROLLER_DOMINANCE_MS = 900;
          var MIN_CHARS = 2;
          var MAX_LEN = 160;

          // Held-navigation coalescing. Holding a direction to sweep a list, slider
          // or dropdown otherwise speaks every item it flies past, which buries the
          // one the user actually lands on. Instead: speak the first item, stay
          // silent through the run, and announce the landing as soon as motion
          // stops. The settle delay is derived from the observed repeat cadence
          // rather than a fixed worst case, because a fixed ~500ms idle debounce
          // makes the whole interface feel sluggish even when it is not.
          var NAV_BURST_GAP_MS = 350;    // idle longer than this starts a new burst
          var NAV_SETTLE_MIN_MS = 90;
          // MUST stay >= the engine's controller repeat interval, or the settle timer
          // fires in the gap between two repeats and flushes on every single item --
          // which both defeats the coalescing entirely and still charges its full
          // settle delay to every announcement. That is exactly what a 200ms ceiling
          // did: the part selector repeats at ~240ms (measured ema 232-244), so every
          // row was announced, each one 200ms late. Tying it to NAV_BURST_GAP_MS is
          // the principled ceiling -- a gap longer than that already starts a new
          // burst by definition, so settling at that boundary can never cut short a
          // run the gap logic still considers continuous.
          var NAV_SETTLE_MAX_MS = NAV_BURST_GAP_MS;
          var NAV_SETTLE_FACTOR = 1.5;   // settle = clamp(ema * FACTOR, MIN, MAX)
          var NAV_POLL_FAST_MS = 60;     // Vue focus poll interval during a burst
          // On release the last focus move is still in flight (the poll interval plus
          // FOCUS_DEBOUNCE_MS), so drain before announcing rather than
          // speaking the second-to-last item and then correcting it.
          var NAV_RELEASE_DRAIN_MS = 140;
          // A key reported as held but with no movement for this long is treated as
          // stale, so a release event that never arrives (focus loss, alt-tab)
          // cannot strand the announcement. Comfortably longer than the worst
          // uneven-repeat gap, so it never cuts a real sweep short.
          var NAV_HOLD_STALE_MS = 500;
          // Mirrors beamtel's ui_nav_hold_suppression. Defaults to the config
          // default so behavior is right before the settings reply arrives.
          var navHoldSuppression = true;
          // Evaluated live on every use rather than latched at install time.
          // BNVDA_DEBUG is set from the accessible console (CEF/UI - JS context)
          // after the runtime has already installed, and reloading the UI to
          // re-run install resets the JS context, clearing the global before it
          // would be read. Latching it left no sequence that could enable debug.
          function isDebug() { return !!window.BNVDA_DEBUG; }

          // Mirror the flag to the Python side so its debug output can be toggled from
          // the same accessible console command. Polled rather than hooked: the console
          // assigns window.BNVDA_DEBUG directly, which fires no event we could observe.
          var lastReportedDebug = null;
          function reportDebugState() {
            var state = isDebug();
            if (state === lastReportedDebug) return;
            lastReportedDebug = state;
            send({ type: 'debug_state', enabled: state });
          }

          // ---------- TRANSPORTS ----------
          var activeTransport = null;
          function receiveTransportMessage(data) {
            if (!data) return;
            if (data.type === 'transport_pong') {
              if (loadingActive) send({ type: 'loading_state', active: true, focusText: '' });
              return;
            }
            if (data.type === 'context_action') handleContextAction(data.action);
            else if (data.type === 'dom_dump') performDomDump();
            else if (data.type === 'settings') applySettings(data);
          }
          function applySettings(data) {
            if (typeof data.ui_nav_hold_suppression === 'boolean') {
              if (navHoldSuppression !== data.ui_nav_hold_suppression) {
                navHoldSuppression = data.ui_nav_hold_suppression;
                if (!navHoldSuppression) navBurstReset();
                log('info', '[bnvda] Held-navigation coalescing ' + (navHoldSuppression ? 'enabled' : 'disabled') + '.');
              }
            }
          }
          function activateTransport(transport) {
            if (activeTransport && activeTransport !== transport) activeTransport.shutdown();
            activeTransport = transport;
            console.info('[bnvda] Active transport: ' + transport.name);
            transport.send({type: 'log', level: 'info', msg: '[bnvda] Active transport: ' + transport.name});
            // Re-send on a new transport: the far side has no state from the old one.
            lastReportedDebug = null;
            reportDebugState();
            // Pull rather than wait for a push: beamtel broadcasts on config change,
            // but at startup that fires before the bridge is connected and the frame
            // would be dropped.
            transport.send({ type: 'settings_request' });
          }
          function send(obj) {
            if (activeTransport) activeTransport.send(obj);
          }
          function log(level, msg) {
            console[level === 'error' ? 'error' : 'info'](msg);
            send({ type: "log", level: level, msg: msg });
          }
          function speechValue(value, source) {
            while (value !== null && typeof value === 'object') {
              var keys = Object.keys(value);
              if (isDebug()) {
                var serialized;
                try { serialized = JSON.stringify(value); } catch (e) { serialized = '[unserializable: ' + String(e) + ']'; }
                try { log('warn', '[LUA_TABLE_SPEECH] source=' + source + ' count=' + keys.length + ' contents=' + serialized); } catch (e) {}
              }
              if (keys.length !== 1) return null;
              value = value[keys[0]];
            }
            if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return String(value);
            return null;
          }

          function createLuaTCPTransport() {
            var stopped = false;
            var transport = {
              name: 'lua-tcp',
              start: function () { activateTransport(transport); },
              send: function (obj) {
                if (stopped || !window.bngApi || !bngApi.engineLua) return;
                var payload = JSON.stringify(obj);
                var luaValue = bngApi.serializeToLua ? bngApi.serializeToLua(payload) : JSON.stringify(payload);
                bngApi.engineLua('extensions.bnvdaBridge.sendFromUI(' + luaValue + ')');
              },
              shutdown: function () { stopped = true; }
            };
            onCleanup(function () { transport.shutdown(); if (activeTransport === transport) activeTransport = null; });
            return transport;
          }

          function startTransportSelection() {
            // The engine bridge is already available when this UI app loads and
            // keeps transport work outside CEF. Avoid redundant HTTP probes and
            // two five-second WebSocket attempts on every UI recreation.
            createLuaTCPTransport().start();
          }

          if ($rootScope && typeof $rootScope.$on === 'function') {
            subscribe($rootScope, 'BNVDATransportMessage', function (_event, data) {
              if (activeTransport && activeTransport.name === 'lua-tcp') receiveTransportMessage(data);
            });
          }

          function logFocusedElementDetails(element, eventType) {
            if (!element) return;
            var details = "BNVDA Focus Event (" + eventType + "):\n" +
                          "  Tag Name:    " + element.tagName + "\n" +
                          "  ID:          " + (element.id || 'N/A') + "\n" +
                          "  Classes:     " + (element.className || 'N/A') + "\n" +
                          "  Inner Text:  '" + element.innerText.replace(/\s+/g, ' ').trim() + "'\n" +
                          "  Outer HTML:  " + element.outerHTML;
            log('info', details);
          }


          // ---------- HELPERS ----------
          function nowTS() { return (window.performance && performance.now) ? performance.now() : Date.now(); }
          // A DOM `value` property is not necessarily text. Vue components and
          // custom elements routinely expose a boolean or an object there, and
          // cleanText stringifies whatever it is handed -- so a boolean `true`
          // was announced as the literal word "true" inside a control's state
          // list. The artifact was always "true" and never "false" because
          // cleanText's leading falsy check silently maps `false` to "".
          function scalarValue(v) {
            return (typeof v === 'string' || typeof v === 'number') ? v : '';
          }

          function cleanText(s) {
            if (!s) return "";
            // Strip bngIcons glyphs (Unicode Private Use Area U+E000-U+F8FF)
            s = String(s).replace(/[\uE000-\uF8FF]/g, '');
            s = s.replace(/\s+/g, " ").trim();
            if (s.length > MAX_LEN) s = s.slice(0, MAX_LEN - 3) + "...";
            return s;
          }
          // Detect raw CSS that leaks in when a handler reads textContent of a
          // non-rendered element (e.g. a <style> block) — innerText is empty for
          // hidden nodes so extraction falls back to textContent, which for a
          // style element is the stylesheet itself. This happens notably when the
          // UI is toggled off. Never speak such text.
          function looksLikeCss(s) {
            if (!s) return false;
            if (/[#.\w-]+\s*\{[^}]*:[^}]*;/.test(s)) return true;   // selector { prop: val; }
            if (/\}\s*[#.\w-]+\s*\{/.test(s)) return true;          // } selector {
            if (/\b(rgba?|md-|color|background|padding|margin)\b[^;{]*\{/.test(s)) return true;
            return false;
          }
          function throttle(func, limit) {
            var lastFunc, lastRan;
            return function() {
              var context = this, args = arguments;
              if (!lastRan) {
                func.apply(context, args);
                lastRan = Date.now();
              } else {
                clearTimeout(lastFunc);
                lastFunc = trackedSetTimeout(function() {
                  if ((Date.now() - lastRan) >= limit) {
                    func.apply(context, args);
                    lastRan = Date.now();
                  }
                }, limit - (Date.now() - lastRan));
              }
            }
          }
          function stripHtml(html) { var tmp = document.createElement("div"); tmp.innerHTML = html; return (tmp.textContent || tmp.innerText || ""); }
          function toArray(list) { try { return Array.prototype.slice.call(list); } catch (e) { var a = []; for (var i = 0; i < list.length; i++) a.push(list[i]); return a; } }
          function firstVisible(sel) {
            var nodes = toArray(document.querySelectorAll(sel));
            for (var i = 0; i < nodes.length; i++) {
              var el = nodes[i];
              if (el && el.getBoundingClientRect) {
                var r = el.getBoundingClientRect();
                if (r.width > 4 && r.height > 4 && r.bottom > 0 && r.top < innerHeight && r.right > 0 && r.left < innerWidth) return el;
              }
            }
            return null;
          }
          function closest(el, selector) {
            if (!el || !el.closest) return null;
            return el.closest(selector);
          }
          function isHidden(el) {
            for (var node = el; node && node.nodeType === 1; node = node.parentElement) {
              if (node.hidden || (node.getAttribute && node.getAttribute('aria-hidden') === 'true')) return true;
              var style = null;
              try { style = window.getComputedStyle(node); } catch (e) {}
              if (style && (style.display === 'none' || style.visibility === 'hidden' || parseFloat(style.opacity || '1') <= 0.01)) return true;
            }
            return false;
          }

          function q(el, sel) { try { return el.querySelector(sel); } catch (e) { return null; } }

          var P = { POINTER: 1, KEYBOARD: 2, CONTROLLER: 3, SYSTEM: 4 };
          var lastSpoken = "", lastSource = 0, lastSpeakTs = 0, lastControllerTs = 0, speakTimer = null;


          function hasNonAscii(s) {
            for (var i = 0; i < s.length; i++) {
              if (s.charCodeAt(i) > 127) return true;
            }
            return false;
          }
          function charDump(s) {
            var parts = [];
            for (var i = 0; i < s.length; i++) {
              var code = s.charCodeAt(i);
              if (code > 127) parts.push('U+' + ('0000' + code.toString(16).toUpperCase()).slice(-4));
              else parts.push(s[i]);
            }
            return parts.join('');
          }

          // ---------- HELD-NAVIGATION COALESCING ----------
          // lastTs is the time of the most recent navigation-ish signal, from either
          // a speech request or a UINavigation press. ema tracks the repeat cadence.
          // held names the direction actions the engine currently reports as down,
          // which lets a release flush the pending announcement with no wait at all.
          var _navBurst = { lastTs: 0, ema: 0, active: false, text: '', src: 0, timer: null, held: Object.create(null) };

          function navBurstReset() {
            if (_navBurst.timer) { try { clearTimeout(_navBurst.timer); } catch (e) {} }
            _navBurst.timer = null;
            _navBurst.active = false;
            _navBurst.ema = 0;
            _navBurst.text = '';
            _navBurst.src = 0;
            _navBurst.held = Object.create(null);
          }
          onCleanup(navBurstReset);

          function navBurstHeldCount() {
            var n = 0;
            for (var k in _navBurst.held) { if (_navBurst.held[k]) n++; }
            return n;
          }

          // The body of the original speak timer, shared by the plain debounce path
          // and the burst settle path.
          //
          // A tab announcement opens a short window in which the NEXT navigation
          // utterance queues behind it instead of cutting it off (see the tab
          // tracking block). It is armed here rather than at the call site because
          // the tab name goes out through the same DEBOUNCE_MS timer as everything
          // else -- arming it any earlier and the tab name queues behind itself.
          function emitSpeak(txt, src) {
            var interrupt = true;
            if (_vueTabQueueText && txt === _vueTabQueueText) {
              _vueTabQueueText = '';
              _vueTabQueueUntil = nowTS() + VUE_TAB_QUEUE_MS;
            } else if (nowTS() < _vueTabQueueUntil && navBurstIsNavSource(src)) {
              // One utterance only: the landing item. Anything after it is a new
              // movement and interrupts normally, or a held sweep would stack up
              // behind a tab name nobody is waiting on any more.
              interrupt = false;
              _vueTabQueueUntil = 0;
            }
            lastSpoken = txt;
            lastSource = src;
            lastSpeakTs = nowTS();
            send({ type: "speak", text: txt, interrupt: interrupt });
            if (isDebug()) log("info", "speak(" + src + (interrupt ? "" : ",queued") + "): " + txt);
          }

          function navBurstFlush() {
            if (_navBurst.timer) { try { clearTimeout(_navBurst.timer); } catch (e) {} }
            _navBurst.timer = null;
            _navBurst.active = false;
            var txt = _navBurst.text, src = _navBurst.src;
            _navBurst.text = '';
            _navBurst.src = 0;
            if (!txt || loadingActive) return;
            // The run may have ended on the same item the leading edge announced.
            if (txt === lastSpoken && src <= lastSource && (nowTS() - lastSpeakTs) < 400) return;
            emitSpeak(txt, src);
          }

          function navSettleDelay() {
            var d = _navBurst.ema * NAV_SETTLE_FACTOR;
            if (!(d > NAV_SETTLE_MIN_MS)) return NAV_SETTLE_MIN_MS;
            return d > NAV_SETTLE_MAX_MS ? NAV_SETTLE_MAX_MS : d;
          }

          function navBurstArm(delay) {
            if (_navBurst.timer) { try { clearTimeout(_navBurst.timer); } catch (e) {} }
            _navBurst.timer = trackedSetTimeout(navBurstOnSettle, delay);
          }

          // Where the engine reports key state, an uneven repeat must not be
          // mistaken for the end of the sweep: while a direction is still down we
          // re-arm instead of announcing, up to NAV_HOLD_STALE_MS since the last
          // actual movement.
          function navBurstOnSettle() {
            _navBurst.timer = null;
            if (navBurstHeldCount() > 0 && (nowTS() - _navBurst.lastTs) < NAV_HOLD_STALE_MS) {
              navBurstArm(navSettleDelay());
              return;
            }
            navBurstFlush();
          }

          // A request dropped by the same-text dedup below is still a movement, and
          // during a burst it means the sweep has come back to the item that was
          // already announced. Drop the pending text: keeping the older one would
          // announce the wrong landing spot once the run settles.
          // Only keyboard and controller focus moves are navigation. P.SYSTEM covers
          // toasts, damage and cruise-control callouts, camera changes and download
          // notices -- none of which is "held", and all of which were being delayed
          // by the settle or, worse, dropped outright when the next focus move
          // overwrote _navBurst.text mid-sweep.
          function navBurstIsNavSource(src) {
            return src === P.KEYBOARD || src === P.CONTROLLER;
          }

          function navBurstNoteRedundant(txt, src, t) {
            if (!navHoldSuppression || !navBurstIsNavSource(src)) return;
            _navBurst.lastTs = t;
            if (!_navBurst.active) return;
            _navBurst.text = '';
            _navBurst.src = 0;
          }

          // Returns true when the request was swallowed into an in-progress burst.
          function navBurstCapture(txt, src, t) {
            if (!navHoldSuppression) return false;
            // Pointer/hover speech is throttled on its own and is never "held";
            // system notifications are not navigation at all.
            if (!navBurstIsNavSource(src)) return false;
            var dt = t - _navBurst.lastTs;
            _navBurst.lastTs = t;
            // Leading edge is decided purely on cadence, never on the held set: the
            // first item of a hold must still speak, and a single tap must not be
            // captured just because the key happened to still be down.
            if (!_navBurst.active && dt > NAV_BURST_GAP_MS) {
              _navBurst.ema = 0;
              return false;
            }
            _navBurst.active = true;
            _navBurst.ema = _navBurst.ema ? (_navBurst.ema * 0.6 + dt * 0.4) : dt;
            _navBurst.text = txt;
            _navBurst.src = src;
            // Deliberately leave speakTimer alone. Anything pending there is the
            // leading edge of this very burst, which is the one item a hold is
            // supposed to announce; cancelling it would swallow the first item.
            navBurstArm(navSettleDelay());
            if (isDebug()) log('info', '[NAVBURST] held "' + txt + '" ema=' + Math.round(_navBurst.ema) + ' settle=' + Math.round(navSettleDelay()));
            return true;
          }

          function scheduleSpeak(txt, src) {
            if (!txt) return;
            if (loadingActive) return;
            if (looksLikeCss(txt)) return;
            if (isDebug() && hasNonAscii(txt)) {
              log('info', '[GLYPH] "' + txt + '" chars=' + charDump(txt));
            }
            var t = nowTS();
            if (src === P.CONTROLLER) lastControllerTs = t;
            if (src === P.POINTER && (t - lastControllerTs) < CONTROLLER_DOMINANCE_MS) return;
            if (txt === lastSpoken && src <= lastSource && (t - lastSpeakTs) < 400) {
              navBurstNoteRedundant(txt, src, t);
              return;
            }
            if (navBurstCapture(txt, src, t)) return;
            if (speakTimer) try { clearTimeout(speakTimer); } catch (e) {}
            speakTimer = trackedSetTimeout(function () {
              emitSpeak(txt, src);
            }, DEBOUNCE_MS);
          }

          function locateScope() {
            var md = firstVisible(".md-select-menu-container.md-active"); if (md) return md;
            var dlg = firstVisible("[role='dialog'], md-dialog-container, .md-dialog-container, .modal, .modal-content, [class*='dialog'], [class*='popup']"); if (dlg) return dlg;
            var parts = firstVisible(".contentNavVehicleconfig, #contentNavVehicleconfig"); if (parts) return parts;
            return document;
          }
          function getInteractiveAncestor(el) {
            return closest(el, "input,select,textarea,button,a[href],[role='option'],[role='menuitem'],[role='treeitem'],[role='tab'],[role='button'],[role='checkbox'],[role='switch'],[role='slider'],md-option,md-tab-item,[bng-nav-item],.bng-row,.dropdown-option,.pause-button,.pause-menu-button,.pause-menu-tile,.category-button,[tabindex]");
          }

          // Hover fires for whatever the cursor happens to cross, including
          // full-panel layout containers. Passing those straight to extractText
          // announced the entire panel ("System Freeroam Vehicle Environment
          // 03:14 PM <user> One moment...", "Loading... Loading... Loading...").
          // Resolving to an interactive ancestor is not enough on its own: in
          // 0.39 the full-screen .menu-screen wrapper carries bng-nav-item, and
          // .bng-tabs-root carries tabindex="-1", so both satisfy
          // getInteractiveAncestor. Reject anything that aggregates text from
          // more than a shallow subtree of markup.
          //
          // Counting only direct children with text does NOT work: a panel that
          // nests everything under one wrapper div presents a single texty child
          // and passes. Count total descendant elements instead, which is
          // insensitive to nesting depth, and cap the spoken length -- a control
          // label is short, a panel dump is not.
          // Calibrated against a 0.39 DOM dump: a switch row has 5 descendant
          // elements, a slider row 8-11, with labels of 11-16 chars. Panels have
          // hundreds of descendants, and the loading screen produced a 77-char
          // "Loading... Loading... ..." string from only ~7, so both limits earn
          // their keep -- neither alone catches every observed case.
          var HOVER_MAX_DESCENDANTS = 16;
          var HOVER_MAX_CHARS = 70;
          function hoverAnnouncementText(el) {
            var target = getInteractiveAncestor(el);
            if (!target) return "";
            if (target.getAttribute && target.getAttribute("tabindex") === "-1") return "";
            if (target.querySelectorAll && target.querySelectorAll("*").length > HOVER_MAX_DESCENDANTS) return "";
            var text = extractText(target);
            if (text.length > HOVER_MAX_CHARS) return "";
            return text;
          }

          function extractText(el) {
            if (!el) return "";
            if (isHidden(el) || closest(el, '.loading-screen, .loadingBackground')) return "";
            // Known Vue buttons that only show a hotkey glyph
            if (el.matches && el.matches('button.pause-button')) return 'Pause';
            var targetElement = el.querySelector('[bng-translate], [ng-bind]') || el;
            var rawText = (targetElement.innerText || targetElement.textContent || "").trim();
            // Diagnose short results: log the element's context so we can improve extraction
            if (isDebug()) {
              var cleaned0 = cleanText(rawText);
              if (cleaned0.length > 0 && cleaned0.length <= 2) {
                var parent = el.parentElement;
                var parentText = parent ? cleanText(parent.innerText || '') : '';
                log('info', '[SHORTTEXT] "' + cleaned0 + '" tag=' + el.tagName + ' class=' + (el.className || '').toString().substring(0, 80) + ' parentTag=' + (parent ? parent.tagName : 'none') + ' parentText="' + (parentText || '').substring(0, 120) + '" outerHTML=' + el.outerHTML.substring(0, 300));
              }
            }
            if (rawText.startsWith('{{') && rawText.endsWith('}}')) {
              try {
                var scope = angular.element(targetElement).scope();
                if (scope && !scope.$$phase && !scope.$root.$$phase) {
                  var expression = rawText.substring(2, rawText.length - 2).trim();
                  var evaluatedText = scope.$eval(expression);
                  if (evaluatedText && typeof evaluatedText === 'string') {
                    var cleaned = cleanText(evaluatedText);
                    if (cleaned) return cleaned;
                  }
                }
              } catch (e) { /* Fall through */ }
            }
            var inter = getInteractiveAncestor(el) || el;
            var mdSel = closest(inter, "md-select, .md-select-menu-container");
            var appHost = closest(inter, '.ui-app-host');
            if (appHost && !getInteractiveAncestor(el)) return "";
            if (mdSel) {
            var downloadRow = closest(inter, '.download-item, .download-row, [data-download-id]');
            if (downloadRow) {
              var filenameNode = q(downloadRow, '.filename, .download-name, [data-filename]');
              var stateNode = q(downloadRow, '.state, .download-state, .status');
              var filename = cleanText(
                downloadRow.getAttribute('data-filename') ||
                (filenameNode && (filenameNode.innerText || filenameNode.textContent)) || ''
              );
              var downloadState = cleanText(
                (stateNode && (stateNode.innerText || stateNode.textContent)) ||
                downloadRow.getAttribute('data-state') || ''
              );
              if (filename) return [filename, downloadState].filter(Boolean).join(', ');
            }
              var scope = locateScope();
              var isMd = scope !== document && (scope.matches ? scope.matches(".md-select-menu-container") : false);
              if (isMd) {
                var opt = q(scope, "md-option[aria-selected='true'], md-option.md-selected, md-option._md-focused, md-option.md-active");
                var t = cleanText((opt && (opt.getAttribute('aria-label') || opt.title || opt.alt)) || (opt && (opt.innerText || opt.textContent)) || "");
                if (t) return t;
              }
              var label = q(mdSel.tagName && mdSel.tagName.toLowerCase() === "md-select" ? mdSel : inter, "md-select-label");
              if (label) {
                var s1 = cleanText(label.innerText || label.textContent || ""); if (s1 && s1.length >= MIN_CHARS) return s1;
              }
            }
            var attr = (inter.getAttribute && (inter.getAttribute("aria-label") || inter.title || inter.alt || inter.getAttribute("data-name") || inter.getAttribute("data-label"))) || "";
            if (attr) { var s2 = cleanText(attr); if (s2) return s2; }
            var t2 = cleanText((inter.innerText || inter.textContent || "")); if (t2 && t2.length >= MIN_CHARS && t2.length <= MAX_LEN) return t2;
            return "";
          }

          // ---------- TRANSLATION HELPER ----------
          // Shared by the message processor and the bindings text formatters.
          var _translate = null;

          // Last resort only, when no translator is reachable at all. Speaking the
          // final dot-segment verbatim is how "ui.common.vehicleAccessoryOn" was
          // announced as "vehicleAccessoryOn" and, worse, how state keys such as
          // "vehicle.engine.oilLevelCritical.true" collapsed to the single word
          // "true". Keep a trailing boolean attached to the concept it qualifies
          // instead of dropping it -- dropping ".false" would invert the meaning.
          function humanizeTranslationKey(key) {
            if (typeof key !== 'string' || !key) return key;
            var parts = key.split('.');
            var leaf = parts.pop() || key;
            if (/^(true|false)$/i.test(leaf) && parts.length) leaf = parts.pop() + ' ' + leaf.toLowerCase();
            return leaf
              .replace(/_/g, ' ')
              .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
              .replace(/\s+/g, ' ')
              .trim();
          }

          // 0.39 publishes the petite-vue-i18n instance as window.vueI18n
          // (ui-vue/src/services/translation.js) and the legacy AngularJS
          // $translate service as window.angular$translate (entrypoints/main/main.js).
          // bngApi.engine.translate does not exist in 0.39 at all, and the injector
          // probe only ever resolves on the remaining AngularJS screens -- so on
          // every Vue screen lookups fell straight through to the humanizing
          // fallback above and spoke raw key fragments.
          function findTranslateFunc() {
            if (_translate) return _translate;
            if (window.vueI18n && window.vueI18n.global && typeof window.vueI18n.global.t === 'function') {
              // Re-resolved per call rather than bound once: initTranslation()
              // replaces window.vueI18n outright when its variant check fails.
              _translate = function (key, ctx) {
                var g = window.vueI18n && window.vueI18n.global;
                if (!g || typeof g.t !== 'function') return key;
                try { return ctx ? g.t(key, ctx) : g.t(key); } catch (e) { return key; }
              };
              return _translate;
            }
            if (window.angular$translate && typeof window.angular$translate.instant === 'function') {
              _translate = function (key, ctx) {
                try { return window.angular$translate.instant(key, ctx); } catch (e) { return key; }
              };
              return _translate;
            }
            try {
              var inj = angular.element(document.body).injector();
              if (inj) {
                var $translate = inj.get('$translate');
                if ($translate && typeof $translate.instant === 'function') {
                  _translate = function(key, ctx) { return $translate.instant(key, ctx); };
                  return _translate;
                }
              }
            } catch (e) {}
            // Deliberately not cached: the real translator appears once the UI
            // finishes booting, and caching this would freeze us on key fragments
            // for the rest of the session.
            return humanizeTranslationKey;
          }

          // ---------- TOASTER SERVICE PATCHER ----------
          var toasterInterval;
          var toasterPatched = false;
          function attachToasterPatcher() {
            if (toasterPatched) return;
            try {
              var injector = angular.element(document.body).injector();
              if (injector) {
                var toasterService = injector.get('MessageToasterService');
                if (toasterService && typeof toasterService.add === 'function' && !toasterService.add.__bnvdaPatched) {
                  var originalAdd = toasterService.add;
                  toasterService.add = function(message) {
                    if (message && message.message) { processAndSpeakMessage(message.message); }
                    return originalAdd.apply(this, arguments);
                  };
                  toasterService.add.__bnvdaPatched = true;
                  onCleanup(function () {
                    if (toasterService.add && toasterService.add.__bnvdaPatched) toasterService.add = originalAdd;
                  });
                  toasterPatched = true;
                  clearInterval(toasterInterval);
                  log('info', '[bnvda] MessageToasterService patched successfully.');
                }
              }
            } catch (e) { /* Fails silently */ }
          }

          // ---------- CENTRALIZED MESSAGE PROCESSOR ----------
          var lastCameraSwitchTs = 0;

          // Every game message carries a category (guihooks.message's third
          // argument). BeamNG keys its on-screen message list by that category and
          // REPLACES the entry each time rather than appending -- see
          // services/messagesStore.js -- so a message the vehicle re-fires every
          // frame just sits there quietly for a sighted player. Speech has no
          // equivalent of "still showing": without the same grouping, every
          // refresh is a fresh interruption. The stalled-engine message re-fires
          // about twice a second for as long as the ignition is on with the engine
          // off, which talks over everything else.
          //
          // So mirror the game's own model. Say a category once, stay silent while
          // it keeps being refreshed, and allow it again only once it has been
          // absent long enough that the on-screen message would have expired.
          var MESSAGE_REPEAT_GRACE_MS = 1500;
          var MESSAGE_STATE_LIMIT = 64;
          var _messageState = Object.create(null);

          // Categories never worth speaking, whatever wording the game picks for
          // them. Blocking by category covers every variant of a situation in one
          // entry -- the stall message has three, chosen by gearbox mode.
          var BLOCKED_MESSAGE_CATEGORIES = [
            'vehicle.engine.isStalling'
          ];
          // Same intent, for messages that arrive without a useful category.
          var BLOCKED_TRANSLATION_KEYS = [
            'vehicle.vehicleController.stalled',
            'vehicle.vehicleController.stalledAutoClutch',
            'vehicle.vehicleController.stalledStarting'
          ];

          // A message is either a bare translation key or a { txt, context }
          // descriptor. The realistic-mode stall message uses the bare string form,
          // which is why testing only payload.txt never blocked it.
          function messageTranslationKey(payload) {
            if (typeof payload === 'string') return payload;
            if (payload && typeof payload === 'object' && typeof payload.txt === 'string') return payload.txt;
            return '';
          }

          // How long the game would keep this message on screen. Matches
          // messagesStore.js: ttlMs wins, else ttl in seconds, else its 5 s default.
          function messageWindowMs(meta) {
            var ttlMs = typeof meta.ttlMs === 'number' ? meta.ttlMs
              : (typeof meta.ttl === 'number' ? meta.ttl * 1000 : 5000);
            if (!(ttlMs > 0)) ttlMs = 5000;
            return ttlMs + MESSAGE_REPEAT_GRACE_MS;
          }

          function messageIsRepeat(category, text, meta) {
            // Uncategorised messages fall back to their own text, so repeat
            // suppression still applies without lumping unrelated ones together.
            var key = category || ('\u0001text:' + text);
            var now = nowTS();
            var entry = _messageState[key];
            if (entry && entry.text === text && (now - entry.lastSeen) <= entry.window) {
              // Still on screen as far as the game is concerned. Note that we saw
              // it, so a message refreshed forever stays suppressed forever.
              entry.lastSeen = now;
              return true;
            }
            var keys = Object.keys(_messageState);
            if (keys.length > MESSAGE_STATE_LIMIT) {
              for (var i = 0; i < keys.length; i++) {
                var stale = _messageState[keys[i]];
                if ((now - stale.lastSeen) > stale.window) delete _messageState[keys[i]];
              }
            }
            _messageState[key] = { text: text, lastSeen: now, window: messageWindowMs(meta) };
            return false;
          }

          onCleanup(function () { _messageState = Object.create(null); });

          function processAndSpeakMessage(payload, meta) {
            meta = meta || {};
            var category = typeof meta.category === 'string' ? meta.category : '';
            if (category && BLOCKED_MESSAGE_CATEGORIES.indexOf(category) !== -1) return;
            if (BLOCKED_TRANSLATION_KEYS.indexOf(messageTranslationKey(payload)) !== -1) return;
            var finalText = '';
            if (typeof payload === 'string') {
              // Try translating dot-delimited keys (e.g. "vehicle.engine.oilLevelCritical.true")
              if (/^[\w]+\.[\w.]+$/.test(payload)) {
                var translator = findTranslateFunc();
                var translated = speechValue(translator(payload), 'Message.translation');
                if (translated && translated !== payload) {
                  finalText = translated;
                } else {
                  finalText = stripHtml(payload);
                }
              } else {
                finalText = stripHtml(payload);
              }
            }
            else if (typeof payload === 'object' && payload !== null && payload.txt) {
              if (isDebug()) {
                // Informational, not a fault: {txt, context} is the normal shape
                // for a localization descriptor and is handled below.
                try { log('info', '[MESSAGE_DESCRIPTOR] ' + JSON.stringify(payload)); } catch (e) {}
              }
              var translator = findTranslateFunc();
              // Pre-translate context values that are translation keys (e.g. "ui.xxx")
              // and provide both "key" and "key | translate" variants so angular-translate's
              // literal interpolation of {{key | translate}} patterns works correctly.
              var ctx = {};
              if (payload.context && typeof payload.context === 'object') {
                for (var k in payload.context) {
                  var v = payload.context[k];
                  var translated = (typeof v === 'string' && v.indexOf('ui.') === 0) ? translator(v) : v;
                  ctx[k] = translated;
                  ctx[k + ' | translate'] = translated;
                }
              }
              finalText = speechValue(translator(payload.txt, ctx), 'Message.descriptor.translation');
            }
            else if (typeof payload === 'object' && payload !== null) {
              finalText = speechValue(payload, 'Message.payload');
              if (finalText === null) return;
            }
            else { log('warn', '[UNSPEAKABLE] unexpected payload type (' + typeof payload + '): ' + String(payload)); return; }
            if (finalText === null) return;
            // Follow the device most recently used, like BngBinding does.
            finalText = finalText.replace(/\[action=([^\]]+)\]/g, function(match, actionName) {
              return formatActionBinding(actionName);
            });
            finalText = cleanText(finalText);
            if (finalText.toLowerCase() === 'switched' && (nowTS() - lastCameraSwitchTs) < 250) { return; }
            if (finalText && finalText.length >= MIN_CHARS) {
              if (messageIsRepeat(category, finalText, meta)) return;
              scheduleSpeak(finalText, P.SYSTEM);
            }
            else if (payload && !finalText) { log('warn', '[UNSPEAKABLE] empty after processing: ' + JSON.stringify(payload)); }
          }


          // ---------- DOWNLOAD MILESTONES ----------
          var observedDownloads = new Set();
          var completedDownloads = new Set();
          var repositoryErrors = new Set();

          function downloadField(item, names) {
            if (!item || typeof item !== 'object') return '';
            for (var i = 0; i < names.length; i++) {
              if (item[names[i]] !== undefined && item[names[i]] !== null) return cleanText(item[names[i]]);
            }
            return '';
          }

          function downloadIdentity(item) {
            return downloadField(item, ['id', 'downloadId', 'uri', 'url', 'filename', 'fileName', 'name']);
          }

          function downloadFilename(item) {
            var name = downloadField(item, ['filename', 'fileName', 'name', 'title', 'uri', 'url']);
            return name.replace(/^.*[\\/]/, '').replace(/[?#].*$/, '');
          }

          subscribe($rootScope, 'downloadStatesChanged', function (_event, payload) {
            var states = payload && (payload.states || payload.downloads || payload);
            var items = Array.isArray(states) ? states : Object.keys(states || {}).map(function (key) { return states[key]; });
            for (var i = 0; i < items.length; i++) {
              var item = items[i];
              var identity = downloadIdentity(item);
              if (!identity || observedDownloads.has(identity)) continue;
              observedDownloads.add(identity);
              var filename = downloadFilename(item) || 'download';
              scheduleSpeak('Downloading ' + filename, P.SYSTEM);
            }
          });

          subscribe($rootScope, 'downloadStateChanged', function (_event, item) {
            var state = downloadField(item, ['state', 'status']).toLowerCase();
            if (!/^(complete|completed|finished|success|succeeded)$/.test(state)) return;
            var identity = downloadIdentity(item) || downloadFilename(item);
            if (!identity || completedDownloads.has(identity)) return;
            completedDownloads.add(identity);
            scheduleSpeak((downloadFilename(item) || 'Download') + ' complete', P.SYSTEM);
          });

          subscribe($rootScope, 'repoError', function (_event, payload) {
            var message = cleanText(downloadField(payload, ['message', 'msg', 'error', 'reason']) || payload || 'Repository download failed');
            if (repositoryErrors.has(message)) return;
            repositoryErrors.add(message);
            scheduleSpeak(message, P.SYSTEM);
          });

          onCleanup(function () {
            observedDownloads.clear();
            completedDownloads.clear();
            repositoryErrors.clear();
          });
          // ---------- GLOBAL EVENT LISTENERS ----------
          subscribe($rootScope, 'Message', function (event, args) {
            if (!args || !args.msg) return;
            try { var injector = angular.element(document.body).injector(); if (injector) { var toasterService = injector.get('MessageToasterService'); if (toasterService.activeCategories.includes(args.category)) return false; } } catch (e) {}
            processAndSpeakMessage(args.msg, args);
          });
          subscribe($rootScope, 'DamageMessage', function (event, args) {
            if (!args || !args.damageText) return;
            var translator = findTranslateFunc();
            var damageText = speechValue(args.damageText, 'DamageMessage.damageText');
            if (damageText === null) return;
            var translatedText = speechValue(translator(damageText), 'DamageMessage.translation');
            if (translatedText && translatedText.length >= MIN_CHARS) { scheduleSpeak(cleanText(translatedText), P.SYSTEM); }
          });
          subscribe($rootScope, 'onCameraNameChanged', function (event, data) {
            if (data && data.name) {
              if (suppressNextCameraEvent) {
                suppressNextCameraEvent = false;
                return;
              }
              lastCameraSwitchTs = nowTS();
              var cameraName = data.name.charAt(0).toUpperCase() + data.name.slice(1);
              var message = 'Camera: ' + cameraName;
              scheduleSpeak(message, P.SYSTEM);
            }
          });

          // ========== CRUISE CONTROL SPEECH ==========
          var _ccUnitMultiplier = 2.23694; // default imperial (m/s to mph)
          var _ccUnitLabel = 'mph';
          var _ccWasEnabled = false;
          var _ccLastSpokenSpeed = 0;

          subscribe($rootScope, 'SettingsChanged', function (event, data) {
            if (data && data.values && data.values.uiUnitLength) {
              if (data.values.uiUnitLength === 'metric') {
                _ccUnitMultiplier = 3.6;
                _ccUnitLabel = 'km/h';
              } else {
                _ccUnitMultiplier = 2.23694;
                _ccUnitLabel = 'mph';
              }
            }
          });

          subscribe($rootScope, 'CruiseControlState', function (event, data) {
            if (!data) return;
            var isEnabled = !!data.isEnabled;
            var speedDisplay = Math.round(data.targetSpeed * _ccUnitMultiplier);

            if (isEnabled && !_ccWasEnabled) {
              // Just turned on
              scheduleSpeak('Cruise control set, ' + speedDisplay + ' ' + _ccUnitLabel, P.SYSTEM);
              _ccLastSpokenSpeed = speedDisplay;
            } else if (!isEnabled && _ccWasEnabled) {
              // Just turned off
              scheduleSpeak('Cruise control cancelled', P.SYSTEM);
              _ccLastSpokenSpeed = 0;
            } else if (isEnabled && speedDisplay !== _ccLastSpokenSpeed) {
              // Speed changed while active
              scheduleSpeak(speedDisplay + ' ' + _ccUnitLabel, P.SYSTEM);
              _ccLastSpokenSpeed = speedDisplay;
            }

            _ccWasEnabled = isEnabled;
          });

          // Request initial settings so we have the correct unit
          try { bngApi.engineLua('settings.notifyUI()'); } catch (e) {}

          // ========== GENERIC UI CONTROL HANDLERS ==========
          function speakCheckboxRow(focusedElement, src) {
            // Prefer the md-checkbox that encloses the focused element directly,
            // so rows with multiple checkboxes / nested layouts read correctly.
            var checkboxEl = closest(focusedElement, 'md-checkbox');
            var row = closest(focusedElement, 'md-list-item');
            if (!checkboxEl && row) checkboxEl = row.querySelector('md-checkbox');
            if (!checkboxEl) return false;
            if (!row) {
              // Angular controls outside the Vue pause/options screens only.
              return false;
            }
            var parts = [];
            var labelText = '';
            var labelEl = row.querySelector('p');
            if (labelEl) labelText = cleanText(labelEl.innerText);
            if (!labelText) {
              var altLabel = row.querySelector('span[flex], label, [bng-translate]');
              if (altLabel) labelText = cleanText(altLabel.innerText);
            }
            if (!labelText) {
              // md-checkbox itself may contain inline text
              var cbText = cleanText(checkboxEl.innerText);
              if (cbText) labelText = cbText;
            }
            if (labelText) parts.push(labelText);
            var isChecked = checkboxEl.classList.contains('md-checked');
            parts.push(isChecked ? 'checked' : 'unchecked');
            var finalText = parts.join(', ');
            scheduleSpeak(finalText, src);
            return true;
          }

          function speakSliderRow(focusedElement, src) {
            var row = closest(focusedElement, 'md-list-item');
            if (!row) return false;
            // Prefer the slider that contains / is adjacent to the focused element.
            var sliderEl = closest(focusedElement, 'md-slider');
            if (!sliderEl) sliderEl = row.querySelector('md-slider');
            if (!sliderEl) return false;
            var parts = [];
            var labelText = '';

            // Label search: preceding sibling of the slider first (handles <span flex>),
            // then first <p>/<label>/<span flex> in the row.
            if (sliderEl.previousElementSibling) {
              var sib = sliderEl.previousElementSibling;
              if (sib.matches && sib.matches('p, label, span[flex]')) {
                labelText = cleanText(sib.innerText);
              }
            }
            if (!labelText) {
              var labelEl = row.querySelector('p, label, span[flex]');
              if (labelEl) labelText = cleanText(labelEl.innerText);
            }
            if (labelText) parts.push(labelText);

            // Value: prefer a sibling directly after the focused slider
            var valueText = '';
            var vdisp = sliderEl.nextElementSibling;
            if (vdisp && vdisp.tagName) {
              var vtn = vdisp.tagName.toLowerCase();
              if (vtn === 'span' || vtn === 'input') {
                valueText = vtn === 'input' ? vdisp.value : cleanText(vdisp.innerText);
              }
            }
            if (!valueText) {
              var valueEl = row.querySelector('input[type="number"], span.md-body-1');
              if (valueEl) {
                valueText = valueEl.tagName.toLowerCase() === 'input' ? valueEl.value : cleanText(valueEl.innerText);
              }
            }
            if (!valueText) {
              var ariaValue = sliderEl.getAttribute('aria-valuenow');
              if (ariaValue) valueText = cleanText(ariaValue);
            }
            if (valueText) parts.push(valueText);

            if (parts.length >= 2) {
              var finalText = parts.join(', ');
              scheduleSpeak(finalText, src);
              return true;
            }
            return false;
          }

          // ========== Specialized handler for Vehicle Tuning Sliders (Vue.js version) ==========
          // Timer for the deferred tuning hint so navigating to a new control
          // cancels a still-pending description from the previous one.
          var _tuningHintTimer = null;
          // Last spoken section grouping, so a category/subcategory heading
          // (e.g. "Tires", "Front") is announced only the first time focus
          // enters it and not repeated until focus moves into a different one.
          var _lastTuningCategory = null;
          var _lastTuningSubcategory = null;
          function speakTuningControl(focusedElement, src) {
            var row = closest(focusedElement, '.input-container');
            if (!row) return false;
            var titleEl = row.querySelector('.variable-title');
            var valueInput = row.querySelector('input[data-testid="input"]');
            var unitEl = row.querySelector('[data-testid="suffix"]');
            if (titleEl && valueInput && unitEl) {
              var name = cleanText(titleEl.innerText);
              var value = valueInput.value;
              var unit = cleanText(unitEl.innerText);
              var finalText = name + ", " + value + " " + unit;
              // Announce the enclosing section grouping when it changes. The
              // subcategory heading is omitted by the game for the "Other" bucket,
              // so subName is simply empty there and nothing extra is read.
              var catEl = closest(row, '.tuning-category');
              var catNameEl = catEl ? catEl.querySelector('.category-name') : null;
              var catName = catNameEl ? cleanText(catNameEl.innerText) : '';
              var subEl = closest(row, '.tuning-subcategory');
              var subNameEl = subEl ? subEl.querySelector('.subcategory-name') : null;
              var subName = subNameEl ? cleanText(subNameEl.innerText) : '';
              var catChanged = catName !== _lastTuningCategory;
              var subChanged = subName !== _lastTuningSubcategory;
              _lastTuningCategory = catName;
              _lastTuningSubcategory = subName;
              var groupParts = [];
              if (catChanged && catName) groupParts.push(catName);
              if ((catChanged || subChanged) && subName) groupParts.push(subName);
              if (groupParts.length) finalText = groupParts.join(', ') + '. ' + finalText;
              scheduleSpeak(finalText, src);
              // Speak the tuning hint (the in-game tooltip that describes what the
              // control does) shortly after, so the name and value are heard first.
              // BeamNG's v-bng-tooltip directive stashes the description on the
              // .input-container element itself as el.__bngTooltip.text — always
              // present, regardless of whether the floating tooltip is rendered.
              if (_tuningHintTimer) { try { clearTimeout(_tuningHintTimer); } catch (e) {} _tuningHintTimer = null; }
              var tip = row.__bngTooltip;
              var hint = (tip && tip.text) ? cleanText(String(tip.text)) : '';
              if (hint && hint.toLowerCase() !== name.toLowerCase()) {
                // Queue the hint after the control speech instead of interrupting
                // it (interrupt:false). This makes ordering independent of the
                // user's speech rate — on slow speech the value is no longer cut
                // off; the hint simply plays once the control finishes. The short
                // delay only ensures this message is dispatched after the control's
                // debounced send, so it lands behind it in the speech queue.
                _tuningHintTimer = trackedSetTimeout(function () {
                  _tuningHintTimer = null;
                  send({ type: "speak", text: hint, interrupt: false });
                }, 250);
              }
              return true;
            }
            return false;
          }

          // ========== Handler for menu accordion items (e.g. radial menu config) ==========
          function speakMenuAccordionItem(focusedElement, src) {
            if (!focusedElement.classList.contains('menu-navigation')) return false;
            var row = closest(focusedElement, '.bng-accitem');
            if (!row) return false;
            var contentEl = row.querySelector('.bng-accitem-caption-content');
            var text = contentEl ? cleanText(contentEl.innerText) : cleanText(focusedElement.innerText);
            if (!text) return false;
            var parts = [text];
            var isExpandable = row.classList.contains('bng-accitem-expandable');
            if (isExpandable) {
              var isExpanded = row.classList.contains('bng-accitem-expanded');
              parts.push(isExpanded ? 'expanded' : 'collapsed');
            }
            scheduleSpeak(parts.join(', '), src);
            return true;
          }

          // ========== CONTROLS BINDINGS SCREEN MODULE ==========
          // Reverse glyph map: maps bngIcons glyph characters back to friendly names
          var ICON_FRIENDLY_NAMES = {
            // Xbox
            xboxA: 'A button', xboxB: 'B button', xboxX: 'X button', xboxY: 'Y button',
            xboxLB: 'Left bumper', xboxRB: 'Right bumper',
            xboxLT: 'Left trigger', xboxRT: 'Right trigger',
            xboxView: 'View button', xboxMenu: 'Menu button',
            xboxDDown: 'D-pad down', xboxDLeft: 'D-pad left', xboxDRight: 'D-pad right', xboxDUp: 'D-pad up',
            xboxLSButton: 'Left stick press', xboxRSButton: 'Right stick press',
            xboxXAxis: 'Left stick X', xboxYAxis: 'Left stick Y',
            xboxXRot: 'Right stick X', xboxYRot: 'Right stick Y',
            // PlayStation
            psCross: 'Cross', psSquare: 'Square', psCircle: 'Circle', psTriangle: 'Triangle',
            psL1: 'L1', psR1: 'R1', psL2: 'L2', psR2: 'R2',
            psCreate2: 'Create', psMenu2: 'Menu',
            psDDown: 'D-pad down', psDLeft: 'D-pad left', psDRight: 'D-pad right', psDUp: 'D-pad up',
            psL3Button: 'L3', psR3Button: 'R3',
            psLSX: 'Left stick X', psLSY: 'Left stick Y',
            psRSX: 'Right stick X', psRSY: 'Right stick Y',
            psTrackpadPressCenter: 'Trackpad press',
            // Mouse
            mouseLMB: 'Left click', mouseRMB: 'Right click', mouseMMB: 'Middle click',
            mouseXAxis: 'Mouse X', mouseYAxis: 'Mouse Y', mouseWheel: 'Mouse wheel'
          };

          // The icon NAME is the only thing that says which device family a glyph
          // belongs to. BngIcon renders a bare glyph character with no class,
          // attribute or data hook naming the icon, so once it is in the DOM this
          // reverse map is the only route back -- and controls.js names every
          // control icon by family prefix (xboxA, psCross, mouseLMB).
          function iconDeviceFamily(iconName) {
            if (iconName.indexOf('xbox') === 0) return 'xbox';
            if (iconName.indexOf('ps') === 0) return 'ps';
            if (iconName.indexOf('mouse') === 0) return 'pc_mouse';
            return '';
          }

          // Built lazily on first use: glyph character -> friendly name
          var _glyphToName = null;
          // ...and glyph character -> device family, built in the same pass.
          var _glyphToFamily = null;
          function buildGlyphMap() {
            _glyphToName = {};
            _glyphToFamily = {};
            try {
              var icons = iconCatalog || (window.bngVue && window.bngVue.icons);
              if (!icons) return;
              var keys = Object.keys(icons);
              for (var i = 0; i < keys.length; i++) {
                var iconName = keys[i];
                var friendly = ICON_FRIENDLY_NAMES[iconName];
                if (friendly && icons[iconName] && icons[iconName].glyph) {
                  _glyphToName[icons[iconName].glyph] = friendly;
                  var family = iconDeviceFamily(iconName);
                  if (family) _glyphToFamily[icons[iconName].glyph] = family;
                }
              }
              log('info', '[BINDING] Built glyph reverse map with ' + Object.keys(_glyphToName).length + ' entries');
            } catch (e) {
              log('info', '[BINDING] Failed to build glyph map: ' + e.message);
            }
          }

          var KEYBOARD_SYMBOLS = {
            '\u21E7': 'Shift', '\u2191': 'Up', '\u2193': 'Down', '\u2190': 'Left', '\u2192': 'Right',
            '\u2318': 'Cmd', '\u2325': 'Alt', '\u2303': 'Ctrl', '\u232B': 'Backspace',
            '\u21B5': 'Enter', '\u2423': 'Space', '\u21E5': 'Tab', '\u238B': 'Escape',
            'L\u21E7': 'Left Shift', 'R\u21E7': 'Right Shift',
            'L\u2303': 'Left Ctrl', 'R\u2303': 'Right Ctrl',
            'L\u2325': 'Left Alt', 'R\u2325': 'Right Alt'
          };

          function cleanKeyboardText(text) {
            if (!text) return '';
            // Try full-string match first (e.g. "L⇧" -> "Left Shift")
            var t = text.trim();
            if (KEYBOARD_SYMBOLS[t]) return KEYBOARD_SYMBOLS[t];
            // Replace individual symbols
            var result = t;
            var syms = Object.keys(KEYBOARD_SYMBOLS);
            for (var i = 0; i < syms.length; i++) {
              if (result.indexOf(syms[i]) !== -1) {
                result = result.replace(syms[i], KEYBOARD_SYMBOLS[syms[i]] + ' ');
              }
            }
            return result.replace(/\s+/g, ' ').trim();
          }

          function replaceKnownGlyphs(text) {
            if (!text) return '';
            if (!_glyphToName) buildGlyphMap();
            var result = String(text);
            if (_glyphToName) Object.keys(_glyphToName).forEach(function(glyph) {
              result = result.split(glyph).join(' ' + _glyphToName[glyph] + ' ');
            });
            result = cleanKeyboardText(result);
            return result.replace(/[\uE000-\uF8FF]/g, '').replace(/\s+/g, ' ').trim();
          }

          function friendlyControlName(value) {
            var raw = replaceKnownGlyphs(value);
            var known = {
              btn_a: 'A button', btn_b: 'B button', btn_x: 'X button', btn_y: 'Y button',
              btn_l: 'Left bumper', btn_r: 'Right bumper', triggerl: 'Left trigger', triggerr: 'Right trigger',
              btn_back: 'View button', btn_start: 'Menu button',
              upov: 'D-pad up', dpov: 'D-pad down', lpov: 'D-pad left', rpov: 'D-pad right'
            };
            var key = raw.toLowerCase();
            if (known[key]) return known[key];
            raw = raw.replace(/_/g, ' ');
            return raw.replace(/(^|[ +])([a-z])/g, function(_match, prefix, letter) {
              return prefix + letter.toUpperCase();
            });
          }

          function friendlyIconName(iconName) {
            if (!iconName) return '';
            if (ICON_FRIENDLY_NAMES[iconName]) return ICON_FRIENDLY_NAMES[iconName];
            var icon = (iconCatalog || (window.bngVue && window.bngVue.icons) || {})[iconName];
            if (icon && icon.glyph) {
              var byGlyph = replaceKnownGlyphs(icon.glyph);
              if (byGlyph) return byGlyph;
            }
            return '';
          }

          function viewerBindingText(value, seen) {
            if (value === null || value === undefined) return '';
            if (typeof value === 'string' || typeof value === 'number') return replaceKnownGlyphs(String(value));
            if (typeof value !== 'object') return '';
            seen = seen || [];
            if (seen.indexOf(value) !== -1) return '';
            seen.push(value);
            if (Array.isArray(value)) return value.map(function(item) { return viewerBindingText(item, seen); }).filter(Boolean).join(' + ');
            if (value.multiControls && value.multiControls.length) {
              return viewerBindingText(value.multiControls, seen);
            }
            if (value.special || value.ownIcon) {
              var iconText = friendlyIconName(value.ownIcon || value.icon);
              if (iconText) return iconText;
            }
            var preferred = ['bindingText', 'displayName', 'label', 'controlName', 'control', 'key', 'glyph'];
            for (var i = 0; i < preferred.length; i++) if (value[preferred[i]] !== undefined) {
              var text = (preferred[i] === 'control' || preferred[i] === 'key')
                ? friendlyControlName(String(value[preferred[i]]))
                : viewerBindingText(value[preferred[i]], seen);
              if (text && text.toLowerCase() !== 'title') return text;
            }
            var collections = ['value', 'parts', 'controls', 'bindings', 'binding', 'modifiers'];
            for (var j = 0; j < collections.length; j++) if (value[collections[j]] !== undefined) {
              var joined = viewerBindingText(value[collections[j]], seen);
              if (joined && joined.toLowerCase() !== 'title') return joined;
            }
            return '';
          }

          function translatedActionName(actionName) {
            var translator = findTranslateFunc();
            var candidates = ['ui.inputActions.vehicle.' + actionName + '.title', 'ui.inputActions.' + actionName + '.title'];
            for (var i = 0; i < candidates.length; i++) {
              var result = speechValue(translator(candidates[i]), 'Action.translation');
              if (result && result !== candidates[i] && result.toLowerCase() !== 'title') return cleanText(result);
            }
            var leaf = actionName.substring(actionName.lastIndexOf('.') + 1).replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/[_-]+/g, ' ');
            return leaf.charAt(0).toUpperCase() + leaf.slice(1);
          }

          function formatActionBinding(actionName) {
            try {
              if (Controls && typeof Controls.makeViewerObj === 'function') {
                var binding = viewerBindingText(Controls.makeViewerObj({ action: actionName, useLastDevice: true }));
                if (binding && binding.toLowerCase() !== 'title') return binding;
              }
            } catch (e) { log('info', '[BINDING] Unable to resolve ' + actionName + ': ' + e.message); }
            return translatedActionName(actionName);
          }

          // ---------- ACTIVE INPUT DEVICE ----------
          // BngBinding renders one .binding-container per variant, and with two
          // pads plugged in -- an Xbox pad and a DualSense, say -- that is the
          // same action twice, once per glyph set. A sighted player skims the
          // pair; a listener hears every hint and every prompt read out twice,
          // and half of each pair names buttons that are not on the pad in their
          // hands. Controls.lastDevices is the game's own most-recently-used
          // ordering (core_input_bindings.getRecentDevices), i.e. the very rule
          // BngBinding follows for its own useLastDevice lookups, so following it
          // here is what keeps the speech and the screen talking about one pad.
          var DEV_FAMILY_PREFIXES = [
            ['xinput', 'xbox'], ['ps5', 'ps'], ['ps4', 'ps'], ['sce', 'ps'],
            ['mouse', 'pc_mouse'],
            // Keyboards, wheels and generic joysticks have no glyph set of their
            // own -- controls.js's CONTROL_ICONS has no family for them, so
            // bngBinding falls back to a <kbd> carrying the control name. Their
            // containers therefore hold no glyph at all, which is the empty
            // family, and it must stay reachable rather than being treated as
            // "unknown device".
            ['keyboard', ''], ['wheel', ''], ['joystick', ''], ['gamepad', '']
          ];

          function deviceGlyphFamily(devName) {
            var name = String(devName || '').toLowerCase();
            for (var i = 0; i < DEV_FAMILY_PREFIXES.length; i++) {
              if (name.indexOf(DEV_FAMILY_PREFIXES[i][0]) === 0) return DEV_FAMILY_PREFIXES[i][1];
            }
            return '';
          }

          // Families in most-recently-used order, deduped. The glyphless family
          // is appended when it is not already there, so a binding that exists
          // only on the keyboard is still announced when a pad is the active
          // device -- the alternative is silence about a control that is on
          // screen.
          function activeDeviceFamilies() {
            var order = [];
            try {
              // The pinia store is the one that carries lastDevices; fall back
              // to it directly in case an older bootstrap handed us main.js's
              // method-only facade instead.
              var store = (Controls && Controls.lastDevices) ? Controls
                : (window.bngVue && window.bngVue.controlsStore) || null;
              var devices = store && store.lastDevices;
              if (devices && devices.length) {
                for (var i = 0; i < devices.length; i++) {
                  var family = deviceGlyphFamily(devices[i]);
                  if (order.indexOf(family) === -1) order.push(family);
                }
              }
            } catch (e) {
              log('info', '[BINDING] Controls.lastDevices unavailable: ' + e.message);
            }
            if (order.indexOf('') === -1) order.push('');
            return order;
          }

          function containerGlyphFamily(container) {
            if (!_glyphToFamily) buildGlyphMap();
            if (!_glyphToFamily) return '';
            var text = (container && container.textContent) || '';
            for (var i = 0; i < text.length; i++) {
              var ch = text[i];
              if (ch.charCodeAt(0) < 128) continue;
              if (ch.charCodeAt(0) >= 0xD800 && ch.charCodeAt(0) <= 0xDBFF && i + 1 < text.length) {
                ch = text[i] + text[i + 1];
                i++;
              }
              if (_glyphToFamily[ch]) return _glyphToFamily[ch];
            }
            return '';
          }

          // Keep only the variants belonging to the device in use. Variants that
          // share a family are genuine alternatives for one device -- an axis and
          // its inversion -- and all of those survive; it is only the per-device
          // duplicates that are dropped. Falling back to the first container
          // rather than to all of them matters: a family this map cannot name is
          // still one device's worth of binding, and reading the set would be the
          // same double announcement in a case nobody would think to look at.
          function pickBindingVariants(containers) {
            if (!containers || containers.length < 2) return containers || [];
            var families = containers.map(containerGlyphFamily);
            var order = activeDeviceFamilies();
            for (var i = 0; i < order.length; i++) {
              var family = order[i];
              var picked = containers.filter(function (_container, index) { return families[index] === family; });
              if (picked.length) return picked;
            }
            return [containers[0]];
          }

          // Resolve a single binding part: a <kbd> (keyboard-style) or <div> (special glyph-only)
          function resolveSingleBindingPart(partEl) {
            if (!partEl) return '';
            var tag = partEl.tagName ? partEl.tagName.toLowerCase() : '';
            // Keyboard-style: <kbd> with icon glyph span + label span
            if (tag === 'kbd') {
              var spans = partEl.querySelectorAll('span span');
              if (spans.length >= 2) {
                var label = (spans[spans.length - 1].textContent || '').trim();
                if (label) return cleanKeyboardText(label);
              }
              var kbdText = (partEl.textContent || '').trim();
              if (kbdText) return cleanKeyboardText(kbdText);
            }
            // Special controller binding: glyph character only
            var text = (partEl.textContent || '').trim();
            if (!text) return '';
            if (_glyphToName) {
              for (var i = 0; i < text.length; i++) {
                var ch = text[i];
                if (ch.charCodeAt(0) < 128) continue;
                if (ch.charCodeAt(0) >= 0xD800 && ch.charCodeAt(0) <= 0xDBFF && i + 1 < text.length) {
                  ch = text[i] + text[i + 1];
                  i++;
                }
                if (_glyphToName[ch]) return _glyphToName[ch];
              }
            }
            return cleanText(text);
          }

          function bindingContainerFriendlyName(container) {
            // Keyboard modifiers can be wrapped one level deeper than the main
            // key. Scan the whole variant while keeping only outer binding parts
            // so icons nested inside a <kbd> are not announced twice.
            var candidates = container.querySelectorAll('kbd, .bng-binding-icon');
            var parts = toArray(candidates).filter(function(part) {
              var parentPart = part.parentElement && part.parentElement.closest('kbd, .bng-binding-icon');
              return !parentPart || !container.contains(parentPart);
            });
            var names = [];
            for (var i = 0; i < parts.length; i++) {
              var name = resolveSingleBindingPart(parts[i]);
              if (name) names.push(name);
            }
            if (names.length) return names.join(' + ');
            return resolveSingleBindingPart(container);
          }

          function getBindingFriendlyName(bindingEl) {
            try {
              if (!_glyphToName) buildGlyphMap();
              // Current Vue BngBinding renders one binding-container per variant.
              // Combo containers contain each modifier/key/icon as separate parts;
              // reading the wrapper as one string stops at the first icon glyph.
              var containers = bindingEl.matches && bindingEl.matches('.binding-container')
                ? [bindingEl]
                : toArray(bindingEl.querySelectorAll(':scope > .binding-container'));
              if (containers.length) {
                var variants = pickBindingVariants(containers).map(bindingContainerFriendlyName).filter(Boolean);
                if (variants.length) return variants.join(' or ');
              }

              // Legacy Angular bindings put their parts directly under the wrapper.
              var parts = bindingEl.querySelectorAll(':scope > kbd, :scope > div');
              if (parts.length > 1) {
                var names = [];
                for (var i = 0; i < parts.length; i++) {
                  var n = resolveSingleBindingPart(parts[i]);
                  if (n) names.push(n);
                }
                if (names.length > 0) return names.join(' + ');
              }
              return resolveSingleBindingPart(bindingEl);
            } catch (e) {
              log('info', '[BINDING] getBindingFriendlyName error: ' + e.message);
              return '';
            }
          }

          // ========== RADIAL MENU SPEECH MODULE ==========
          // The middle of the radial menu -- the name, price and hotkey of the
          // item currently pointed at -- is painted onto a <canvas> by
          // radialCenterCanvas.js. See the <canvas ref="centerCanvas"> in
          // modules/radial/views/Radial.vue: it is even marked aria-hidden.
          // Canvas pixels carry no text, so there is nothing in the DOM to read;
          // the previous implementation scraped an SVG <foreignObject> that does
          // not exist in this UI, which is why it only ever logged "Gave up
          // finding wrap after 10 attempts" and then polled a null every 80ms.
          //
          // RadialCenterCanvas.prototype.setState is the last point at which the
          // label is still a string, and Radial.vue calls it on every focus and
          // blur whatever the input device (mouse, stick or d-pad). Wrapping it
          // hands us exactly the text a sighted player sees, event-driven, with
          // no polling and no dependence on markup.
          //
          // Everything outside the circle -- title, breadcrumbs, category tabs --
          // is still ordinary DOM and is read from there.
          var _radialMenuWasOpen = false;
          var _radialAnnouncedOpen = false;
          var _radialLastSpokenItem = '';
          var _radialLastCategory = '';
          var _radialLastHeading = '';
          var _radialLastSig = '';
          var _radialPendingSig = null;

          function radialCenterText(state) {
            if (!state) return '';
            // cleanText strips the bngIcons private-use glyphs that Radial.vue's
            // getHotkey() prefixes onto the control name.
            var label = cleanText(state.label);
            if (!label) return '';
            var parts = [label];
            // Price arrives as "250 <money glyph>"; cleanText drops the glyph, so
            // say what the bare number means.
            var price = cleanText(state.price);
            if (price) parts.push('costs ' + price);
            // state.hotkey is deliberately NOT spoken. Radial.vue's getHotkey()
            // builds it as an icon glyph plus the RAW control name, so cleanText
            // strips the glyph and leaves things like "Btn_a" -- and it is
            // appended to every wedge, so sweeping the menu reads a button name
            // after every single item. Worse, getHotkey calls makeViewerObj with
            // no device and no useLastDevice, so with two pads plugged in the
            // button it names is whichever device happens to come first in the
            // bindings list rather than the one in the player's hands. The item
            // name is what the menu is navigated by; the shortcut is not worth
            // the interruption, still less a wrong one.
            return parts.join(', ');
          }

          function onRadialCenterState(state) {
            if (!_radialMenuWasOpen) return;
            if (!state || !state.focused) {
              // Centre fell back to the "Select an option" placeholder, meaning
              // the pointer sits in the dead zone between wedges. That happens
              // every time the stick is released, so stay quiet -- but clear the
              // de-dupe so returning to the same item speaks it again.
              _radialLastSpokenItem = '';
              return;
            }
            var text = radialCenterText(state);
            if (!text || text === _radialLastSpokenItem) return;
            _radialLastSpokenItem = text;
            scheduleSpeak(text, P.CONTROLLER);
          }

          function installRadialCenterHook() {
            if (!RadialCenterCanvas || !RadialCenterCanvas.prototype) {
              log('error', '[bnvda] RadialCenterCanvas unavailable; radial item names will not be spoken.');
              return;
            }
            var proto = RadialCenterCanvas.prototype;
            var original = proto.setState;
            if (typeof original !== 'function' || original.__bnvdaWrapped) return;
            var wrapped = function (state) {
              var result = original.apply(this, arguments);
              // Read this.state, not the argument: setState normalizes it first,
              // filling in the defaults. Run after the original and inside a
              // try so a fault of ours can never stop the menu drawing.
              try {
                onRadialCenterState(this.state || state);
              } catch (e) {
                log('error', '[bnvda] Radial centre hook failed: ' + e.message);
              }
              return result;
            };
            wrapped.__bnvdaWrapped = true;
            proto.setState = wrapped;
            onCleanup(function () { if (proto.setState === wrapped) proto.setState = original; });
            log('info', '[bnvda] Radial centre hook installed.');
          }

          function radialHeadingText() {
            var title = document.querySelector('.radial-menu .radial-title');
            var crumbs = document.querySelector('.radial-menu .radial-breadcrumbs');
            var heading = cleanText(title && title.textContent);
            var trail = cleanText(crumbs && crumbs.textContent);
            // The breadcrumb trail starts with the title, so drop it when identical.
            if (trail && trail !== heading) return heading ? heading + ', ' + trail : trail;
            return heading;
          }

          function radialCategoryText() {
            var sel = document.querySelector('.radial-menu .radial-category.selected .radial-category-label');
            return cleanText(sel && sel.textContent);
          }

          // Opening the menu, descending a level and switching category all
          // replace the whole menu, and all three are plain DOM. A two-selector
          // read on the existing open/closed poll covers them without having to
          // subscribe to the Lua event stream.
          function radialMenuCheckLevel() {
            var heading = radialHeadingText();
            var category = radialCategoryText();
            var sig = heading + '\u0001' + category;
            if (sig === _radialLastSig) { _radialPendingSig = null; return; }
            // Both flicker through placeholders ("No Actions Available") while
            // Radial.vue's getUiData() round-trip to Lua is still in flight, so
            // only announce a reading that survived a whole poll tick.
            if (sig !== _radialPendingSig) { _radialPendingSig = sig; return; }
            _radialPendingSig = null;
            _radialLastSig = sig;
            _radialLastSpokenItem = '';

            var parts = [];
            if (heading && heading !== _radialLastHeading) parts.push(heading);
            if (category && category !== _radialLastCategory) parts.push(category);
            _radialLastHeading = heading;
            _radialLastCategory = category;
            if (!parts.length) return;

            var text = parts.join(', ');
            if (!_radialAnnouncedOpen) {
              _radialAnnouncedOpen = true;
              if (!/^radial menu/i.test(text)) text = 'Radial menu, ' + text;
            }
            scheduleSpeak(text, P.CONTROLLER);
          }

          function radialMenuOnClose() {
            _radialAnnouncedOpen = false;
            _radialLastSpokenItem = '';
            _radialLastCategory = '';
            _radialLastHeading = '';
            _radialLastSig = '';
            _radialPendingSig = null;
          }

          function startRadialMenuWatcher() {
            installRadialCenterHook();
            function pollRadialMenu() {
              var nextDelay = 1000;
              try {
                var isOpen = !!document.querySelector('.radial-menu');
                if (isOpen) {
                  nextDelay = 200;
                  _radialMenuWasOpen = true;
                  radialMenuCheckLevel();
                } else if (_radialMenuWasOpen) {
                  _radialMenuWasOpen = false;
                  radialMenuOnClose();
                }
              } catch (e) {
                log('info', '[bnvda] Radial menu watcher error: ' + e.message);
              }
              trackedSetTimeout(pollRadialMenu, nextDelay);
            }
            pollRadialMenu();
          }

          // ========== CURRENT VUE PAUSE / OPTIONS SCREENS ==========
          var VUE_TARGET_SELECTOR = "input,select,textarea,button,a[href],[role='option'],[role='menuitem'],[role='treeitem'],[role='tab'],[role='button'],[role='checkbox'],[role='switch'],[role='slider'],[bng-nav-item],.bng-row,.dropdown-option,.pause-button,.pause-menu-button,.pause-menu-tile,.category-button,[tabindex]";
          var _vueWatchTimer = null;
          var _vueWatchScreen = null;
          var _vueWatchSignature = '';
          var _vueWatchElement = null;
          var _vueConfigFocusEntry = true;
          var _vueOptionsOkDown = false;
          var _vueOptionsActivation = 0;
          var _vueOptionsCategory = '';
          var _vueOptionsCategoryTimer = null;
          var _vueOptionsCategoryGeneration = 0;
          var _vueOptionsCategoryEchoElement = null;
          var _vueOptionsCategoryEchoUntil = 0;
          var PARTS_DROPDOWN_ACTIVATION_TIMEOUT_MS = 1500;
          var _partsDropdownActivation = null;
          var _partsDropdownActivationTimer = null;
          var _partsDropdownBlockedDirections = {};
          var _partsDropdownPopup = null;
          var VUE_HINT_IDLE_MS = 3000;
          // vuePauseHintSet is the most expensive thing the focus poll touches:
          // nested querySelectorAll over footers/hints/binding-containers, each
          // filtered through visibleVueElement (getBoundingClientRect +
          // getComputedStyle -> forced layout), plus a per-character glyph scan
          // per binding. Running that every tick reflowed the document at poll
          // cadence to produce a signature that almost never changes. The hint is
          // only spoken after VUE_HINT_IDLE_MS of idle, so sampling it coarsely
          // cannot change the outcome.
          var VUE_HINT_SCAN_MS = 400;
          var _vueHintTimer = null;
          var _vueHintSignature = '';
          var _vueHintAnnounced = {};
          var _vueHintGeneration = 0;
          var _vueHintScanTs = 0;
          var _vueHintScanRoot = null;
          var _vueHintScanResult = null;

          function isVehicleSelectorRoute() {
            var route = ((location.hash || '') + ' ' + (location.pathname || '')).toLowerCase();
            return /vehicle[-_/]?selector|vehicleselect/.test(route);
          }

          function isVehicleConfigRoute() {
            var route = ((location.hash || '') + ' ' + (location.pathname || '')).toLowerCase();
            return /\/vehicle-config(?:\/|\b)|\/pause\/vehicle(?:\/|\b)|\/pause\/vehicle\/configurationcombined(?:\/|\b)/.test(route);
          }

          function isMainMenuRoute() {
            var hash = (location.hash || '').toLowerCase();
            var path = (location.pathname || '').toLowerCase();
            return /mainmenu|main-menu/.test(hash + ' ' + path) ||
              /^#\/(?:menu)?(?:\/|$)/.test(hash);
          }

          function focusedVueMainMenuItem(root) {
            if (!root || !isMainMenuRoute()) return null;
            var selectors = [
              '.mainmenu-button:hover', '.menu-button-simple:hover',
              '.focus-visible',
              '[aria-current="true"]', '[aria-current="page"]',
              '[aria-selected="true"]',
              '[bng-nav-item].selected', '[bng-nav-item].active',
              '.selected[bng-nav-item]', '.active[bng-nav-item]',
              '[bng-scoped-nav-autofocus]',
              '[bng-nav-item][tabindex="0"]'
            ];
            for (var i = 0; i < selectors.length; i++) {
              var candidates = toArray(root.querySelectorAll(selectors[i]));
              for (var j = 0; j < candidates.length; j++) {
                var candidate = closest(candidates[j], VUE_TARGET_SELECTOR) || candidates[j];
                if (visibleVueElement(candidate) && cleanText(extractText(candidate))) return candidate;
              }
            }
            return null;
          }

          function vueScreenRoot() {
            var configMarker = document.querySelector('.pause-tab-combined, .vehcfg, .parts-browser, .innerTuningCard, .paint-acc-wrapper, .saveload, .parts-packs, .mirrors-card, .adjustment-container, [class*="configuration-combined"], [class*="vehicle-configuration"]');
            if (configMarker && (isVehicleConfigRoute() || closest(configMarker, '.vehcfg, .mirrors-card, [class*="configuration-combined"], [class*="vehicle-configuration"]'))) {
              return closest(configMarker, '.pause-tab-combined, .vehcfg, .mirrors-card, [class*="configuration-combined"], [class*="vehicle-configuration"], #vue-app, .vue-app, main, [role="main"]') || configMarker;
            }
            var vehicleMarker = document.querySelector('.vehicle-grid, .grid-selector-screen-content, .grid-content');
            if (vehicleMarker && (isVehicleSelectorRoute() || closest(vehicleMarker, '.grid-selector-screen-content'))) {
              return closest(vehicleMarker, '#vue-app, .vue-app, main, [role="main"]') ||
                closest(vehicleMarker, '.grid-selector-screen-content') || document.body;
            }
            var optionMarker = document.querySelector('.options-view, .options-item-label-text, .options-categories, .options-category-button, .options-category-side, .options-content-wrapper, .options-toc, .options-category, .settings-category, #binding_list, .binding-item-row, .binding-item-rail-content');
            if (optionMarker) return closest(optionMarker, '#vue-app, .vue-app, .options-container, .options-screen, .settings-container, main, [role="main"]') || document.body;
            var pauseMarker = document.querySelector('.pause-menu, .pause-screen, .pause-tabs, button.pause-button');
            if (pauseMarker) return closest(pauseMarker, '#vue-app, .vue-app, .pause-menu, .pause-screen, main, [role="main"]') || document.body;

            // The 0.39 main menu has no pause/options/vehicle marker. Treat the
            // Vue application as a screen whenever BeamNG's navigation system
            // exposes a focused item. This also covers new Vue screens without
            // coupling accessibility to each screen's private CSS class names.
            var vueRoot = document.querySelector('#vue-app, .vue-app');
            if (vueRoot) {
              if (isMainMenuRoute()) return vueRoot;
              var focused = vueRoot.querySelector('.focus-visible');
              var active = document.activeElement;
              if ((focused && visibleVueElement(focused)) ||
                  (active && active !== document.body && vueRoot.contains(active))) {
                return vueRoot;
              }
            }
            return null;
          }

          function vueOwnLabel(el) {
            if (!el) return '';
            var attr = el.getAttribute && (el.getAttribute('aria-label') || el.getAttribute('data-label') || el.title);
            if (attr) return cleanText(attr);
            var label = el.querySelector && el.querySelector('.options-item-label-text, .binding-label, .action-title, .item-label, .button-label, .title, .label');
            return cleanText((label && label.innerText) || el.innerText || '');
          }

          function visibleVueElement(el) {
            if (!el || !el.isConnected || !el.getBoundingClientRect) return false;
            var rect = el.getBoundingClientRect();
            if (rect.width <= 0 || rect.height <= 0) return false;
            try {
              var style = window.getComputedStyle(el);
              if (style.display === 'none' || style.visibility === 'hidden') return false;
            } catch (e) {}
            return true;
          }

          function vuePartsDropdown() {
            var dropdowns = toArray(document.querySelectorAll('.bng-dropdown-content'));
            for (var i = 0; i < dropdowns.length; i++) {
              if (visibleVueElement(dropdowns[i]) && dropdowns[i].querySelector('.dropdown-option, [role=option]')) return dropdowns[i];
            }
            return null;
          }

          function focusedVuePartsDropdownOption(dropdown) {
            if (!dropdown) return null;
            var options = toArray(dropdown.querySelectorAll('.dropdown-option.focus-visible, [role=option].focus-visible'));
            var active = document.activeElement;
            if (active && dropdown.contains(active)) {
              var activeOption = closest(active, '.dropdown-option, [role=option]');
              if (activeOption) options.unshift(activeOption);
            }
            for (var i = 0; i < options.length; i++) if (visibleVueElement(options[i])) return options[i];
            return null;
          }

          function activeVuePartsDropdown() {
            if (_partsDropdownPopup && visibleVueElement(_partsDropdownPopup)) return _partsDropdownPopup;
            _partsDropdownPopup = null;
            return null;
          }

          // 0.39 renders the parts tree from two different components:
          //   vehicleConfig/components/Parts.vue          -> .parts-browser
          //     (the standalone vehicle config screen)
          //   pause/components/PauseVehicleConfigurationCombined.vue
          //     -> .pause-tab-combined-parts
          //     (Pause > Vehicle > Configuration Combined)
          // Only .parts-browser used to be matched, so the combined tab read
          // nothing and fell through to the generic label path. Keep every
          // parts lookup going through these two constants.
          var PARTS_CONTAINER_SELECTOR = '.parts-browser, .pause-tab-combined-parts';
          var PARTS_ROW_SELECTOR = '.parts-browser .bng-accitem, .pause-tab-combined-parts .bng-accitem';

          function focusedVuePartsRow() {
            var browsers = toArray(document.querySelectorAll(PARTS_CONTAINER_SELECTOR));
            for (var b = 0; b < browsers.length; b++) {
              var browser = browsers[b];
              if (!visibleVueElement(browser)) continue;
              var marked = toArray(browser.querySelectorAll('.focus-visible'));
              if (browser.contains(document.activeElement)) marked.push(document.activeElement);
              for (var i = 0; i < marked.length; i++) {
                var row = closest(marked[i], PARTS_ROW_SELECTOR);
                if (row && visibleVueElement(row)) return row;
              }
            }
            return null;
          }

          function vuePartsRowCanOpenDropdown(row) {
            if (!row || !row.isConnected || !row.querySelector('.dropdown-display')) return false;
            return !(row.disabled || row.getAttribute('aria-disabled') === 'true' ||
              row.classList.contains('disabled') || row.classList.contains('is-disabled') ||
              row.classList.contains('bng-accitem-disabled'));
          }

          function clearPartsDropdownActivation(clearDirectionLatch) {
            if (_partsDropdownActivationTimer) clearTimeout(_partsDropdownActivationTimer);
            _partsDropdownActivationTimer = null;
            _partsDropdownActivation = null;
            if (clearDirectionLatch) {
              _partsDropdownBlockedDirections = {};
              _partsDropdownPopup = null;
            }
          }

          function armPartsDropdownActivation(row) {
            clearPartsDropdownActivation(true);
            _partsDropdownActivation = {
              row: row,
              route: (location.hash || '') + '|' + (location.pathname || ''),
              dropdownSeen: false
            };
            _partsDropdownActivationTimer = trackedSetTimeout(function() {
              clearPartsDropdownActivation(true);
            }, PARTS_DROPDOWN_ACTIVATION_TIMEOUT_MS);
          }

          function partsDirectionName(action) {
            var name = String(action || '').toLowerCase().replace(/[-\s]+/g, '_');
            var match = name.match(/(?:^|_)(up|down|left|right)$/);
            return match ? match[1] : '';
          }

          function navigationValuePressed(value) {
            return value === true || value === '1' || (typeof value === 'number' && Math.abs(value) > 0.01);
          }

          function discardPartsDropdownDirection(event, action, value) {
            var direction = partsDirectionName(action);
            if (!direction) return false;
            var pressed = navigationValuePressed(value);
            if (_partsDropdownActivation && pressed) _partsDropdownBlockedDirections[direction] = true;
            var discard = !!_partsDropdownActivation || !!_partsDropdownBlockedDirections[direction];
            if (!pressed) delete _partsDropdownBlockedDirections[direction];
            if (!discard) return false;
            if (event) {
              if (typeof event.preventDefault === 'function') event.preventDefault();
              if (typeof event.stopPropagation === 'function') event.stopPropagation();
            }
            return true;
          }

          function updatePartsDropdownActivation(root) {
            if (!_partsDropdownActivation) return null;
            var activation = _partsDropdownActivation;
            var route = (location.hash || '') + '|' + (location.pathname || '');
            if (!root || route !== activation.route || !activation.row.isConnected ||
                !closest(activation.row, PARTS_CONTAINER_SELECTOR)) {
              clearPartsDropdownActivation(true);
              return null;
            }
            var dropdown = vuePartsDropdown();
            if (dropdown) {
              activation.dropdownSeen = true;
              _partsDropdownPopup = dropdown;
            }
            else if (activation.dropdownSeen) {
              clearPartsDropdownActivation(true);
              return null;
            }
            var option = focusedVuePartsDropdownOption(dropdown);
            if (option) {
              // Direction presses made during activation stay latched until release.
              clearPartsDropdownActivation(false);
              return option;
            }
            return null;
          }

          function resetVueHintLifecycle() {
            if (_vueHintTimer) clearTimeout(_vueHintTimer);
            _vueHintTimer = null;
            _vueHintSignature = '';
            _vueHintAnnounced = {};
            _vueHintGeneration++;
            resetVueHintScanCache();
          }

          function vuePauseHintSet(root) {
            if (!root || !root.querySelectorAll) return null;
            var footers = toArray(root.querySelectorAll('.info-bar-buttons'));
            for (var i = 0; i < footers.length; i++) {
              var footer = footers[i];
              if (!visibleVueElement(footer)) continue;
              var hints = toArray(footer.querySelectorAll('.hint'));
              var pairs = [];
              for (var j = 0; j < hints.length; j++) {
                var hint = hints[j];
                if (!visibleVueElement(hint)) continue;
                var actionEl = hint.querySelector('.hint-text');
                if (!actionEl || !visibleVueElement(actionEl)) continue;
                var action = cleanText(actionEl.innerText || actionEl.textContent || '');
                // Read one entry per BngBinding root, NEVER per .binding-container.
                // Hint.vue wraps its whole row of bindings in a div that carries
                // that same class (see its template), so a container scan matches
                // the outer div AND each BngBinding's own container inside it --
                // and getBindingFriendlyName reads the outer one whole. A hint
                // with a single button therefore came out as "A button or A
                // button". The wrapper class is BngBinding's own root and nothing
                // else uses it, so it cannot nest. Legacy Angular hints have no
                // wrapper, hence the fallback.
                var roots = toArray(hint.querySelectorAll('.binding-wrapper')).filter(visibleVueElement);
                if (!roots.length) roots = toArray(hint.querySelectorAll('.binding-container')).filter(visibleVueElement);
                var bindings = pickBindingVariants(roots).map(getBindingFriendlyName).filter(Boolean);
                if (!bindings.length || !action) continue;
                pairs.push({ button: bindings.join(' or '), action: action });
              }
              if (!pairs.length) continue;
              return {
                signature: pairs.map(function(pair) {
                  return (pair.button + '|' + pair.action).toLowerCase().replace(/\s+/g, ' ').trim();
                }).join('||'),
                speech: pairs.map(function(pair) { return pair.button + ', ' + pair.action; }).join('. ') + '.'
              };
            }
            return null;
          }

          // Throttled view of vuePauseHintSet for the poll path. A focus change
          // (activityChanged) always rescans, so cancelling a pending hint stays
          // immediate; only the idle repeat-scan is coarsened.
          function vuePauseHintSetThrottled(root, activityChanged) {
            var t = nowTS();
            if (!activityChanged && root === _vueHintScanRoot && (t - _vueHintScanTs) < VUE_HINT_SCAN_MS) {
              return _vueHintScanResult;
            }
            _vueHintScanTs = t;
            _vueHintScanRoot = root;
            _vueHintScanResult = vuePauseHintSet(root);
            return _vueHintScanResult;
          }

          function resetVueHintScanCache() {
            _vueHintScanTs = 0;
            _vueHintScanRoot = null;
            _vueHintScanResult = null;
          }

          function updateVuePauseHints(root, activityChanged) {
            var hintSet = vuePauseHintSetThrottled(root, activityChanged);
            if (!hintSet) {
              if (_vueHintSignature || _vueHintTimer) resetVueHintLifecycle();
              return;
            }
            var signatureChanged = hintSet.signature !== _vueHintSignature;
            if (signatureChanged) {
              if (_vueHintTimer) clearTimeout(_vueHintTimer);
              _vueHintTimer = null;
              _vueHintSignature = hintSet.signature;
              _vueHintGeneration++;
            } else if (activityChanged && _vueHintTimer) {
              clearTimeout(_vueHintTimer);
              _vueHintTimer = null;
              _vueHintGeneration++;
            }
            if (_vueHintAnnounced[hintSet.signature] || _vueHintTimer) return;
            var generation = _vueHintGeneration;
            _vueHintTimer = trackedSetTimeout(function() {
              _vueHintTimer = null;
              if (generation !== _vueHintGeneration) return;
              var currentRoot = vueScreenRoot();
              var current = vuePauseHintSet(currentRoot);
              if (!current || current.signature !== hintSet.signature) return;
              _vueHintAnnounced[hintSet.signature] = true;
              scheduleSpeak(current.speech, P.CONTROLLER);
            }, VUE_HINT_IDLE_MS);
          }

          function selectedVueOptionsCategory(root) {
            if (!root || !root.querySelector) return null;
            // Cheap gate: on any screen without an options/settings category rail
            // -- the parts selector included -- none of the 19 selectors below can
            // match, so without this they all ran to completion on every poll tick.
            if (!root.querySelector('.options-toc, .options-categories, .options-category, .settings-category')) return null;
            var selectors = [
              '.options-categories .options-category-button.selected',
              '.options-toc [aria-selected=true]',
              '.options-toc [aria-current=page]',
              '.options-toc .category-button.router-link-active',
              '.options-toc .category-button.selected',
              '.options-toc .category-button.active',
              '.options-toc .category-button.current',
              '.options-toc [role=tab].selected',
              '.options-toc [role=tab].active',
              '.options-toc [role=tab].current',
              '.options-toc .router-link-active',
              '.options-toc .selected',
              '.options-toc .active',
              '.options-toc .current',
              '.options-category[aria-selected=true]',
              '.options-category.selected',
              '.options-category.active',
              '.settings-category[aria-selected=true]',
              '.settings-category.selected',
              '.settings-category.active'
            ];
            for (var i = 0; i < selectors.length; i++) {
              var category = root.querySelector(selectors[i]);
              if (category) {
                category = closest(category, '.options-category-button, .category-button, [role=tab], .options-category, .settings-category, [bng-nav-item], button, a') || category;
              }
              if (category && visibleVueElement(category)) return category;
            }
            return null;
          }

          function vueOptionsCategoryLabel(root) {
            var category = selectedVueOptionsCategory(root);
            return cleanText(vueOwnLabel(category));
          }

          function isVueOptionsCategoryControl(el) {
            return !!closest(el, '.options-toc, .options-category, .settings-category, .category-button, [role=tab]');
          }

          function focusedVueOptionsItem(root) {
            if (!root || !root.querySelectorAll) return null;
            var focused = toArray(root.querySelectorAll('.focus-visible'));
            if (root.contains(document.activeElement)) focused.push(document.activeElement);
            for (var i = 0; i < focused.length; i++) {
              if (visibleVueElement(focused[i]) && !isVueOptionsCategoryControl(focused[i])) return focused[i];
            }
            var candidates = toArray(root.querySelectorAll(
              '.options-item .bng-row, .options-item [bng-nav-item], ' +
              '.options-item input, .options-item select, .options-item button, ' +
              '.binding-item-row, #binding_list [bng-nav-item]'
            ));
            for (var j = 0; j < candidates.length; j++) {
              if (visibleVueElement(candidates[j]) && !isVueOptionsCategoryControl(candidates[j])) return candidates[j];
            }
            return null;
          }

          function announceVueOptionsCategory(categoryLabel) {
            var generation = ++_vueOptionsCategoryGeneration;
            if (_vueOptionsCategoryTimer) clearTimeout(_vueOptionsCategoryTimer);
            _vueOptionsCategoryTimer = trackedSetTimeout(function () {
              _vueOptionsCategoryTimer = null;
              if (generation !== _vueOptionsCategoryGeneration) return;
              var root = vueScreenRoot();
              if (!root || vueOptionsCategoryLabel(root) !== categoryLabel) return;
              var item = focusedVueOptionsItem(root);
              var itemText = '';
              if (item) {
                _vueOptionsCategoryEchoElement = item;
                _vueOptionsCategoryEchoUntil = nowTS() + 500;
                var row = closest(item, '.bng-row, .options-item, .binding-row, .binding-item, .binding-item-row') || item;
                var labelEl = row.querySelector && row.querySelector('.options-item-label-text');
                var label = cleanText((labelEl && labelEl.innerText) || vueOwnLabel(item));
                var state = vueControlState(closest(item, VUE_TARGET_SELECTOR) || item, row);
                itemText = [label, state].filter(Boolean).join(', ');
                _vueWatchElement = item;
                _vueWatchSignature = vueFocusSignature(item, root);
              }
              scheduleSpeak(categoryLabel + (itemText ? '. ' + itemText : ''), P.CONTROLLER);
            }, 100);
          }

          function isVueOptionsCategoryEcho(element) {
            if (!element || nowTS() >= _vueOptionsCategoryEchoUntil) return false;
            if (element === _vueOptionsCategoryEchoElement) return true;
            var side = closest(element, '.options-category-side');
            var echoedSide = closest(_vueOptionsCategoryEchoElement, '.options-category-side');
            return !!side && side === echoedSide;
          }

          // ========== TAB / SUB-TAB TRACKING ==========
          // Changing a pause tab with the bumpers announced NOTHING. The poll's
          // screenKey looked for '[role=tab][aria-selected=true]', '.bng-tab.active'
          // and '.bng-tab.selected', and the game emits none of those: tabList.vue
          // renders a plain <Button class="tab-item tab-active-tab"> with no ARIA at
          // all. So the tab half of screenKey was dead, and a tab change was noticed
          // only when it happened to move the route or swap the sub-screen class --
          // at which point the branch RESETS the watcher (_vueWatchSignature = '',
          // _vueWatchElement = null) and speaks nothing, on the assumption that the
          // landing focus move will do the talking. It frequently does not: the
          // engine leaves focus where it was until the first D-pad tap, so the
          // driver arrives on a screen with no idea which one it is. Tracked here
          // the way the options category already is, because a tab change is the
          // same kind of event -- the whole context moved, not one item within it.
          //
          // Polled rather than hooked to the bumpers. tab_l/tab_r are only two of
          // the ways a tab changes (a click, a route push and the shell's own
          // selectedTab sync are the others), and the DOM is where all of them
          // agree.
          var _vueTabPath = '';
          var _vueTabSeen = false;
          var _vueTabTimer = null;
          var _vueTabGeneration = 0;
          // The tab name that is about to go out, and the window it opens once it
          // has. Long enough to cover the settle plus a slow route push and the
          // engine's own autofocus, which is when a landing focus move arrives;
          // short enough that a genuine second move by the driver still interrupts.
          var _vueTabQueueText = '';
          var _vueTabQueueUntil = 0;
          var VUE_TAB_QUEUE_MS = 900;
          // Let the tab strip settle before reading it, so a route push that
          // re-syncs selectedTab does not get announced twice.
          var VUE_TAB_SETTLE_MS = 60;

          // The active tab button carries no accessible name whatsoever when the
          // strip is icon-only -- and the pause vehicle sub-tabs (Parts, Tuning,
          // Paint, Other, Vehicle debug) are exactly that. bngTabs passes
          // icon-only, so tabList.vue renders the BngIcon and skips the <span> that
          // would have held the heading, leaving the name only in a tooltip that
          // does not exist in the DOM until the mouse hovers it. tabs.vue does hold
          // the authoritative list -- {index, heading, icon, tooltip, active} -- and
          // hands it down with provide("tabs", ...), so that is what gets read.
          // instance.provides is a plain runtime property that survives
          // minification (Vue's own inject() walks it), and the game ships a build
          // that defines __vueParentComponent unconditionally -- the same class of
          // internal the tuning-slider fix already rests on (appContext.propsCache).
          // The DOM tier below it still answers for every strip that renders text.
          function vueProvided(el, key) {
            var node = el, hops = 0;
            while (node && hops++ < 8) {
              var instance = node.__vueParentComponent;
              while (instance) {
                var provides = instance.provides;
                if (provides && (key in provides)) return provides[key];
                instance = instance.parent;
              }
              node = node.parentElement;
            }
            return null;
          }

          // A label that is still a bare translation key is otherwise spoken as
          // one. Stock pause tabs arrive pre-translated (routeLifecycleCallbacks.lua
          // builds every label through _tr) and the config sub-tabs use $t, but a
          // mod-contributed tab is under nobody's control and tabList.vue renders
          // its heading raw.
          var TRANSLATION_KEY_RE = /^[a-zA-Z][\w-]*(?:\.[\w-]+)+$/;
          function vueTranslatedLabel(text) {
            if (!text || !TRANSLATION_KEY_RE.test(text)) return text;
            try { return cleanText(findTranslateFunc()(text)) || text; } catch (e) { return text; }
          }

          function tabStripLabel(listEl) {
            var tabs = vueProvided(listEl, 'tabs');
            var list = tabs && (tabs.value || tabs);
            if (Array.isArray(list)) {
              for (var i = 0; i < list.length; i++) {
                var tab = list[i];
                if (!tab || !tab.active) continue;
                var name = cleanText(tab.heading || tab.tooltip || '');
                if (name) return vueTranslatedLabel(name);
              }
            }
            var active = listEl.querySelector('.tab-item.tab-active-tab, .tab-active-tab, [aria-selected="true"]');
            if (!active) return '';
            var text = cleanText(active.innerText || active.textContent || '');
            if (!text) text = cleanText((active.getAttribute && (active.getAttribute('aria-label') || active.title)) || '');
            return vueTranslatedLabel(text);
          }

          // Where the driver is, outermost first. layoutMenu shows tabs and
          // breadcrumbs as ALTERNATIVES (display.tabs is gated on there being no
          // breadcrumbs), so drilling into a tab replaces the strip with a trail --
          // which is why a breadcrumb-only screen has to contribute a name too, or
          // going one level deeper would read as leaving the tabs behind entirely.
          function vueTabPath(root) {
            var out = [];
            var lists = toArray(root.querySelectorAll('.tab-list'));
            for (var i = 0; i < lists.length; i++) {
              if (!visibleVueElement(lists[i])) continue;
              var label = tabStripLabel(lists[i]);
              if (label && out.indexOf(label) === -1) out.push(label);
            }
            var crumb = root.querySelector('.bng-path .bng-path-item.bng-path-last, .menu-breadcrumbs .bng-path-item.bng-path-last');
            if (crumb && visibleVueElement(crumb)) {
              var crumbText = vueTranslatedLabel(cleanText(crumb.innerText || crumb.textContent || ''));
              if (crumbText && out.indexOf(crumbText) === -1) out.push(crumbText);
            }
            return out.join(', ');
          }

          // The element the focus path would speak about, resolved WITHOUT the
          // parts-dropdown activation side effect so the tab announcer can ask the
          // same question the poll asks and be sure of the same answer.
          function focusedVueElement(root, bindingPopup, activatedDropdownFocused) {
            return vueBindingEditorFocused(bindingPopup) || activatedDropdownFocused ||
              focusedVuePartsDropdownOption(activeVuePartsDropdown()) ||
              focusedVueMainMenuItem(root) || root.querySelector('.focus-visible') ||
              (root.contains(document.activeElement) ? document.activeElement : null) ||
              document.querySelector('.bng-dropdown-content .dropdown-option.focus-visible');
          }

          // The tab name is spoken ALONE and the landing item is left to the focus
          // watcher, which then QUEUES behind it rather than interrupting.
          //
          // The first version appended the item here instead, to make one utterance
          // of it. That was wrong twice over. It read focus a fixed delay after the
          // tab flipped, and focus usually has not moved yet at that point -- on a
          // sub-tab change, where the old panel's element survives the switch, what
          // it appended was the item from the tab the driver had just LEFT. And
          // where it appended nothing, the focus move that arrived a moment later
          // cut the tab name off anyway, which is the whole complaint. Waiting long
          // enough to be sure is not available either: focus often never lands at
          // all until the first D-pad tap, so the wait would be charged to every
          // tab change to fix some of them.
          //
          // Queuing settles all of it without predicting anything. The tab name is
          // spoken as soon as it is known, the focus watcher announces whatever is
          // genuinely focused whenever that happens, and the window in emitSpeak
          // keeps the second from cutting the first. Two utterances back to back
          // take about as long as the combined one did, and neither can be stale.
          function announceVueTab(tabPath) {
            var generation = ++_vueTabGeneration;
            if (_vueTabTimer) clearTimeout(_vueTabTimer);
            _vueTabTimer = trackedSetTimeout(function () {
              _vueTabTimer = null;
              if (generation !== _vueTabGeneration) return;
              var root = vueScreenRoot();
              if (!root || vueTabPath(root) !== tabPath) return;
              _vueTabQueueText = tabPath;
              scheduleSpeak(tabPath, P.CONTROLLER);
            }, VUE_TAB_SETTLE_MS);
          }

          function resetVueTabTracking() {
            _vueTabPath = '';
            _vueTabSeen = false;
            _vueTabGeneration++;
            // Dropped rather than left armed: a tab name that never reached
            // emitSpeak (deduped, or suppressed by a loading screen) would
            // otherwise open the window around some unrelated later utterance.
            _vueTabQueueText = '';
            _vueTabQueueUntil = 0;
            if (_vueTabTimer) { clearTimeout(_vueTabTimer); _vueTabTimer = null; }
          }

          function vueControlState(control, row) {
            var out = [];
            if (!control) control = row;
            var role = control && control.getAttribute ? control.getAttribute('role') : '';
            var checked = control && control.getAttribute ? control.getAttribute('aria-checked') : null;
            if (checked === null && control && (control.type === 'checkbox' || control.type === 'radio')) checked = control.checked ? 'true' : 'false';
            // Do not gate this on an ancestor class. It used to require
            // closest(row, '.options-item-checkbox'), which does not exist anywhere
            // in 0.39, so binding-editor rows (.bng-row.options-item-row
            // .binding-option-row) never reported state -- Invert Axis, Feedback
            // Enabled and the vibration toggles all read as bare labels. The inner
            // query is self-validating: a row containing a switch toggle is a
            // switch row. This matches vueOptionsCheckboxState, which was already
            // ungated and is why the options screen did announce on/off.
            if (checked === null && row && row.querySelector) {
              var optionsToggle = row.querySelector('.options-checkbox-toggle, .bng-switch-toggle, .bng-switch-on');
              if (optionsToggle) {
                checked = (optionsToggle.classList.contains('is-checked') ||
                  optionsToggle.classList.contains('bng-switch-on')) ? 'true' : 'false';
              }
            }
            if (checked !== null) out.push(checked === 'true' ? 'on' : 'off');
            var expanded = control && control.getAttribute ? control.getAttribute('aria-expanded') : null;
            if (expanded !== null) out.push(expanded === 'true' ? 'expanded' : 'collapsed');
            var selected = control && control.getAttribute ? control.getAttribute('aria-selected') : null;
            if (selected === 'true' || (control.classList && control.classList.contains('selected'))) out.push('selected');
            var value = '';
            if (control) {
              if (control.type !== 'checkbox' && control.type !== 'radio') value = scalarValue(control.value);
              if (!value && control.getAttribute) value = control.getAttribute('aria-valuetext') || control.getAttribute('aria-valuenow') || '';
            }
            if (!value && row && row.querySelector) {
              var valueEl = row.querySelector('.dropdown-display, .options-item-value, .current-value, .value, output, input, select, [aria-valuetext], [aria-valuenow]');
              if (valueEl) {
                value = scalarValue(valueEl.value);
                if (!value) value = valueEl.getAttribute('aria-valuetext') || valueEl.getAttribute('aria-valuenow') || valueEl.innerText || '';
              }
            }
            value = cleanText(value);
            if (value) out.push(value);
            if ((control && control.disabled) || (control && control.getAttribute && control.getAttribute('aria-disabled') === 'true') || (row && row.getAttribute && row.getAttribute('aria-disabled') === 'true')) out.push('disabled');
            if ((control && control.getAttribute && control.getAttribute('aria-busy') === 'true') || (row && row.getAttribute && row.getAttribute('aria-busy') === 'true')) out.push('busy');
            if (role === 'slider' && !value) out.push('slider');
            return out.join(', ');
          }

          function vueOptionsCheckboxState(row) {
            if (!row || !row.querySelector) return null;
            var toggle = row.querySelector('.options-checkbox-toggle, .bng-switch-toggle, .bng-switch-on');
            if (!toggle) return null;
            return toggle.classList.contains('is-checked') || toggle.classList.contains('bng-switch-on');
          }

          function vueOptionsCheckboxUnavailable(row) {
            var item = row && row.parentElement;
            var toggle = row && row.querySelector && row.querySelector('.options-checkbox-toggle, .bng-switch-on');
            if (!row || !item) return true;
            return !!(row.disabled || item.disabled || (toggle && toggle.disabled) ||
              row.getAttribute('aria-disabled') === 'true' || item.getAttribute('aria-disabled') === 'true' ||
              (toggle && toggle.getAttribute('aria-disabled') === 'true') ||
              row.getAttribute('aria-busy') === 'true' || item.getAttribute('aria-busy') === 'true' ||
              (toggle && toggle.getAttribute('aria-busy') === 'true') ||
              row.classList.contains('disabled') || row.classList.contains('is-disabled') ||
              item.classList.contains('disabled') || item.classList.contains('is-disabled') ||
              row.classList.contains('busy') || row.classList.contains('is-busy') ||
              item.classList.contains('busy') || item.classList.contains('is-busy'));
          }

          function focusedVueOptionsCheckboxRow() {
            var focused = document.querySelector('.focus-visible') || document.activeElement;
            var row = closest(focused, '.options-item-checkbox > .bng-row');
            if (!row || !row.parentElement || !row.parentElement.classList.contains('options-item-checkbox')) return null;
            return row;
          }

          function vehicleConfigRoot(root) {
            var selector = '.pause-tab-combined, .vehcfg, .parts-browser, .innerTuningCard, .paint-acc-wrapper, .saveload, .parts-packs, .mirrors-card, .adjustment-container, [class*="configuration-combined"], [class*="vehicle-configuration"]';
            return root && ((root.matches && root.matches(selector)) || (root.querySelector && root.querySelector(selector)));
          }

          function vehicleConfigLabel(el) {
            if (!el) return '';
            var attr = el.getAttribute && (el.getAttribute('aria-label') || el.getAttribute('data-label') || el.getAttribute('data-name'));
            if (attr) return cleanText(attr);
            var label = el.querySelector && el.querySelector('.bng-row-label, .bng-accitem-caption-content, .pack-title, .tile-label, .label, label, .variable-title, .saveload-metadata-caption, .bng-card-heading');
            var text = cleanText((label && label.innerText) || vueOwnLabel(el));
            if (text) return text;
            // Tooltips are descriptions, not names -- several tuning rows share one
            // (every gear ratio carries the same text), so they only stand in when
            // the element offers no name of its own.
            var tipAttr = el.getAttribute && (el.getAttribute('data-tooltip') || el.title);
            if (tipAttr) return cleanText(tipAttr);
            if (el.__bngTooltip && el.__bngTooltip.text) return cleanText(el.__bngTooltip.text);
            return '';
          }

          function speakVueVehicleConfig(element, src, root, focusEntry) {
            if (!vehicleConfigRoot(root)) return false;
            var target = closest(element, VUE_TARGET_SELECTOR) || element;
            var out = [];

            var combined = closest(target, '.pause-tab-combined') || (root.matches && root.matches('.pause-tab-combined') ? root : null);
            var search = combined && closest(target, 'input[type="search"], input.search-input, .search input, [class*="search"] input');
            if (search) {
              scheduleSpeak('Search, edit; press Enter to return to Parts.', src);
              return true;
            }

            var pack = closest(target, '.folder-button, .pack-button, .folder-item, [class*="pack-item"]');
            if (pack && closest(pack, '.parts-packs')) {
              var packTitle = q(pack, '.pack-title');
              var packDescription = q(pack, '.pack-description');
              out.push(cleanText((packTitle && packTitle.innerText) || vehicleConfigLabel(pack)));
              if (packDescription) out.push(cleanText(packDescription.innerText));
              if (pack.classList.contains('selected') || pack.classList.contains('pack-selected') || pack.getAttribute('aria-selected') === 'true') out.push('selected');
              var path = q(closest(pack, '.parts-packs'), '.path-label');
              if (path) out.push(cleanText(path.innerText));
            }

            var partRow = closest(target, PARTS_ROW_SELECTOR);
            if (partRow && !out.length) {
              out.push(vehicleConfigLabel(partRow));
              var equipped = q(partRow, '.dropdown-display');
              var equippedText = cleanText(equipped && equipped.innerText);
              if (equippedText) out.push(equippedText.toLowerCase() === 'empty' ? 'slot is empty' : 'currently equipped: ' + equippedText);
              var visibility = q(partRow, '.visibility-toggle');
              if (visibility) out.push(visibility.classList.contains('visibility-toggle-on') ? 'visible' : 'hidden');
              if (partRow.classList.contains('changed') || partRow.classList.contains('is-changed') || partRow.classList.contains('bng-accitem-changed')) out.push('changed');
              if (partRow.classList.contains('disabled') || partRow.classList.contains('is-disabled') || partRow.classList.contains('bng-accitem-disabled') || partRow.getAttribute('aria-disabled') === 'true') out.push('disabled');
              if (partRow.classList.contains('bng-accitem-expandable')) out.push(partRow.classList.contains('bng-accitem-expanded') ? 'expanded' : 'collapsed');
            }

            var tuningRow = closest(target, '.innerTuningCard .input-container');
            if (tuningRow && !out.length) {
              var category = closest(tuningRow, '.tuning-category');
              var subcategory = closest(tuningRow, '.tuning-subcategory');
              var categoryName = cleanText((q(category, '.category-name') || {}).innerText);
              var subcategoryName = cleanText((q(subcategory, '.subcategory-name') || {}).innerText);
              if (categoryName !== _lastTuningCategory && categoryName) out.push(categoryName);
              if ((categoryName !== _lastTuningCategory || subcategoryName !== _lastTuningSubcategory) && subcategoryName) out.push(subcategoryName);
              _lastTuningCategory = categoryName;
              _lastTuningSubcategory = subcategoryName;
              // Name the row from its own title element. vehicleConfigLabel is only
              // a fallback here: the row's tooltip must never stand in for the name,
              // or every gear ratio announces the same shared description.
              var tuningName = cleanText((q(tuningRow, '.variable-title') || {}).innerText) || vehicleConfigLabel(tuningRow);
              out.push(tuningName);
              var tuningInput = q(tuningRow, 'input[data-testid="input"], input[type="range"], input[type="number"]');
              var tuningValue = tuningInput ? cleanText(tuningInput.value) : '';
              var tuningUnit = cleanText((q(tuningRow, '[data-testid="suffix"], .suffix, .unit') || {}).innerText);
              if (tuningValue) out.push(tuningValue + (tuningUnit ? ' ' + tuningUnit : ''));
              if ((tuningInput && tuningInput.disabled) || tuningRow.getAttribute('aria-disabled') === 'true') out.push('disabled');
              if (focusEntry) {
                if (_tuningHintTimer) clearTimeout(_tuningHintTimer);
                var tip = tuningRow.__bngTooltip;
                var hint = cleanText(tip && tip.text);
                if (hint && hint.toLowerCase() !== tuningName.toLowerCase()) {
                  _tuningHintTimer = trackedSetTimeout(function () { _tuningHintTimer = null; send({ type: 'speak', text: hint, interrupt: false }); }, 250);
                }
              }
            }

            var paintTile = closest(target, '.multi-paint-setup-item, .paint-preset, [class*="paint-tile"]');
            if (paintTile && !out.length) {
              out.push(vehicleConfigLabel(paintTile) || 'paint preset');
              if (paintTile.classList.contains('selected') || paintTile.getAttribute('aria-selected') === 'true') out.push('selected');
            }
            var mirrorTile = closest(target, '.mirror-button, .mirror-tile');
            if (mirrorTile && !out.length) out.push(vehicleConfigLabel(mirrorTile));

            if (!out.length) {
              var row = closest(target, '.bng-row, .saveload-row, .saveload-option, .bng-accitem, .paint-acc-container, .adjustment-container') || target;
              out.push(vehicleConfigLabel(target) || vehicleConfigLabel(row));
              var state = vueControlState(target, row);
              if (state) out.push(state);
              var prefix = row.querySelector && row.querySelector('[data-testid="prefix"], .prefix');
              var suffix = row.querySelector && row.querySelector('[data-testid="suffix"], .suffix, .unit');
              var affixes = cleanText(((prefix && prefix.innerText) || '') + ' ' + ((suffix && suffix.innerText) || ''));
              if (affixes) out.push(affixes);
              var status = row.querySelector && row.querySelector('.saveload-thumbnail-status, .saveload-save-status, .saveload-paint-validation, .validation-message, .packs-empty');
              if (status) out.push(cleanText(status.innerText));
            }
            var text = out.filter(Boolean).join(', ');
            if (text) scheduleSpeak(text, src);
            // Report handled only when something was actually spoken. Returning
            // true unconditionally swallowed the rest of the dispatch chain
            // (dropdown / checkbox / slider / extractText), so any selector that
            // drifted out of date turned into silence instead of a degraded
            // announcement.
            return !!text;
          }

          // BngRow normally handles controller activation. Some OptionsCheckbox
          // rows receive the event without invoking their registered control, so
          // fall back to the row's normal click path only if native handling did
          // not change its state after two rendered frames.
          function handleVueOptionsOk(value) {
            var pressed = value === true || (typeof value === 'number' && value > 0) || value === '1';
            if (!pressed) {
              _vueOptionsOkDown = false;
              return;
            }
            if (_vueOptionsOkDown) return;
            _vueOptionsOkDown = true;

            var partsRow = focusedVuePartsRow();
            if (vuePartsRowCanOpenDropdown(partsRow)) armPartsDropdownActivation(partsRow);

            var row = focusedVueOptionsCheckboxRow();
            if (!row || vueOptionsCheckboxUnavailable(row)) return;
            var before = vueOptionsCheckboxState(row);
            if (before === null) return;
            var route = (location.hash || '') + '|' + (location.pathname || '');
            var activation = ++_vueOptionsActivation;

            trackedRequestAnimationFrame(function () {
              trackedRequestAnimationFrame(function () {
                if (activation !== _vueOptionsActivation) return;
                if (!row.isConnected || route !== ((location.hash || '') + '|' + (location.pathname || ''))) return;
                if (focusedVueOptionsCheckboxRow() !== row || vueOptionsCheckboxUnavailable(row)) return;
                if (vueOptionsCheckboxState(row) !== before) return;
                row.click();
              });
            });
          }

          // Direction-ish actions whose repeat can sweep a control. `ok`/`back` and
          // the like are one-shot and must never gate speech.
          var NAV_ACTION_RE = /(^|_)(up|down|left|right|prev|previous|next|scroll|tab|change)(_|$)/;
          function navActionName(action) {
            var name = String(action || '').toLowerCase().replace(/[-\s]+/g, '_');
            return NAV_ACTION_RE.test(name) ? name : '';
          }

          // Track which navigation actions the engine reports as held. This is only
          // used to end a burst early: when the last direction comes up we know the
          // sweep is over and can announce the landing without waiting out the
          // settle timer. Screens where the engine emits repeats but no release
          // still settle correctly on the timer alone.
          function trackNavigationHold(action, value) {
            var name = navActionName(action);
            if (!name) return;
            if (navigationValuePressed(value)) {
              _navBurst.held[name] = true;
              return;
            }
            if (!_navBurst.held[name]) return;
            delete _navBurst.held[name];
            if (navBurstHeldCount() === 0 && _navBurst.active) navBurstArm(NAV_RELEASE_DRAIN_MS);
          }

          subscribe($rootScope, 'UINavigation', function (_event, action, value) {
            // Before the parts-dropdown filter: a discarded direction is still a
            // real key movement as far as speech pacing is concerned.
            trackNavigationHold(action, value);
            if (discardPartsDropdownDirection(_event, action, value)) return;
            if (action === 'ok') handleVueOptionsOk(value);
          });

          function bindingChipName(chip) {
            return getBindingFriendlyName(chip) || replaceKnownGlyphs((chip && (chip.innerText || chip.textContent)) || '');
          }

          // The Vue binding editor overlays the controls list, whose stale
          // .focus-visible marker can remain behind the active popup.
          function vueBindingEditorPopup() {
            var popup = document.querySelector('[bng-ui-scope=options-edit-binding-popup]');
            return popup && visibleVueElement(popup) ? popup : null;
          }

          function vueBindingEditorFocused(popup) {
            if (!popup) return null;
            // Scoped controller navigation moves this marker without always
            // moving document.activeElement. It is the authoritative target.
            var focused = toArray(popup.querySelectorAll('.focus-visible'));
            for (var i = 0; i < focused.length; i++) if (visibleVueElement(focused[i])) return focused[i];
            var active = document.activeElement;
            if (active && popup.contains(active) && active !== popup && visibleVueElement(active)) return active;
            return null;
          }

          function vueBindingEditorDisabled(control, row) {
            var nodes = [control, row];
            for (var i = 0; i < nodes.length; i++) {
              var node = nodes[i];
              if (node && (node.disabled || node.getAttribute('aria-disabled') === 'true' ||
                  node.classList.contains('disabled') || node.classList.contains('is-disabled'))) return true;
            }
            return false;
          }

          function vueBindingEditorRow(control, popup) {
            var row = closest(control, '.bng-row, .options-item, .binding-edit-row, .binding-editor-row, ' +
              '.conflict-row, [class*=conflict-row], .smart-select, [class*=smart-select], [class*=setting-row]');
            return row && popup.contains(row) ? row : control;
          }

          function vueBindingEditorLabel(row, control) {
            var label = row && row.querySelector && row.querySelector('.options-item-label-text, .bng-row-label, ' +
              '.binding-label, .setting-label, .label, label, [class*=label], [class*=title]');
            return cleanText((label && label.innerText) || vueOwnLabel(control));
          }

          function vueBindingEditorValue(control, row) {
            var value = control ? scalarValue(control.value) : '';
            if (!value && control && control.getAttribute) value = control.getAttribute('aria-valuetext') || control.getAttribute('aria-valuenow') || '';
            if (!value && row && row.querySelector) {
              var valueEl = row.querySelector('.dropdown-display, .current-value, .options-item-value, output, ' +
                'input[type=number], input[type=range], select, [aria-valuetext], [aria-valuenow]');
              if (valueEl) {
                value = scalarValue(valueEl.value);
                if (!value) value = valueEl.getAttribute('aria-valuetext') || valueEl.getAttribute('aria-valuenow') || valueEl.innerText || '';
              }
            }
            return cleanText(value);
          }

          // Basic Info, Axis Options, and FFB Options share these row shapes.
          function vueBindingEditorInfo(element, popup) {
            if (!element || !popup || !popup.contains(element)) return null;
            var control = closest(element, VUE_TARGET_SELECTOR) || element;
            var row = vueBindingEditorRow(control, popup);
            var label = vueBindingEditorLabel(row, control);
            var state = '';
            var role = 'control';
            var position = 0;
            // Vue often puts focus-visible/bng-nav-item on a component wrapper
            // while its semantic input or button is nested inside it.
            var button = closest(control, 'button, [role=button]') ||
              (control.querySelector && control.querySelector('button, [role=button]'));

            var tab = closest(control, '[role=tab], .bng-tab, [class*=tab-button]') ||
              (control.querySelector && control.querySelector('[role=tab], .bng-tab, [class*=tab-button]'));
            if (tab && popup.contains(tab)) {
              role = 'tab'; label = vueOwnLabel(tab) || label || 'Binding options'; state = vueControlState(tab, tab);
              position = toArray((tab.parentElement || popup).querySelectorAll('[role=tab], .bng-tab, [class*=tab-button]')).indexOf(tab);
              return { role: role, label: label, state: state, position: position };
            }
            var tabBar = closest(control, '[role=tablist], .bng-tabs, [class*=tab-list], [class*=tabs]');
            if (tabBar && button) {
              var tabButtons = toArray(tabBar.querySelectorAll('button, [role=button]'));
              position = tabButtons.indexOf(button); role = position <= 0 ? 'previous-tab' : 'next-tab';
              label = role === 'previous-tab' ? 'Previous tab' : 'Next tab';
              state = vueBindingEditorDisabled(button, tabBar) ? 'disabled' : '';
              return { role: role, label: label, state: state, position: position };
            }

            var buttonText = cleanText(vueOwnLabel(button));
            var buttonLower = buttonText.toLowerCase();
            if (button && (/^cancel\b/.test(buttonLower) || /^apply\b/.test(buttonLower))) {
              role = /^cancel\b/.test(buttonLower) ? 'cancel' : 'apply'; label = role === 'cancel' ? 'Cancel' : 'Apply';
              state = vueBindingEditorDisabled(button, row) ? 'disabled' : '';
              return { role: role, label: label, state: state, position: 0 };
            }

            var conflict = closest(control, '.conflict-row, [class*=conflict-row], [class*=conflict-item], [class*=conflict]');
            if (conflict) {
              var conflictLabel = vueBindingEditorLabel(conflict, control) || cleanText(conflict.innerText);
              position = button ? toArray(conflict.querySelectorAll('button, [role=button]')).indexOf(button) : 0;
              role = button ? 'conflict-remove' : 'conflict';
              label = button ? 'Remove' + (conflictLabel ? ' ' + conflictLabel : ' conflict') :
                'Conflict' + (conflictLabel ? ': ' + conflictLabel : '');
              state = vueBindingEditorDisabled(control, conflict) ? 'disabled' : '';
              return { role: role, label: label, state: state, position: position };
            }

            // DOM order is Reassign followed by Delete in the assigned row.
            var assignedRow = closest(control, '.bng-row, [class*=row]');
            var binding = assignedRow && assignedRow.querySelector('.binding-container, .bng-binding, [class*=binding-container]');
            if (assignedRow && binding && popup.contains(assignedRow)) {
              var assignedName = getBindingFriendlyName(binding) || cleanText(binding.innerText || binding.textContent);
              var actions = toArray(assignedRow.querySelectorAll('button, [role=button]'));
              if (button && actions.indexOf(button) >= 0) {
                position = actions.indexOf(button); role = position === 0 ? 'reassign' : 'delete';
                label = role === 'reassign' ? 'Reassign' : 'Delete'; state = assignedName ? 'currently ' + assignedName : '';
              } else { role = 'assigned-control'; label = 'Assigned control'; state = assignedName || 'unassigned'; }
              return { role: role, label: label, state: state, position: position };
            }

            var slider = closest(control, 'input[type=range], [role=slider]') ||
              (control.querySelector && control.querySelector('input[type=range], [role=slider]'));
            if (slider) {
              role = 'slider'; state = vueBindingEditorValue(slider, row);
              if (vueBindingEditorDisabled(slider, row)) state = [state, 'disabled'].filter(Boolean).join(', ');
              return { role: role, label: label || 'Slider', state: state, position: 0 };
            }
            var toggle = closest(control, 'input[type=checkbox], [role=checkbox], [role=switch]') ||
              (control.querySelector && control.querySelector('input[type=checkbox], [role=checkbox], [role=switch]'));
            if (toggle) {
              role = 'toggle'; var checked = toggle.getAttribute('aria-checked');
              if (checked === null && toggle.checked !== undefined) checked = toggle.checked ? 'true' : 'false';
              state = checked === 'true' ? 'on' : 'off';
              if (vueBindingEditorDisabled(toggle, row)) state += ', disabled';
              return { role: role, label: label || 'Toggle', state: state, position: 0 };
            }

            var smart = closest(control, '.smart-select, [class*=smart-select]');
            var select = closest(control, 'select, [role=combobox], [class*=select]') ||
              (control.querySelector && control.querySelector('select, [role=combobox], [class*=select]'));
            if (smart || select) {
              smart = smart || row; var smartButtons = toArray(smart.querySelectorAll('button, [role=button]'));
              if (button && smartButtons.indexOf(button) >= 0) {
                position = smartButtons.indexOf(button); role = position === 0 ? 'select-previous' : 'select-next';
                label = (label || 'Value') + (role === 'select-previous' ? ', previous' : ', next');
              } else role = 'select';
              state = vueBindingEditorValue(select || control, smart);
              if (vueBindingEditorDisabled(control, smart)) state = [state, 'disabled'].filter(Boolean).join(', ');
              return { role: role, label: label || 'Select', state: state, position: position };
            }

            if (button) { role = 'button'; label = buttonText || label || 'Button'; state = vueBindingEditorDisabled(button, row) ? 'disabled' : ''; }
            else state = vueControlState(control, row);
            return { role: role, label: label, state: state, position: position };
          }

          function speakVueBindingEditor(element, src, popup) {
            var info = vueBindingEditorInfo(element, popup || vueBindingEditorPopup());
            if (!info) return false;
            var text = [info.label, info.state].filter(Boolean).join(', ');
            if (text) scheduleSpeak(text, src);
            return true;
          }

          function vueBindingEditorSignature(element, popup) {
            var info = vueBindingEditorInfo(element, popup);
            return info ? ['binding-editor', info.role, info.position, info.label, info.state].join('|') : '';
          }

          function selectedBindingChip(rail) {
            if (!rail) return null;
            return rail.querySelector('.binding-item-chip-selected, .binding-item-chip.focus-visible, .binding-item.focus-visible, .binding-chip.focus-visible, [aria-selected="true"], .selected, .active') ||
              (rail.matches('.focus-visible') ? rail.querySelector('.binding-item-chip, .binding-item, .binding-chip, [class*="binding"]') : null);
          }

          function speakVueBinding(element, src) {
            var row = closest(element, '.binding-item-row');
            var rail = closest(element, '.binding-item-rail-content');
            if (!row && rail) row = closest(rail, '.binding-item-row') || rail.parentElement;
            if (!row) return false;
            var titleEl = row.querySelector('.bng-row-label, .binding-item-title, .binding-title, .action-title, [class*="action-name"], [class*="title"]');
            var title = cleanText((titleEl && titleEl.innerText) || row.getAttribute('aria-label') || 'Action');
            if (rail || closest(element, '.binding-item-chip, .binding-chip, .binding-item-rail-content .binding-item')) {
              rail = rail || row.querySelector('.binding-item-rail-content');
              var chips = toArray(rail ? rail.querySelectorAll('.binding-item-chip, .binding-chip, .binding-item') : []);
              var selected = selectedBindingChip(rail) || closest(element, '.binding-item-chip, .binding-chip, .binding-item');
              var position = chips.indexOf(selected);
              var name = bindingChipName(selected) || 'Unassigned';
              var parts = [name];
              if (position >= 0 && chips.length) parts.push((position + 1) + ' of ' + chips.length);
              parts.push('Activate to edit', 'Back to actions');
              scheduleSpeak(parts.join(', '), src);
              return true;
            }
            var rowChips = toArray(row.querySelectorAll('.binding-item-rail-content .binding-item-chip, .binding-item-rail-content .binding-chip, .binding-item-rail-content .binding-item'));
            var names = rowChips.map(bindingChipName).filter(Boolean);
            scheduleSpeak([title, names.length ? 'assigned to ' + names.join(', ') : 'unassigned',
              names.length ? 'Activate to select bindings' : 'Activate to add binding'].join(', '), src);
            return true;
          }

          function speakVueOptions(element, src, root) {
            if (!root || !root.querySelector('.options-view, .options-item-label-text, .options-categories, .options-category-button, .options-category-side, .options-content-wrapper, .options-toc, .options-category, .settings-category, #binding_list, .binding-item-row')) return false;
            if (speakVueBinding(element, src)) return true;
            var option = closest(element, '.dropdown-option, [role="option"]');
            if (option) {
              var optionText = vueOwnLabel(option);
              var optionState = vueControlState(option, option);
              scheduleSpeak([optionText, optionState].filter(Boolean).join(', '), src);
              return true;
            }
            var category = closest(element, '.category-button, [role="tab"]');
            if (category) {
              scheduleSpeak([vueOwnLabel(category), vueControlState(category, category)].filter(Boolean).join(', '), src);
              return true;
            }
            var row = closest(element, '.bng-row, .options-item, .binding-row, .binding-item, .binding-chip, .accordion-heading, .bng-accitem-caption');
            var target = closest(element, VUE_TARGET_SELECTOR) || element;
            if (!row) row = target;
            var labelEl = row.querySelector && row.querySelector('.options-item-label-text');
            var label = cleanText((labelEl && labelEl.innerText) || vueOwnLabel(target));
            var state = vueControlState(target, row);
            var text = [label, state].filter(Boolean).join(', ');
            if (text) scheduleSpeak(text, src);
            return true;
          }

          function speakVuePause(element, src, root) {
            if (!root || !root.querySelector('.pause-menu, .pause-screen, .pause-tabs, button.pause-button')) return false;
            var target = closest(element, VUE_TARGET_SELECTOR);
            if (!target) return true;
            var row = closest(target, '.bng-row') || target;
            var text = [vueOwnLabel(target), vueControlState(target, row)].filter(Boolean).join(', ');
            if (text) scheduleSpeak(text, src);
            return true;
          }

          function vehicleTileState(tile) {
            var out = [];
            var classes = tile && tile.classList;
            var disabled = tile && (tile.disabled || tile.getAttribute('aria-disabled') === 'true' ||
              (classes && (classes.contains('disabled') || classes.contains('is-disabled'))));
            var selected = tile && (tile.getAttribute('aria-selected') === 'true' || tile.getAttribute('aria-current') === 'true' ||
              (classes && (classes.contains('selected') || classes.contains('is-selected') || classes.contains('current'))) ||
              tile.querySelector('[aria-current="true"], .is-current'));
            var favourite = tile && ((classes && (classes.contains('favourite') || classes.contains('favorite') || classes.contains('is-favourite') || classes.contains('is-favorite'))) ||
              tile.querySelector('.favourite.active, .favorite.active, .is-favourite, .is-favorite, [aria-label*="avourite" i][aria-pressed="true"], [aria-label*="avorite" i][aria-pressed="true"]'));
            if (disabled) out.push('disabled');
            if (selected) out.push('selected');
            if (favourite) out.push('favourite');
            return out;
          }

          function vehicleTileCount(tile) {
            if (!tile) return '';
            var attr = tile.getAttribute('data-count') || tile.getAttribute('data-config-count') || tile.getAttribute('data-sub-element-count');
            var countEl = tile.querySelector('.sub-element-count, .subelement-count, .config-count, .configuration-count, .item-count');
            var raw = cleanText(attr || (countEl && countEl.innerText) || '');
            if (!raw) return '';
            var match = raw.match(/\d+/);
            if (!match) return raw;
            var count = parseInt(match[0], 10);
            return count + (count === 1 ? ' configuration' : ' configurations');
          }

          function speakVueVehicleSelector(element, src, root) {
            if (!root || !root.querySelector('.vehicle-grid, .grid-selector-screen-content, .grid-content')) return false;
            var tile = closest(element, '[bng-nav-item]');
            if (tile && tile.querySelector('.item-name')) {
              var name = cleanText(tile.querySelector('.item-name').innerText || '');
              var text = [name, vehicleTileCount(tile)].concat(vehicleTileState(tile)).filter(Boolean).join(', ');
              if (text) scheduleSpeak(text, src);
              return true;
            }
            var target = closest(element, VUE_TARGET_SELECTOR) || element;
            var row = closest(target, '.bng-row') || target;
            var genericText = [vueOwnLabel(target), vueControlState(target, row)].filter(Boolean).join(', ');
            if (genericText) scheduleSpeak(genericText, src);
            return true;
          }

          // ========== PAUSE > ENVIRONMENT: TIME OF DAY + TRAFFIC SUMMARY ==========
          // Two controls on this tab reach the generic path and announce a raw
          // internal number that is not the one on screen.
          //
          // The time-of-day slider is an <input type="range"> over MINUTES SINCE
          // MIDNIGHT (0-1440, step 5), so vueControlState finds it inside the row
          // and speaks "725". It is not merely unformatted, it is not even the
          // current time: a range element snaps its own .value to its step, so a
          // clock reading 12:03 reads back as 725, i.e. 12:05. The honest figure
          // is the sibling text input, which carries the game's own formatted
          // HH:MM -- so read that for BOTH controls and never convert the slider
          // position. The seconds live in a separate .suffix span that reruns
          // every second, and it was being taken as the row's LABEL (":05, 12:03");
          // it is deliberately dropped rather than appended, because the focus
          // signature is what re-announces a control and a per-second signature
          // would re-announce the time continuously while it plays.
          //
          // The traffic summary is four counts identified only by icon glyphs.
          // cleanText strips those (U+E000-U+F8FF, and it must), leaving the bare
          // "0 0 0 0" -- four numbers naming nothing. The names come from the
          // panel's own popover legend, matched by GLYPH rather than by position,
          // so a reordered or extended row falls back to the count alone instead
          // of confidently mislabelling it.
          var TRAFFIC_COUNT_ICONS = { cars: 'active', carPlus: 'pooled', carChase01: 'police', parking: 'parked' };

          function iconGlyphOf(name) {
            var catalog = iconCatalog || (window.bngVue && window.bngVue.icons) || {};
            var icon = catalog[name];
            return (icon && icon.glyph) || '';
          }

          function todTimeText(tod) {
            var input = tod && tod.querySelector('.tod-time-input input');
            return cleanText(scalarValue(input && input.value));
          }

          function todPlayLabel(button) {
            // The button carries an icon and nothing else, so which icon it is IS
            // the state: TodControl renders pause while time advances and play
            // while it is held. Compared against the catalog rather than against a
            // hard-coded code point, which is a font build detail.
            var glyphs = (button.innerText || button.textContent || '').replace(/[^\uE000-\uF8FF]/g, '');
            var pauseGlyph = iconGlyphOf('pause'), playGlyph = iconGlyphOf('play');
            if (pauseGlyph && glyphs.indexOf(pauseGlyph) !== -1) return 'Pause time';
            if (playGlyph && glyphs.indexOf(playGlyph) !== -1) return 'Resume time';
            return 'Play or pause time';
          }

          function trafficSummaryText(panel) {
            var items = toArray(panel.querySelectorAll('.traffic-summary-item'));
            if (!items.length) return '';
            var byGlyph = {}, names = Object.keys(TRAFFIC_COUNT_ICONS);
            for (var i = 0; i < names.length; i++) {
              var glyph = iconGlyphOf(names[i]);
              if (glyph) byGlyph[glyph] = TRAFFIC_COUNT_ICONS[names[i]];
            }
            var parts = [];
            for (var j = 0; j < items.length; j++) {
              var count = cleanText(items[j].innerText || items[j].textContent);
              if (!count) continue;
              var iconEl = items[j].querySelector('.icon-base');
              var raw = iconEl ? (iconEl.innerText || iconEl.textContent || '') : '';
              var name = byGlyph[raw.charAt(0)] || '';
              parts.push(name ? count + ' ' + name : count);
            }
            return parts.length ? 'Traffic, ' + parts.join(', ') : '';
          }

          function vueEnvironmentText(target) {
            if (!target) return '';
            var traffic = closest(target, '.traffic-count-panel');
            if (traffic) return trafficSummaryText(traffic);
            var tod = closest(target, '.tod-control');
            if (!tod) return '';
            if (closest(target, '.tod-play-step-button')) return todPlayLabel(target);
            // Not navigable today (tabindex -1, bng-no-nav), but it lives inside
            // .tod-slider and would otherwise inherit the slider's label.
            if (closest(target, '.bng-slider-popover-button')) return '';
            var time = todTimeText(tod);
            if (closest(target, '.tod-slider')) return ['Time of day', time].filter(Boolean).join(', ');
            if (closest(target, '.tod-time-input')) return ['Set time, edit', time].filter(Boolean).join(', ');
            // The step buttons carry real labels (-1h, +10m) and the day-length
            // and date controls on the full panel already read correctly; leave
            // every one of them to the generic path.
            return '';
          }

          function speakVueEnvironment(element, src) {
            var text = vueEnvironmentText(closest(element, VUE_TARGET_SELECTOR) || element);
            if (!text) return false;
            scheduleSpeak(text, src);
            return true;
          }

          function speakVueScreen(element, src) {
            var bindingPopup = vueBindingEditorPopup();
            if (bindingPopup && speakVueBindingEditor(element, src, bindingPopup)) return true;
            var root = vueScreenRoot();
            if (!root) return false;
            // Ahead of the screen-kind handlers: the environment controls appear
            // both on the pause Environment tab and inside its "More Time &
            // Weather Options" panel, and only the element itself says which.
            if (speakVueEnvironment(element, src)) return true;
            if (speakVueVehicleConfig(element, src, root, _vueConfigFocusEntry)) return true;
            if (speakVueVehicleSelector(element, src, root)) return true;
            if (speakVueOptions(element, src, root)) return true;
            return speakVuePause(element, src, root);
          }

          // Which kind of screen `root` is cannot change without screenKey changing,
          // so resolve it once per screen instead of once per poll tick. The
          // vehicleConfigRoot() probe is the costliest selector in the file --
          // [class*="..."] substring matching bypasses the browser's class/id fast
          // paths and runs document-wide.
          var _vueScreenKindRoot = null;
          var _vueScreenKindKey = null;
          var _vueScreenKindValue = '';

          function vueScreenKind(root) {
            if (root && root === _vueScreenKindRoot && _vueWatchScreen === _vueScreenKindKey && root.isConnected) {
              return _vueScreenKindValue;
            }
            var kind = vehicleConfigRoot(root) ? 'vehicle-config' :
              (root.querySelector('.vehicle-grid, .grid-selector-screen-content, .grid-content') ? 'vehicle-selector' :
              (root.querySelector('.options-view, .options-categories, .options-category-button, .options-content-wrapper, .options-item-label-text, .options-toc, #binding_list, .binding-item-row') ? 'options' : 'pause'));
            _vueScreenKindRoot = root;
            _vueScreenKindKey = _vueWatchScreen;
            _vueScreenKindValue = kind;
            return kind;
          }

          function vueFocusSignature(el, root) {
            if (!el) return '';
            var bindingPopup = vueBindingEditorPopup();
            if (bindingPopup && bindingPopup.contains(el)) return vueBindingEditorSignature(el, bindingPopup);
            // Sign the environment controls on what they will actually say, so
            // arrowing the slider re-announces the new time and nothing else in
            // the row (a per-second seconds field, a live traffic count) can
            // retrigger an announcement on its own.
            var environmentText = vueEnvironmentText(closest(el, VUE_TARGET_SELECTOR) || el);
            if (environmentText) return 'environment|' + environmentText;
            var screenKind = vueScreenKind(root);
            if (screenKind === 'vehicle-config') {
              var configRow = closest(el, '.input-container, .bng-accitem, .bng-row, .folder-button, .pack-button, .multi-paint-setup-item, .mirror-button, .saveload-row') || el;
              var accitem = closest(el, '.bng-accitem');
              var accitemState = accitem ? ((accitem.className || '').toString() + '|' + (accitem.getAttribute('aria-expanded') || '')) : '';
              return screenKind + '|' + vehicleConfigLabel(configRow) + '|' + vueControlState(el, configRow) + '|' + (el.className || '').toString() + '|' + accitemState;
            }
            var row = closest(el, '.bng-row, .options-item, .binding-row, .binding-item, .binding-item-row') || el;
            var tile = screenKind === 'vehicle-selector' ? closest(el, '[bng-nav-item]') : null;
            var label = tile && tile.querySelector('.item-name') ? cleanText(tile.querySelector('.item-name').innerText || '') : vueOwnLabel(el);
            var rail = closest(el, '.binding-item-rail-content') || (row.querySelector && row.querySelector('.binding-item-rail-content'));
            var selectedChip = selectedBindingChip(rail);
            var chipSignature = selectedChip ? bindingChipName(selectedChip) + '|' + toArray(rail.querySelectorAll('.binding-item-chip, .binding-chip, .binding-item')).indexOf(selectedChip) : '';
            return screenKind + '|' + label + '|' + vueControlState(el, row) + '|' + (el.className || '').toString() + '|' + chipSignature;
          }

          function startVueFocusWatcher() {
            if (_vueWatchTimer) return;
            function scheduleVuePoll(delay) {
              if (_vueWatchTimer) clearTimeout(_vueWatchTimer);
              _vueWatchTimer = trackedSetTimeout(pollVueFocus, delay);
            }
            function pollVueFocus() {
              // Keep a low-frequency fallback armed so one unexpected DOM shape
              // cannot permanently stop accessibility polling.
              scheduleVuePoll(1000);
              var nextDelay = 1000;
              var root = vueScreenRoot();
              updateTuningSliderDebounce(root);
              if (!root) {
                clearPartsDropdownActivation(true);
                resetVueHintLifecycle();
                // Only on the transition out — this branch re-runs once a second
                // during gameplay, and resetting every tick would stop non-Vue
                // screens from ever forming a burst.
                if (_vueWatchScreen !== null) navBurstReset();
                _vueWatchScreen = null; _vueWatchSignature = ''; _vueWatchElement = null;
                _vueOptionsCategory = '';
                _vueOptionsCategoryGeneration++;
                if (_vueOptionsCategoryTimer) { clearTimeout(_vueOptionsCategoryTimer); _vueOptionsCategoryTimer = null; }
                resetVueTabTracking();
                scheduleVuePoll(nextDelay);
                return;
              }
              // Matches NAV_POLL_FAST_MS: a slower base rate added up to 120ms of
              // pure sampling jitter to every focus move, and quantized the
              // nav-burst interval measurements on the first item of a sweep.
              nextDelay = NAV_POLL_FAST_MS;
              var bindingPopup = vueBindingEditorPopup();
              var optionsCategory = bindingPopup ? '' : vueOptionsCategoryLabel(root);
              if (optionsCategory && optionsCategory !== _vueOptionsCategory) {
                _vueOptionsCategory = optionsCategory;
                _vueOptionsCategoryEchoElement = focusedVueOptionsItem(root);
                _vueOptionsCategoryEchoUntil = nowTS() + 500;
                _vueWatchSignature = '';
                _vueWatchElement = null;
                lastFocusedElement = null;
                announceVueOptionsCategory(optionsCategory);
                updateVuePauseHints(root, true);
                scheduleVuePoll(nextDelay);
                return;
              }
              if (!optionsCategory) _vueOptionsCategory = '';
              var dialog = root.querySelector('[role="dialog"], .bng-dialog, .modal');
              // Resolved through the tab strips themselves rather than through the
              // ARIA the game does not emit. It doubles as the screenKey's tab
              // component, which matters for the tabs that do NOT move the route:
              // a mod-contributed pause tab only flips activeModTabId, so without
              // this the watcher never even noticed the screen had changed.
              // Frozen rather than blanked while the binding editor is up: that
              // popup contains a tab strip of its own, and blanking would make
              // closing it look like a move back onto the pause tab -- announcing
              // "System" at the one moment the driver has just dismissed something.
              var tabPath = bindingPopup ? _vueTabPath : vueTabPath(root);
              // Only a CHANGE is an event. The first sight of a screen is not one:
              // the focus watcher announces the landing item on entry already, and
              // treating entry as a tab change would double every arrival -- worst
              // on the options screen, where the category announcement has just
              // spoken and this would speak over it a tick later.
              var tabChanged = _vueTabSeen && tabPath !== _vueTabPath;
              _vueTabPath = tabPath;
              _vueTabSeen = true;
              var subScreen = root.querySelector('.adjustment-container, .parts-packs, .parts-browser, .pause-tab-combined-parts, .innerTuningCard, .paint-acc-wrapper, .saveload');
              var screenKey = (location.hash || '') + '|' + (location.pathname || '') + '|' +
                tabPath + '|' + (subScreen ? subScreen.className.toString() : '') + '|' +
                (dialog ? ((dialog.id || '') + ':' + (dialog.className || '').toString()) : 'screen');
              if (screenKey !== _vueWatchScreen) {
                if (_vueWatchScreen !== null) clearPartsDropdownActivation(true);
                // Never let an announcement pending from the old screen land here --
                // EXCEPT when the screen changed because a tab changed. Holding a
                // bumper sweeps tabs the same way holding a direction sweeps a list,
                // and most pause tabs push a route, so resetting the burst here
                // would clear `active` and `ema` on every step and make each tab in
                // the sweep take the leading-edge path, i.e. speak. That is the
                // chatter the coalescing exists to prevent, arriving through the one
                // door it does not watch.
                if (!tabChanged) navBurstReset();
                resetVueHintLifecycle();
                _vueWatchScreen = screenKey; _vueWatchSignature = ''; _vueWatchElement = null; lastFocusedElement = null;
                _lastTuningCategory = null; _lastTuningSubcategory = null;
                if (_tuningHintTimer) { clearTimeout(_tuningHintTimer); _tuningHintTimer = null; }
              }
              if (tabChanged && tabPath) {
                announceVueTab(tabPath);
                updateVuePauseHints(root, true);
                scheduleVuePoll(nextDelay);
                return;
              }
              var activatedDropdownFocused = updatePartsDropdownActivation(root);
              var focused = focusedVueElement(root, bindingPopup, activatedDropdownFocused);
              if (!focused) {
                updateVuePauseHints(root, false);
                scheduleVuePoll(nextDelay);
                return;
              }
              // The base rate is already NAV_POLL_FAST_MS, so a held sweep needs no
              // separate step-up here. It used to: at the old 120ms base, every
              // measured repeat interval quantized to ~120ms regardless of the real
              // rate, which both inflated the settle delay and left the landing
              // signature stale.
              _vueConfigFocusEntry = focused !== _vueWatchElement;
              var signature = vueFocusSignature(focused, root);
              var focusChanged = !!(signature && signature !== _vueWatchSignature);
              if (focusChanged) {
                _vueWatchSignature = signature;
                _vueWatchElement = focused;
                lastFocusedElement = null;
                processFocusChange(focused, P.CONTROLLER);
              }
              updateVuePauseHints(root, focusChanged);
              scheduleVuePoll(nextDelay);
            }
            scheduleVuePoll(0);
          }

          // Dropdown speech observes native Vue focus only. BeamNG remains solely
          // responsible for moving focus and applying controller repeat behavior.
          function speakDropdownOption(element, src) {
            var opt = closest(element, '.dropdown-option, [role="option"]');
            if (!opt) return false;
            var text = [vueOwnLabel(opt), vueControlState(opt, opt)].filter(Boolean).join(', ');
            if (text) scheduleSpeak(text, src);
            return true;
          }

          /* Legacy dropdown acceleration removed: native scoped navigation owns movement. */
          /*
          // ========== DROPDOWN LIST ACCELERATION ==========
          // BeamNG's parts/config dropdowns (.bng-dropdown-content, e.g. the wheel
          // and tire selectors) can hold hundreds of options. The D-pad moves the
          // navigation focus one option per repeat (~10/sec), so reaching a wheel
          // near the end of a 225-item list takes many seconds. This mirrors the
          // tuning-slider acceleration: while a direction is held, we advance the
          // focused option by an increasing number of steps the longer it's held.
          //
          // All options live in the DOM as flat .dropdown-option siblings (each
          // tabindex=0, the focused one carries .focus-visible). We poll the
          // focused option's index; when a direction is sustained we move native
          // focus several options ahead (and keep .focus-visible in sync so the
          // highlight and our own detection follow). Speech is throttled to the
          // landing option via speakDropdownOption so flying through the list does
          // not produce a wall of chatter.
          var DROP_POLL_MS = 40;
          var DROP_IDLE_RESET_TICKS = 6;   // ~240ms with no change ends a "hold"
          var DROP_HOLD_GAP_TICKS = 4;     // <=160ms gap still counts as the same hold
          var DROP_SPEAK_INTERVAL = 150;   // min ms between spoken option updates
          function dropMultiplier(count) {
            if (count < 3) return 1;   // ~first 300ms held: normal single-step
            if (count < 8) return 2;
            if (count < 15) return 4;
            if (count < 25) return 8;
            return 16;
          }
          var _drop = { container: null, index: -1, lastDir: 0, count: 0, idleTicks: 0, pending: 0, diag: 0 };
          function dropFocusedOption() {
            return document.querySelector('.bng-dropdown-content .dropdown-option.focus-visible');
          }
          function dropIndexOf(opts, el) {
            for (var i = 0; i < opts.length; i++) { if (opts[i] === el) return i; }
            return -1;
          }
          function dropResetState(cont, idx) {
            _drop.container = cont || null;
            _drop.index = (idx === undefined) ? -1 : idx;
            _drop.lastDir = 0;
            _drop.count = 0;
            _drop.idleTicks = 0;
            _drop.pending = 0;
            _drop.diag = 0;
          }
          // Ask Python to advance the dropdown by `count` steps. We do NOT move
          // focus ourselves and do NOT dispatch DOM key events: BeamNG's crossfire
          // navigation is driven by the game engine, not the DOM. The UI only emits
          // *untrusted* echo keydown events AFTER the engine has already moved, so a
          // synthetic DOM key (or .focus()) can never drive it. Instead Python
          // injects real OS-level numpad keystrokes, which the game reads as genuine
          // input. dir 1 = down/next, -1 = up/prev.
          function dropSendNav(dir, count) {
            // Removed: native Vue navigation must never be synthesized.
          }
          // Leading + trailing throttle so held/accelerated navigation speaks at
          // most every DROP_SPEAK_INTERVAL ms but always announces the final
          // option the user lands on. Returns true to claim the focus event.
          var _dropSpeak = { lastTs: 0, timer: null };
          function speakDropdownOption(element, src) {
            var opt = closest(element, '.dropdown-option');
            if (!opt || !closest(opt, '.bng-dropdown-content')) return false;
            var txt = cleanText(opt.innerText || '');
            if (!txt) return true; // it's a dropdown option, just nothing to say yet
            var now = nowTS();
            if (_dropSpeak.timer) { try { clearTimeout(_dropSpeak.timer); } catch (e) {} _dropSpeak.timer = null; }
            if (now - _dropSpeak.lastTs >= DROP_SPEAK_INTERVAL) {
              _dropSpeak.lastTs = now;
              scheduleSpeak(txt, src);
            } else {
              // Trailing edge: speak whatever option is focused once we settle.
              _dropSpeak.timer = trackedSetTimeout(function () {
                _dropSpeak.timer = null;
                _dropSpeak.lastTs = nowTS();
                var f = dropFocusedOption();
                scheduleSpeak(f ? cleanText(f.innerText || '') : txt, src);
              }, DROP_SPEAK_INTERVAL);
            }
            return true;
          }
          // TEMP PROBE: log real keyboard events while a dropdown is open so we can
          // see exactly what a genuine nav keystroke looks like (and whether menu
          // navigation produces DOM key events at all). Capped to avoid spam.
          var _dropProbe = 0;
          function startDropdownKeyProbe() {
            listen(window, 'keydown', function (e) {
              try {
                if (_dropProbe >= 20) return;
                if (!document.querySelector('.bng-dropdown-content')) return;
                _dropProbe++;
                var t = e.target;
                log('info', '[DROPKEY] key=' + e.key + ' code=' + e.code +
                  ' keyCode=' + e.keyCode + ' which=' + e.which + ' loc=' + e.location +
                  ' trusted=' + e.isTrusted + ' tgt=' +
                  (t && t.tagName ? t.tagName + '.' + (t.className || '').toString().split(' ')[0] : t));
              } catch (err) {}
            }, true);
            log('info', '[bnvda] Dropdown key probe installed.');
          }
          function startDropdownListAcceleration() {
            trackedSetInterval(function () {
              try {
                var el = dropFocusedOption();
                if (!el) { if (_drop.container) dropResetState(null); return; }
                var cont = closest(el, '.bng-dropdown-content');
                if (!cont) return;
                var opts = cont.querySelectorAll('.dropdown-option');
                if (!opts.length) return;

                var idx = dropIndexOf(opts, el);
                if (idx < 0) return;

                if (cont !== _drop.container) {
                  dropResetState(cont, idx);
                  log('info', '[DROP] dropdown opened: opts=' + opts.length + ' startIdx=' + idx);
                  return;
                }

                if (idx === _drop.index) {
                  _drop.idleTicks++;
                  if (_drop.idleTicks >= DROP_IDLE_RESET_TICKS) {
                    _drop.count = 0; _drop.lastDir = 0; _drop.pending = 0;
                  }
                  return;
                }

                // Focus moved since the last poll.
                var jump = idx - _drop.index;
                var absJump = jump < 0 ? -jump : jump;
                var dir = jump > 0 ? 1 : -1;

                // Echoes of our own injected steps: consume them against `pending`
                // and do NOT count them as held input (otherwise the injected motion
                // would feed back and runaway-accelerate). The engine applies our
                // injected keys asynchronously, so they arrive over several polls.
                if (_drop.pending > 0) {
                  _drop.pending -= absJump;
                  if (_drop.pending < 0) _drop.pending = 0;
                  _drop.lastDir = dir;
                  _drop.idleTicks = 0;
                  _drop.index = idx;
                  return;
                }

                // Natural user move.
                var prevIdle = _drop.idleTicks;
                if (dir === _drop.lastDir && _drop.idleTicks <= DROP_HOLD_GAP_TICKS) {
                  _drop.count++;
                } else {
                  _drop.count = 0; // direction change or a fresh tap after release
                }
                _drop.lastDir = dir;
                _drop.idleTicks = 0;
                _drop.index = idx;

                var mult = dropMultiplier(_drop.count);
                // Never jump more than ~1/12 of the list in a single tick, so even
                // at full speed the user can stop within a screen of their target.
                var cap = Math.max(1, Math.round(opts.length / 12));
                if (mult > cap) mult = cap;
                var extra = mult - 1;
                // Don't inject past the ends of the list.
                if (extra > 0) {
                  var room = dir > 0 ? (opts.length - 1 - idx) : idx;
                  if (extra > room) extra = room;
                }

                var diagOn = _drop.diag < 24;
                if (diagOn) {
                  _drop.diag++;
                  log('info', '[DROP] move idx=' + idx + ' dir=' + dir + ' gapTicks=' +
                    prevIdle + ' count=' + _drop.count + ' mult=' + mult + ' extra=' + extra);
                }

                if (extra > 0) {
                  dropSendNav(dir, extra);
                  _drop.pending = extra;
                }
              } catch (e) {
                log('info', '[DROP] accel error: ' + e.message);
              }
            }, DROP_POLL_MS);
            log('info', '[bnvda] Dropdown list acceleration started.');
          }

          */
          // ---------- Non-Invasive Observer for UI Changes ----------
          var mainObserver = null;
          var lastFocusedElement = null;
          var focusDebounceTimer = null;   // controller path
          var kbFocusDebounceTimer = null; // keyboard path
          function processFocusChange(element, src) {
            src = (src !== undefined) ? src : P.CONTROLLER;

            if (src === P.CONTROLLER) {
              // Controller wins: cancel any pending keyboard debounce as well.
              clearTimeout(focusDebounceTimer);
              clearTimeout(kbFocusDebounceTimer);
              focusDebounceTimer = trackedSetTimeout(function() {
                if (isVueOptionsCategoryEcho(element)) return;
                if (!element || element === lastFocusedElement) return;
                lastFocusedElement = element;
                if (speakVueScreen(element, src)) return;
                if (speakDropdownOption(element, src)) return;
                if (speakCheckboxRow(element, src)) return;
                if (speakSliderRow(element, src)) return;
                if (speakMenuAccordionItem(element, src)) return;
                scheduleSpeak(extractText(element), src);
              }, FOCUS_DEBOUNCE_MS);
            } else {
              // Keyboard path: separate timer, does not cancel controller timer.
              clearTimeout(kbFocusDebounceTimer);
              kbFocusDebounceTimer = trackedSetTimeout(function() {
                // If a controller became active during the debounce window, yield.
                if ((nowTS() - lastControllerTs) < CONTROLLER_DOMINANCE_MS) return;
                if (!element || element === lastFocusedElement) return;
                lastFocusedElement = element;
                if (speakVueScreen(element, src)) return;
                if (speakDropdownOption(element, src)) return;
                if (speakCheckboxRow(element, src)) return;
                if (speakSliderRow(element, src)) return;
                if (speakMenuAccordionItem(element, src)) return;
                scheduleSpeak(extractText(element), src);
              }, FOCUS_DEBOUNCE_MS);
            }
          }

          function attachMainObserver() {
            if (mainObserver) return;
            var targetNode = document.body;
            if (!targetNode) return;

            var observerConfig = {
              attributes: true,
              subtree: true,
              attributeFilter: ['class'],
              // We need oldValue so we can detect *transitions* into focus-visible.
              // Without this, every class mutation on an already-focused element
              // (md-active, ng-* state classes, animation toggles, etc.) re-fires
              // processFocusChange, causing needless clearTimeout/setTimeout churn.
              attributeOldValue: true
            };

            var callback = function(mutationsList, observer) {
              for(var mutation of mutationsList) {
                if (mutation.type !== 'attributes' || mutation.attributeName !== 'class') continue;
                var targetElement = mutation.target;
                if (!targetElement || !targetElement.classList || typeof targetElement.classList.contains !== 'function') continue;
                if (!targetElement.classList.contains('focus-visible')) continue;
                // Skip if focus-visible was already present before this mutation
                // (i.e. some other class changed on a still-focused element).
                var oldClass = mutation.oldValue || '';
                if (oldClass.indexOf('focus-visible') !== -1) continue;
                processFocusChange(targetElement);
              }
            };

            mainObserver = trackedMutationObserver(callback);
            mainObserver.observe(targetNode, observerConfig);
            log('info', '[bnvda] Attached main passive MutationObserver to UI.');
          }

          // ========== CONTEXT ACTION SYSTEM ==========
          // F9+Space sends a "context_action" via WS. This handler checks the
          // current UI state and executes the appropriate action.
          function handleContextAction(action) {
            if (action !== 'activate') return;
            log('info', '[CTXACTION] activate requested');

            // --- Freeroam Wizard: click the play/start button ---
            var playBtn = document.querySelector('.play-button');
            if (playBtn) {
              playBtn.click();
              log('info', '[CTXACTION] Clicked freeroam play button');
              return;
            }

            // --- Freeroam Configurator: click the action button ---
            var actionBtn = document.querySelector('.freeroam-configurator .action-button');
            if (actionBtn) {
              actionBtn.click();
              log('info', '[CTXACTION] Clicked configurator action button');
              return;
            }

            // No context action available
            log('info', '[CTXACTION] No context action found for current UI');
          }

          // ========== DOM DUMP LOGGER ==========
          // Triggered by F9+Ctrl+L. Walks the visible DOM tree and sends
          // a structured snapshot back via WS for logging to file.
          function performDomDump() {
            log('info', '[DOMDUMP] Starting DOM dump...');
            var lines = [];
            var url = window.location.href || '';
            lines.push('=== DOM DUMP ===');
            lines.push('URL: ' + url);
            lines.push('Time: ' + new Date().toISOString());

            // Identify the current Angular route/view
            try {
              var injector = angular.element(document.body).injector();
              if (injector) {
                var loc = injector.get('$location');
                if (loc) lines.push('Route: ' + loc.path());
              }
            } catch (e) {}

            // Find the focused element
            var focused = document.querySelector('.focus-visible') || document.activeElement;
            if (focused) {
              lines.push('Focused: <' + focused.tagName.toLowerCase() + '> class="' +
                (focused.className || '').toString().substring(0, 120) + '" text="' +
                (focused.innerText || '').replace(/\s+/g, ' ').trim().substring(0, 100) + '"');
            }

            lines.push('');

            // Walk visible elements with meaningful content
            function dumpNode(node, depth) {
              if (!node || !node.tagName) return;
              if (depth > 15) return;

              var tag = node.tagName.toLowerCase();
              // Skip invisible, script, style elements
              if (tag === 'script' || tag === 'style' || tag === 'link') return;
              var rect = node.getBoundingClientRect ? node.getBoundingClientRect() : null;
              var computed = null;
              try { computed = window.getComputedStyle(node); } catch (eStyle) {}
              if (computed && (computed.display === 'none' || computed.visibility === 'hidden')) return;

              var indent = '';
              for (var d = 0; d < depth; d++) indent += '  ';

              // Build attribute summary
              var attrs = [];
              if (node.id) attrs.push('id="' + node.id + '"');
              var cls = (node.className || '').toString().trim();
              if (cls) attrs.push('class="' + cls.substring(0, 100) + '"');
              if (node.hasAttribute && node.hasAttribute('ng-controller'))
                attrs.push('ng-controller="' + node.getAttribute('ng-controller') + '"');
              if (node.hasAttribute && node.hasAttribute('ng-click'))
                attrs.push('ng-click="' + node.getAttribute('ng-click').substring(0, 60) + '"');
              if (node.hasAttribute && node.hasAttribute('ng-bind-html'))
                attrs.push('ng-bind-html="' + node.getAttribute('ng-bind-html').substring(0, 60) + '"');
              if (node.hasAttribute && node.hasAttribute('bng-nav-item'))
                attrs.push('bng-nav-item');
              ['bng-no-nav', 'bng-no-child-nav', 'bng-ui-scope', 'bng-scoped-nav-autofocus',
                'data-bng-ui-scope', 'data-scope-id', 'data-nav-scope', 'aria-expanded',
                'aria-selected', 'aria-disabled'].forEach(function(name) {
                if (node.hasAttribute && node.hasAttribute(name)) attrs.push(name + '="' + node.getAttribute(name) + '"');
              });
              if (node.hasAttribute && node.hasAttribute('tabindex'))
                attrs.push('tabindex="' + node.getAttribute('tabindex') + '"');
              if (node.hasAttribute && node.hasAttribute('role'))
                attrs.push('role="' + node.getAttribute('role') + '"');
              if (node === focused) attrs.push('[FOCUSED]');

              // Get direct text (not children's text)
              var directText = '';
              for (var c = 0; c < node.childNodes.length; c++) {
                if (node.childNodes[c].nodeType === 3) {
                  var t = node.childNodes[c].textContent.replace(/\s+/g, ' ').trim();
                  if (t) directText += t + ' ';
                }
              }
              directText = directText.trim();
              if (directText.length > 80) directText = directText.substring(0, 80) + '...';

              // Only output nodes that have attrs, text, or are containers
              var hasContent = attrs.length > 0 || directText;
              var isContainer = tag === 'div' || tag === 'section' || tag === 'md-content' ||
                tag === 'md-list' || tag === 'md-grid-list' || tag === 'form' ||
                tag === 'table' || tag === 'tbody' || tag === 'ul' || tag === 'ol' ||
                tag === 'nav' || tag === 'header' || tag === 'main';

              if (hasContent || isContainer) {
                var line = indent + '<' + tag;
                if (attrs.length > 0) line += ' ' + attrs.join(' ');
                line += '>';
                if (rect) line += ' [' + Math.round(rect.left) + ',' + Math.round(rect.top) + ' ' + Math.round(rect.width) + 'x' + Math.round(rect.height) + ']';
                if (computed) line += ' {display=' + computed.display + ' visibility=' + computed.visibility + ' overflow=' + computed.overflow + '/' + computed.overflowY + '}';
                if (directText) line += ' "' + directText + '"';
                lines.push(line);
              }

              // Recurse into children
              var children = node.children;
              if (children) {
                for (var i = 0; i < children.length; i++) {
                  dumpNode(children[i], depth + 1);
                }
              }
            }

            function describeDiagnosticNode(node) {
              if (!node || !node.tagName) return '<none>';
              var r = node.getBoundingClientRect ? node.getBoundingClientRect() : null;
              var cs = null;
              try { cs = window.getComputedStyle(node); } catch (e) {}
              var names = ['bng-nav-item', 'bng-no-nav', 'bng-no-child-nav', 'bng-ui-scope',
                'bng-scoped-nav-autofocus', 'data-bng-ui-scope', 'data-scope-id', 'data-nav-scope',
                'tabindex', 'role', 'aria-expanded', 'aria-selected', 'aria-disabled'];
              var a = [];
              names.forEach(function(name) { if (node.hasAttribute && node.hasAttribute(name)) a.push(name + '="' + node.getAttribute(name) + '"'); });
              return '<' + node.tagName.toLowerCase() + (node.id ? ' id="' + node.id + '"' : '') +
                (node.className ? ' class="' + node.className.toString().substring(0, 120) + '"' : '') +
                (a.length ? ' ' + a.join(' ') : '') + '> [' + (r ? Math.round(r.left) + ',' + Math.round(r.top) + ' ' + Math.round(r.width) + 'x' + Math.round(r.height) : '?') + ']' +
                (cs ? ' {display=' + cs.display + ' visibility=' + cs.visibility + ' overflow=' + cs.overflow + '/' + cs.overflowY + '}' : '');
            }

            lines.push('');
            lines.push('=== ACTIVE ELEMENT AND ANCESTORS ===');
            var active = document.activeElement;
            lines.push('activeElement: ' + describeDiagnosticNode(active));
            var ancestor = focused;
            while (ancestor && ancestor !== document.documentElement) {
              lines.push('  ' + describeDiagnosticNode(ancestor));
              ancestor = ancestor.parentElement;
            }

            lines.push('');
            lines.push('=== PARTS TREES ===');
            var partRows = document.querySelectorAll('.parts-browser .bng-accitem, .pause-tab-combined .bng-accitem');
            for (var pi = 0; pi < partRows.length; pi++) {
              var pr = partRows[pi], prRect = pr.getBoundingClientRect(), prStyle = window.getComputedStyle(pr);
              if (prStyle.display === 'none' || prStyle.visibility === 'hidden' || prRect.width === 0) continue;
              var caption = cleanText(((q(pr, '.bng-accitem-caption-content') || {}).innerText) || '');
              var value = cleanText(((q(pr, '.dropdown-display') || {}).innerText) || '');
              lines.push('[' + pi + '] caption="' + caption + '" value="' + value + '" expanded=' +
                (pr.classList.contains('bng-accitem-expanded') || pr.getAttribute('aria-expanded') === 'true') +
                ' focused=' + !!(pr === focused || pr.contains(focused)) + ' navigable=' +
                !!(pr.hasAttribute('bng-nav-item') || pr.querySelector('[bng-nav-item], [tabindex]')) + ' ' + describeDiagnosticNode(pr));
            }

            lines.push('');
            lines.push('=== VUE SCOPES AND DIRECT NAV TARGETS ===');
            var scopes = document.querySelectorAll('[bng-ui-scope], [bng-scoped-nav-autofocus], [data-bng-ui-scope], [data-scope-id], [data-nav-scope]');
            for (var si = 0; si < scopes.length; si++) {
              lines.push('scope[' + si + '] ' + describeDiagnosticNode(scopes[si]));
              var direct = scopes[si].children;
              for (var di = 0; di < direct.length; di++) {
                if (direct[di].matches && direct[di].matches(VUE_TARGET_SELECTOR + ', [bng-ui-scope], [bng-no-nav], [bng-no-child-nav]')) lines.push('  target ' + describeDiagnosticNode(direct[di]));
              }
            }

            lines.push('');
            lines.push('=== ACTIVE SCOPED NAVIGATION ===');
            var activeScope = closest(focused, '[bng-ui-scope], [data-bng-ui-scope], [data-scope-id], [data-nav-scope]');
            lines.push('focused: ' + describeDiagnosticNode(focused));
            lines.push('scope: ' + describeDiagnosticNode(activeScope));

            lines.push('');
            lines.push('=== COMBINED CONFIGURATION AREAS ===');
            var combinedAreas = document.querySelectorAll('.pause-tab-combined, .pause-tab-combined .parts-browser, .pause-tab-combined .innerTuningCard, .pause-tab-combined .paint-acc-wrapper, .pause-tab-combined .options-container, .pause-tab-combined [class*="search"], .pause-tab-combined [role="dialog"]');
            for (var cai = 0; cai < combinedAreas.length; cai++) lines.push('[' + cai + '] ' + describeDiagnosticNode(combinedAreas[cai]));

            lines.push('');
            lines.push('=== DIALOGS ===');
            var dialogs = document.querySelectorAll('[role="dialog"], .bng-dialog, .modal, .modal-content');
            for (var dgi = 0; dgi < dialogs.length; dgi++) lines.push('[' + dgi + '] ' + describeDiagnosticNode(dialogs[dgi]));

            // Start from body, but skip our own invisible hook element
            var body = document.body;
            if (body) {
              var children = body.children;
              for (var i = 0; i < children.length; i++) {
                dumpNode(children[i], 0);
              }
            }

            // ---- Targeted dropdown enumeration ----
            // The main walker skips zero-size ancestors, so a teleported/overlay
            // dropdown list often never appears above. When a dropdown is open,
            // dump its container, every option, and the focused option's ancestor
            // chain so list navigation can be implemented from real structure.
            try {
              var opts = document.querySelectorAll('.dropdown-option');
              if (opts.length) {
                lines.push('');
                lines.push('=== DROPDOWN OPTIONS (' + opts.length + ') ===');
                var foc = document.querySelector('.dropdown-option.focus-visible') || focused;
                // Ancestor chain of the focused option (tag + id + classes).
                if (foc) {
                  lines.push('-- focused option ancestor chain --');
                  var chain = [];
                  var p = foc;
                  while (p && p !== document.body && chain.length < 15) {
                    var pc = (p.className || '').toString().trim();
                    var pr = p.getBoundingClientRect ? p.getBoundingClientRect() : null;
                    var sz = pr ? (Math.round(pr.width) + 'x' + Math.round(pr.height)) : '?';
                    var ov = '';
                    try {
                      var cs = window.getComputedStyle(p);
                      ov = ' overflow=' + cs.overflow + '/' + cs.overflowY;
                    } catch (e2) {}
                    chain.push('  <' + p.tagName.toLowerCase() +
                      (p.id ? ' id="' + p.id + '"' : '') +
                      (pc ? ' class="' + pc.substring(0, 90) + '"' : '') +
                      '> [' + sz + ']' + ov);
                    p = p.parentElement;
                  }
                  for (var ci = 0; ci < chain.length; ci++) lines.push(chain[ci]);
                }
                lines.push('-- options (in document order) --');
                for (var oi = 0; oi < opts.length; oi++) {
                  var o = opts[oi];
                  var ocls = (o.className || '').toString().trim();
                  var otxt = (o.innerText || '').replace(/\s+/g, ' ').trim().substring(0, 60);
                  var marks = [];
                  if (o.classList.contains('focus-visible')) marks.push('FOCUSED');
                  if (o.getAttribute && o.getAttribute('aria-selected') === 'true') marks.push('selected');
                  if (o.hasAttribute && o.hasAttribute('tabindex')) marks.push('tabindex=' + o.getAttribute('tabindex'));
                  lines.push('  [' + oi + '] class="' + ocls.substring(0, 70) + '"' +
                    (marks.length ? ' {' + marks.join(',') + '}' : '') +
                    ' "' + otxt + '"');
                }
                lines.push('=== END DROPDOWN OPTIONS ===');
              }
            } catch (eDrop) {
              lines.push('[dropdown enum error] ' + eDrop.message);
            }

            lines.push('=== END DOM DUMP (' + lines.length + ' lines) ===');

            // Send back as dom_dump_result
            send({ type: 'dom_dump_result', lines: lines });
            log('info', '[DOMDUMP] Sent ' + lines.length + ' lines.');
          }

          // ---------- MD-SELECT DROPDOWN CLOSE WATCHER ----------
          // When a user opens an md-select, picks an option, and closes the dropdown,
          // focus returns to the same md-select element. processFocusChange's
          // "element === lastFocusedElement" guard then suppresses re-speak, so the
          // user never hears which value ended up selected. Detect dropdown removal
          // and force a fresh announcement of the parent md-select row.
          var selectCloseObserver = null;
          function rememberOpenedSelect(sel) {
            if (sel && sel.tagName && sel.tagName.toLowerCase() === 'md-select') {
              _lastOpenedSelect = sel;
            } else if (sel) {
              var up = closest(sel, 'md-select');
              if (up) _lastOpenedSelect = up;
            }
          }
          function startSelectCloseWatcher() {
            if (selectCloseObserver) return;
            // Track md-select interaction via pointer or keyboard.
            listen(document, 'click', function(e) {
              rememberOpenedSelect(e.target);
            }, true);
            listen(document, 'keydown', function(e) {
              if (e.key !== 'Enter' && e.key !== ' ' && e.key !== 'Spacebar') return;
              rememberOpenedSelect(e.target);
            }, true);
            // md-select-menu-container is appended as a direct child of <body>
            // by Angular Material, so observe only body's direct children — no
            // subtree. This avoids firing on every node removal in the entire
            // UI tree (a major hitch source when closing complex screens like
            // the parts selector). The previous implementation also called
            // querySelector on each removed subtree, which walks tens of
            // thousands of nodes when a big panel is dismissed.
            selectCloseObserver = trackedMutationObserver(function(muts) {
              for (var i = 0; i < muts.length; i++) {
                var removed = muts[i].removedNodes;
                for (var j = 0; j < removed.length; j++) {
                  var n = removed[j];
                  if (n && n.nodeType === 1 && n.classList &&
                      n.classList.contains('md-select-menu-container')) {
                    var target = _lastOpenedSelect;
                    trackedSetTimeout(function(sel) {
                      return function() {
                        if (!sel || !document.body.contains(sel)) return;
                        lastFocusedElement = null;
                        lastSpoken = '';
                        processFocusChange(sel, P.KEYBOARD);
                      };
                    }(target), 150);
                    return;
                  }
                }
              }
            });
            selectCloseObserver.observe(document.body, { childList: true });
          }

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
          function updateTuningSliderDebounce(root) {
            if (!_sliderCacheHooked && root) hookSliderPropsCache(vueAppContext(root));
            var onConfigScreen = !!(root && vehicleConfigRoot(root));
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

          // ---------- EVENT HOOKS AND INITIALIZATION ----------
          function initializeModules() {
            log("info", "[bnvda] Page loaded. Initializing modules...");
            toasterInterval = trackedSetInterval(attachToasterPatcher, 1000);
            attachMainObserver();
            startRadialMenuWatcher();
            startVueFocusWatcher();
            trackedSetTimeout(function () {
              var navNodes = toArray(document.querySelectorAll('[bng-nav-item], [tabindex], [aria-selected], [aria-current]'));
              var samples = [];
              for (var i = 0; i < navNodes.length && samples.length < 8; i++) {
                if (!visibleVueElement(navNodes[i])) continue;
                samples.push(navNodes[i].tagName.toLowerCase() + '.' + cleanText(navNodes[i].className || '') + '="' + cleanText(extractText(navNodes[i])) + '"');
              }
              log('info', '[bnvda] Initial UI route=' + location.pathname + location.hash +
                ' active=' + (document.activeElement && document.activeElement.tagName) +
                ' focusVisible=' + document.querySelectorAll('.focus-visible').length +
                ' nav=' + navNodes.length + ' samples=[' + samples.join(' | ') + ']');
            }, 1000);
            trackedSetTimeout(function () {
              var focused = document.activeElement;
              if (focused && focused !== document.body) processFocusChange(focused, P.SYSTEM);
            }, 0);

          }


          function currentMeaningfulFocusText() {
            var focused = document.querySelector('.focus-visible') || document.activeElement;
            if (!focused || focused === document.body) return '';
            return extractText(focused);
          }

          function loadingShown() {
            if (!loadingScreen) return false;
            var shown = loadingScreen.shown;
            return !!(shown && typeof shown === 'object' && 'value' in shown ? shown.value : shown);
          }

          function handleLoadingState(shown) {
            if (loadingSettleTimer) {
              window.clearTimeout(loadingSettleTimer);
              timeoutIds.delete(loadingSettleTimer);
              loadingSettleTimer = null;
            }
            if (shown) {
              sawLoadingStart = true;
              loadingActive = true;
              suppressNextCameraEvent = true;
              if (speakTimer) {
                window.clearTimeout(speakTimer);
                timeoutIds.delete(speakTimer);
                speakTimer = null;
              }
              navBurstReset();
              lastFocusedElement = null;
              lastSpoken = '';
              send({ type: 'hover_cancel' });
              send({ type: 'loading_state', active: true, focusText: '' });
              log('info', '[bnvda] Loading lifecycle started; automatic speech suspended.');
              return;
            }
            if (!sawLoadingStart) return;
            // Keep suppression through the visual cover fade before Python's
            // one-second ready settling window begins.
            loadingSettleTimer = trackedSetTimeout(function () {
              loadingSettleTimer = null;
              var focusText = currentMeaningfulFocusText();
              send({ type: 'loading_state', active: false, focusText: focusText });
              loadingActive = false;
              lastFocusedElement = null;
              log('info', '[bnvda] Loading lifecycle ended; focus=' + JSON.stringify(focusText));
            }, 250);
          }

          startTransportSelection();
          trackedSetInterval(reportDebugState, 500);

          if (loadingScreen && typeof vueWatch === 'function') {
            var stopLoadingWatch = vueWatch(loadingShown, handleLoadingState, { immediate: true });
            onCleanup(function () { if (typeof stopLoadingWatch === 'function') stopLoadingWatch(); });
          } else {
            log('error', '[bnvda] Official loadingScreen state or Vue watch is unavailable.');
          }

          listen(window, "focusin", function (e) {
            processFocusChange(e.target, P.KEYBOARD);
          }, true);

          listen(window, "mouseover", throttle(function (e) {
            if ((nowTS() - lastControllerTs) < CONTROLLER_DOMINANCE_MS) return;
            var hoverText = hoverAnnouncementText(e.target);
            if (hoverText) scheduleSpeak(hoverText, P.POINTER);
          }, 150), true);
          if (document.readyState === 'complete') {
            initializeModules();
          } else {
            listen(window, 'load', initializeModules, { once: true });
          }

    })();
    console.info('[bnvda] Runtime installed.');
    return uninstall;
  } catch (e) {
    uninstall();
    console.error('[bnvda] Runtime startup failed.', e);
    return function () {};
  }
}
