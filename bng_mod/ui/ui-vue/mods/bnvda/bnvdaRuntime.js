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
          var vueWatch = dependencies.watch || null;
          var loadingActive = false;
          var loadingSettleTimer = null;
          var suppressNextCameraEvent = true;
          var sawLoadingStart = false;

          // ---------- CONFIG ----------
          var DEBOUNCE_MS = 50;
          var CONTROLLER_DOMINANCE_MS = 900;
          var MIN_CHARS = 2;
          var MAX_LEN = 160;
          var DEBUG = !!window.BNVDA_DEBUG;

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
          }
          function activateTransport(transport) {
            if (activeTransport && activeTransport !== transport) activeTransport.shutdown();
            activeTransport = transport;
            console.info('[bnvda] Active transport: ' + transport.name);
            transport.send({type: 'log', level: 'info', msg: '[bnvda] Active transport: ' + transport.name});
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
              if (DEBUG) {
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

          function scheduleSpeak(txt, src) {
            if (!txt) return;
            if (loadingActive) return;
            if (looksLikeCss(txt)) return;
            if (DEBUG && hasNonAscii(txt)) {
              log('info', '[GLYPH] "' + txt + '" chars=' + charDump(txt));
            }
            var t = nowTS();
            if (src === P.CONTROLLER) lastControllerTs = t;
            if (src === P.POINTER && (t - lastControllerTs) < CONTROLLER_DOMINANCE_MS) return;
            if (txt === lastSpoken && src <= lastSource && (t - lastSpeakTs) < 400) return;
            if (speakTimer) try { clearTimeout(speakTimer); } catch (e) {}
            speakTimer = trackedSetTimeout(function () {
              lastSpoken = txt;
              lastSource = src;
              lastSpeakTs = nowTS();
              send({ type: "speak", text: txt });
              if (DEBUG) log("info", "speak(" + src + "): " + txt);
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

          function extractText(el) {
            if (!el) return "";
            if (isHidden(el) || closest(el, '.loading-screen, .loadingBackground')) return "";
            // Known Vue buttons that only show a hotkey glyph
            if (el.matches && el.matches('button.pause-button')) return 'Pause';
            var targetElement = el.querySelector('[bng-translate], [ng-bind]') || el;
            var rawText = (targetElement.innerText || targetElement.textContent || "").trim();
            // Diagnose short results: log the element's context so we can improve extraction
            if (DEBUG) {
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

          // ---------- OPTIONS SCREEN SPEECH ----------
          var optionsObserverAttached = false;
          var optionsInterval;
          var _translate = null;
          function findTranslateFunc() {
            if (_translate) return _translate;
            if (window.bngApi && bngApi.engine && typeof bngApi.engine.translate === 'function') {
              _translate = bngApi.engine.translate;
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
            return function(key) { return key.substring(key.lastIndexOf('.') + 1).replace(/_/g, ' '); };
          }
          // Extract a readable label from a node (respecting both innerHTML translation
          // keys and plain innerText). Returns "" if nothing usable.
          function extractLabelFromElement(el, translator) {
            if (!el) return "";
            var html = el.innerHTML || "";
            var m = html.match(/['"](ui\.(?:options|debug|inputActions|mainmenu|common)\.[^'"]+)['"]/);
            if (m && m[1]) {
              var t = cleanText(translator(m[1]));
              if (t) return t;
            }
            var bngKey = el.getAttribute && el.getAttribute('bng-translate');
            if (bngKey) {
              var tk = cleanText(translator(bngKey));
              if (tk) return tk;
            }
            return cleanText(el.innerText || el.textContent || "");
          }

          function speakOptionRow(focusedElement, src) {
            var optionRow = closest(focusedElement, 'md-list-item');
            var isBare = false;
            if (!optionRow) {
              // Bare <div> consent rows (online / telemetry) — find enclosing container.
              var bareBox = closest(focusedElement, 'md-checkbox');
              if (bareBox) {
                optionRow = bareBox.parentElement;
                // Walk up to a container that also holds a descriptive <p>/[bng-translate]
                var hops = 0;
                while (optionRow && hops < 3 &&
                       !optionRow.querySelector('p, [bng-translate]')) {
                  optionRow = optionRow.parentElement;
                  hops++;
                }
                if (!optionRow) return false;
                isBare = true;
              } else {
                return false;
              }
            }
            var ngScope;
            try { ngScope = window.angular.element(optionRow).scope(); } catch(e) { return false; }
            if (!ngScope) return false;
            var optionsData = (ngScope.options && ngScope.options.data) || (ngScope.$parent && ngScope.$parent.options && ngScope.$parent.options.data);
            if (!optionsData && !isBare) {
              var simpleTextElem = optionRow.querySelector('h3, p, md-button');
              if (simpleTextElem) {
                 var simpleText = cleanText(simpleTextElem.innerText);
                 if (simpleText) { scheduleSpeak(simpleText, src); return true; }
              }
              return false;
            }
            var translator = findTranslateFunc();
            var parts = [];

            // Identify the focused control so multi-control rows can be disambiguated.
            var focusedControl = closest(focusedElement, 'md-select, md-checkbox, md-slider, input, md-button');
            if (focusedControl && !optionRow.contains(focusedControl)) focusedControl = null;
            var inputElem = focusedControl || optionRow.querySelector('md-select, md-checkbox, md-slider, md-input-container input, md-button');

            // --- Label extraction ---
            var labelText = '';

            // 1. md-input-container has its own <label> paired with the input.
            if (focusedControl) {
              var ic = closest(focusedControl, 'md-input-container');
              if (ic) {
                var icLabel = ic.querySelector('label');
                if (icLabel) labelText = cleanText(icLabel.innerText);
              }
            }

            // 2. Preceding sibling of focused control (e.g. <span flex> before a slider).
            if (!labelText && focusedControl && focusedControl.previousElementSibling) {
              var sib = focusedControl.previousElementSibling;
              if (sib.matches && sib.matches('p, label, span[flex]')) {
                var sibText = extractLabelFromElement(sib, translator);
                if (sibText) labelText = sibText;
              }
            }

            // 3. First meaningful <p>/<label>/<span flex> in the row.
            if (!labelText) {
              var candidates = toArray(optionRow.querySelectorAll('p, label, span[flex], [bng-translate]'));
              for (var i = 0; i < candidates.length; i++) {
                // Skip labels that belong to a *different* md-input-container than our focused one
                var candIc = closest(candidates[i], 'md-input-container');
                if (candIc && focusedControl && closest(focusedControl, 'md-input-container') && candIc !== closest(focusedControl, 'md-input-container')) continue;
                var ct = extractLabelFromElement(candidates[i], translator);
                if (ct) { labelText = ct; break; }
              }
            }

            // 4. For bare consent rows, fall back to a preceding description <p>.
            if (!labelText && isBare) {
              var desc = optionRow.querySelector('[bng-translate], p');
              if (desc) labelText = extractLabelFromElement(desc, translator);
            }

            if (labelText) parts.push(labelText);

            // --- Multi-control row: append column header (e.g. "X", "Y", "Z") ---
            if (focusedControl && inputElem === focusedControl) {
              var siblingControls = toArray(optionRow.querySelectorAll('md-slider, md-select, md-checkbox, md-input-container input, md-button'));
              if (siblingControls.length > 1) {
                var idx = siblingControls.indexOf(focusedControl);
                if (idx >= 0) {
                  var prev = optionRow.previousElementSibling;
                  while (prev && prev.tagName && prev.tagName.toLowerCase() !== 'md-list-item') {
                    prev = prev.previousElementSibling;
                  }
                  if (prev) {
                    var prevLabels = toArray(prev.querySelectorAll('p, span[flex]')).filter(function(el) {
                      return cleanText(el.innerText).length > 0;
                    });
                    if (prevLabels[idx]) {
                      parts.push(cleanText(prevLabels[idx].innerText));
                    }
                  }
                }
              }
            }

            // --- Input value extraction ---
            if (inputElem && inputElem.tagName) {
              var tagName = inputElem.tagName.toLowerCase();
              var valueText = '';

              if (tagName === 'input') {
                valueText = inputElem.value || '';
                if (!valueText) valueText = translator('ui.common.empty') || 'empty';
              } else if (tagName === 'md-button') {
                valueText = cleanText(inputElem.innerText);
              } else if (inputElem.hasAttribute && inputElem.hasAttribute('ng-model')) {
                var modelString = inputElem.getAttribute('ng-model');
                var currentValue;
                try {
                  if (ngScope.$$phase || ngScope.$root.$$phase) { currentValue = ''; }
                  else { currentValue = ngScope.$eval(modelString); }
                } catch(e) { currentValue = '[error]'; }

                if (tagName === 'md-select') {
                  var optionsPath = modelString.replace('.values.', '.options.') + '.modes';
                  var modes;
                  try { modes = (ngScope.$$phase || ngScope.$root.$$phase) ? null : ngScope.$eval(optionsPath); } catch (e) { modes = null; }
                  if (modes && modes.keys && modes.values) {
                    var valueIndex = modes.keys.indexOf(currentValue);
                    if (valueIndex !== -1 && modes.values[valueIndex]) { valueText = translator(modes.values[valueIndex]); }
                  }
                  if (!valueText) {
                    var mdsv = inputElem.querySelector('md-select-value');
                    if (mdsv) valueText = cleanText(mdsv.innerText);
                  }
                } else if (tagName === 'md-checkbox') {
                  var isChecked = inputElem.classList.contains('md-checked');
                  valueText = isChecked ? translator('ui.common.on') : translator('ui.common.off');
                } else if (tagName === 'md-slider') {
                  // Prefer a value display that is a sibling of THIS slider, not the first in the row.
                  var vdisp = inputElem.nextElementSibling;
                  if (vdisp && vdisp.tagName && (vdisp.tagName.toLowerCase() === 'span' || vdisp.tagName.toLowerCase() === 'input')) {
                    valueText = vdisp.tagName.toLowerCase() === 'input' ? vdisp.value : vdisp.innerText.trim();
                  }
                  if (!valueText) {
                    var valueDisplay = optionRow.querySelector('span:not([flex]), input[type=number]');
                    if (valueDisplay) { valueText = valueDisplay.tagName.toLowerCase() === 'input' ? valueDisplay.value : valueDisplay.innerText.trim(); }
                  }
                  if (!valueText) {
                    var aria = inputElem.getAttribute('aria-valuenow');
                    if (aria) valueText = aria;
                  }
                }
                if (!valueText) valueText = String(currentValue);
              } else if (tagName === 'md-checkbox') {
                var isChecked2 = inputElem.classList.contains('md-checked');
                valueText = isChecked2 ? translator('ui.common.on') : translator('ui.common.off');
              }

              if (valueText) parts.push(cleanText(valueText));
            }

            var finalText = parts.join(', ');
            if (!finalText && !parts.length) {
                var header = optionRow.querySelector('h3');
                if (header) {
                    var headerMatch = header.innerHTML.match(/['"](ui\.[^'"]+)['"]/);
                    if(headerMatch && headerMatch[1]) finalText = cleanText(translator(headerMatch[1]));
                }
            }
            if (!finalText) return false;
            if (src === undefined) src = P.KEYBOARD;
            scheduleSpeak(finalText, src);
            var tooltip = q(optionRow, 'md-tooltip');
            if (tooltip) {
                var tooltipKeyMatch = tooltip.innerHTML.match(/['"](ui\.options\.[^'"]+)['"]/);
                if (tooltipKeyMatch && tooltipKeyMatch[1]) {
                    var translatedHelp = translator(tooltipKeyMatch[1]);
                    if (translatedHelp && translatedHelp.length > 2) { trackedSetTimeout(function() { send({ type: "speak", text: cleanText(translatedHelp) }); }, 750); }
                }
            }
            return true;
          }
          function attachOptionsObserver() {
            if (optionsObserverAttached) return;
            var container = document.querySelector('.options-toc');
            if (container) {
              optionsObserverAttached = true;
              clearInterval(optionsInterval);
              log('info', '[bnvda] Options screen module attached.');
            }
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
          // Translation keys for the recurring "engine is off" stall messages — these
          // fire every ~1.8 s while ignitionLevel >= 2 and the engine isn't running,
          // producing annoying repetitive speech.  Block them here.
          var BLOCKED_TRANSLATION_KEYS = [
            'vehicle.vehicleController.stalled',
            'vehicle.vehicleController.stalledAutoClutch',
            'vehicle.vehicleController.stalledStarting'
          ];
          function processAndSpeakMessage(payload) {
            // Block known spammy translation keys before any processing
            if (typeof payload === 'object' && payload !== null && payload.txt) {
              if (BLOCKED_TRANSLATION_KEYS.indexOf(payload.txt) !== -1) return;
            }
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
              if (DEBUG) {
                try { log('warn', '[LUA_TABLE_SPEECH] source=Message.localization_descriptor count=' + Object.keys(payload).length + ' contents=' + JSON.stringify(payload)); } catch (e) {}
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
            if (finalText && finalText.length >= MIN_CHARS) { scheduleSpeak(finalText, P.SYSTEM); }
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
            processAndSpeakMessage(args.msg);
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

          // ========== CONTROLS BINDINGS NAVIGATION FIX ==========
          // Crossfire's isTarget scoring favors elements with high horizontal
          // overlap. The small right-aligned [ng-click] binding buttons inside
          // accordion panes have minimal overlap with full-width pane headers,
          // so navigation jumps header-to-header, skipping all bindings.
          // Fix: add bng-nav-item to the individual interactive elements inside
          // each row (label, each binding, add button) so crossfire can navigate
          // left/right across columns and up/down between rows.
          var _bindingsAlreadyPatched = false;
          function patchBindingsNavigation() {
            var bindingList = document.getElementById('binding_list');
            if (!bindingList) { _bindingsAlreadyPatched = false; return; }
            if (_bindingsAlreadyPatched) return;
            _bindingsAlreadyPatched = true;
            var patched = 0;
            var rows = bindingList.querySelectorAll('md-list-item[layout="row"]');
            for (var i = 0; i < rows.length; i++) {
              var row = rows[i];
              var label = row.querySelector('span[flex]');
              if (label && !label.hasAttribute('bng-nav-item')) {
                label.setAttribute('bng-nav-item', '');
                patched++;
              }
              var bindings = row.querySelectorAll('div.bng-binding[ng-click]');
              for (var j = 0; j < bindings.length; j++) {
                if (!bindings[j].hasAttribute('bng-nav-item')) {
                  bindings[j].setAttribute('bng-nav-item', '');
                  patched++;
                }
              }
              var addBtn = row.querySelector('div[ng-click="controlsBindings.select(action.key)"]');
              if (addBtn && !addBtn.hasAttribute('bng-nav-item')) {
                addBtn.setAttribute('bng-nav-item', '');
                patched++;
              }
            }
            if (patched > 0) {
              log('info', '[bnvda] Patched ' + patched + ' binding elements for controller navigation.');
            }
          }

          // Patch md-list-item rows on the Options screen that crossfire navigation
          // would otherwise skip. In BeamNG's options screens, md-list-items containing
          // md-select controls use the `md-no-proxy` class (the md-select itself is the
          // focus target, not the row). Crossfire appears to walk by list-item in some
          // modes, so without a tabindex on the row the user never lands on these rows
          // at all. Force them to be crossfire-visible by adding bng-nav-item and
          // tabindex="0". This also helps our focusin handler hear them consistently.
          function patchOptionsNavigation() {
            var rows = document.querySelectorAll('md-list-item.md-no-proxy');
            if (rows.length === 0) return;
            var patched = 0;
            for (var i = 0; i < rows.length; i++) {
              var row = rows[i];
              // Only touch rows that actually host a focus-capable control
              // (md-select, input, md-button, md-switch). Skip already-patched rows.
              if (row.hasAttribute('data-bnvda-patched')) continue;
              var hasControl = row.querySelector('md-select, md-input-container input, md-input-container textarea, md-switch, md-button');
              if (!hasControl) continue;
              row.setAttribute('data-bnvda-patched', '1');
              if (!row.hasAttribute('bng-nav-item')) row.setAttribute('bng-nav-item', '');
              if (!row.hasAttribute('tabindex')) row.setAttribute('tabindex', '0');
              // Also make sure any inner md-select has bng-nav-item so crossfire
              // can target it directly if it walks by control rather than row.
              var inner = row.querySelectorAll('md-select, md-input-container input');
              for (var k = 0; k < inner.length; k++) {
                if (!inner[k].hasAttribute('bng-nav-item')) {
                  inner[k].setAttribute('bng-nav-item', '');
                }
              }
              patched++;
            }
            if (patched > 0) {
              log('info', '[bnvda] Patched ' + patched + ' options md-no-proxy rows for controller navigation.');
            }
          }

          // Fallback: poll document.activeElement so md-select focus changes that
          // escape our focusin listener (e.g. programmatic focus transitions during
          // crossfire traversal) still trigger speech. This is cheap — we only
          // compare against our own _lastPolledActive reference.
          var _lastPolledActive = null;
          function pollActiveElementForOptions() {
            try {
              var ae = document.activeElement;
              if (!ae || ae === _lastPolledActive) return;
              // Only react while we're on an options/menu route to avoid spam.
              if (!document.querySelector('md-list-item.md-no-proxy')) {
                _lastPolledActive = ae;
                return;
              }
              _lastPolledActive = ae;
              // Only fire for controls we care about that focusin may have missed.
              var sel = closest(ae, 'md-select');
              if (sel) {
                processFocusChange(sel, P.KEYBOARD);
                return;
              }
              var row = closest(ae, 'md-list-item.md-no-proxy');
              if (row) {
                processFocusChange(ae, P.KEYBOARD);
              }
            } catch (e) {}
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

          // Built lazily on first use: glyph character -> friendly name
          var _glyphToName = null;
          function buildGlyphMap() {
            _glyphToName = {};
            try {
              var icons = iconCatalog || (window.bngVue && window.bngVue.icons);
              if (!icons) return;
              var keys = Object.keys(icons);
              for (var i = 0; i < keys.length; i++) {
                var iconName = keys[i];
                var friendly = ICON_FRIENDLY_NAMES[iconName];
                if (friendly && icons[iconName] && icons[iconName].glyph) {
                  _glyphToName[icons[iconName].glyph] = friendly;
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
                var variants = containers.map(bindingContainerFriendlyName).filter(Boolean);
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

          function speakBindingElement(focusedElement, src) {
            if (!focusedElement) return false;
            var header = closest(focusedElement, 'bng-pane-header');
            if (header) {
              var headerText = cleanText(header.innerText);
              if (headerText) {
                scheduleSpeak(headerText, src);
                return true;
              }
            }
            var row = closest(focusedElement, 'md-list-item[layout="row"]');
            if (row) {
              var labelEl = row.querySelector('span[flex][bng-tooltip]');
              var labelText = labelEl ? cleanText(labelEl.innerText) : "Action";
              var focusedBinding = closest(focusedElement, 'div.bng-binding[ng-click]');
              var focusedAddButton = closest(focusedElement, 'div[ng-click="controlsBindings.select(action.key)"]');
              var parts = [];
              if (focusedBinding) {
                var bindingText = getBindingFriendlyName(focusedBinding) || cleanText(focusedBinding.innerText);
                parts.push(bindingText, "for", labelText);
              } else if (focusedAddButton) {
                var addText = cleanText(focusedAddButton.innerText);
                parts.push(addText.includes('+') ? 'Add binding' : addText, "for", labelText);
              } else {
                parts.push(labelText);
                var bindingDivs = row.querySelectorAll('div.bng-binding[ng-click]');
                if (bindingDivs.length > 0) {
                  var bindingsText = toArray(bindingDivs).map(function(el) {
                    return getBindingFriendlyName(el) || cleanText(el.innerText);
                  }).filter(function(t) { return t; }).join(', ');
                  if (bindingsText) parts.push("assigned to", bindingsText);
                } else {
                  var unassignedEl = row.querySelector('span[ng-if="!controls.data.bindingsFilled[action.key]"]');
                  if (unassignedEl) parts.push(cleanText(unassignedEl.innerText));
                }
              }
              var finalText = parts.join(' ');
              if (finalText) {
                scheduleSpeak(finalText, src);
                return true;
              }
            }
            return false;
          }

          // ========== BINDING EDIT PANEL WATCHER ==========
          // After a binding is captured, the edit panel shows the assigned control
          // and any conflicts. This watcher speaks the result and makes conflicts
          // navigable via controller.
          var _bindingEditWasOpen = false;
          var _bindingEditHandled = false;

          function startBindingEditWatcher() {
            function pollBindingEdit() {
              var nextDelay = 1000;
              try {
                // The edit panel lives in [ui-view="edit"] and has .controls-edit
                var editPanel = document.querySelector('[ui-view="edit"] .controls-edit');
                var isOpen = !!editPanel;
                if (isOpen) nextDelay = 200;

                if (isOpen && !_bindingEditWasOpen) {
                  _bindingEditWasOpen = true;
                  _bindingEditHandled = false;
                }
                if (!isOpen && _bindingEditWasOpen) {
                  _bindingEditWasOpen = false;
                  _bindingEditHandled = false;
                }
                if (isOpen) {
                  // Keep patching while open because axis options can be mounted
                  // later or replaced when the user switches option tabs.
                  var cancelBtn = document.getElementById('binding_edit_cancel');
                  patchEditPanelNavigation(editPanel, cancelBtn);

                  // Result/conflict speech remains one-shot per editor opening.
                  // Wait until capture is done: cancel button is visible.
                  if (!_bindingEditHandled && cancelBtn) {
                    _bindingEditHandled = true;
                    handleBindingEditResult(editPanel, cancelBtn);
                  }
                }
              } catch (e) {
                log('info', '[BIND-EDIT] Watcher error: ' + e.message);
              } finally {
                trackedSetTimeout(pollBindingEdit, nextDelay);
              }
            }
            pollBindingEdit();
          }

          function patchEditPanelNavigation(editPanel, cancelBtn) {
            // Helper to set bng-nav-item + tabindex on an element
            function nav(el) {
              if (!el || el.hasAttribute('bng-nav-item')) return;
              el.setAttribute('bng-nav-item', '');
              if (!el.getAttribute('tabindex')) el.setAttribute('tabindex', '0');
            }

            // Delete binding button (trash icon next to assigned control)
            var deleteBtn = editPanel.querySelector('md-list-item md-button.md-warn');
            nav(deleteBtn);

            // Reassign button (the assigned control span with ng-click=captureBinding)
            var reassignSpan = editPanel.querySelector('span[ng-click="controlsEdit.captureBinding()"]');
            nav(reassignSpan);

            // Filter dropdown
            var filterSelect = editPanel.querySelector('md-select[ng-model="controlsEdit.newBinding.details.filterType"]');
            nav(filterSelect);

            // Axis options: sliders and checkboxes inside <axis-options>
            var axisOpts = editPanel.querySelector('axis-options');
            if (axisOpts) {
              var sliders = axisOpts.querySelectorAll('md-slider');
              for (var s = 0; s < sliders.length; s++) nav(sliders[s]);
              var checkboxes = axisOpts.querySelectorAll('md-checkbox');
              for (var c = 0; c < checkboxes.length; c++) nav(checkboxes[c]);
              // Steering angle select
              var steerSelect = axisOpts.querySelector('md-select');
              if (steerSelect) nav(steerSelect);
            }

            // Non-axis inverted checkbox (for buttons on centered actions)
            var invertedCb = editPanel.querySelector('md-checkbox[ng-model="controlsEdit.newBinding.details.isInverted"]');
            nav(invertedCb);

            // Conflict items
            var conflictPanel = editPanel.querySelector('.md-whiteframe-z3');
            if (conflictPanel) {
              var conflictItems = conflictPanel.querySelectorAll('md-list-item');
              for (var i = 0; i < conflictItems.length; i++) {
                var p = conflictItems[i].querySelector('p');
                nav(p);
                var delBtn = conflictItems[i].querySelector('md-button, button');
                nav(delBtn);
              }
            }

            // Cancel and apply buttons
            nav(cancelBtn);
            var btnRow = cancelBtn.parentElement;
            if (btnRow) {
              var applyBtn = btnRow.querySelector('.md-primary');
              nav(applyBtn);
            }
          }

          function handleBindingEditResult(editPanel, cancelBtn) {
            // Read the assigned control name from the <binding> element
            var bindingEl = editPanel.querySelector('md-list-item binding, md-list-item .bng-binding');
            var assignedName = '';
            if (bindingEl) {
              assignedName = getBindingFriendlyName(bindingEl) || cleanText(bindingEl.textContent);
            }

            // Patch all interactive elements for controller navigation
            patchEditPanelNavigation(editPanel, cancelBtn);

            // Check for conflicts
            var conflictPanel = editPanel.querySelector('.md-whiteframe-z3');
            var conflictItems = conflictPanel
              ? conflictPanel.querySelectorAll('md-list-item')
              : [];

            if (conflictItems.length > 0) {
              var conflictNames = [];
              var firstConflictP = null;
              for (var i = 0; i < conflictItems.length; i++) {
                var p = conflictItems[i].querySelector('p');
                var name = p ? cleanText(p.textContent) : '';
                if (name) conflictNames.push(name);
                if (!firstConflictP && p) firstConflictP = p;
              }

              var msg = conflictItems.length + ' conflict' + (conflictItems.length > 1 ? 's' : '') + '. ' +
                conflictNames.join(', ') + '. ';
              if (assignedName) msg = 'Assigned to ' + assignedName + '. ' + msg;
              scheduleSpeak(msg, P.SYSTEM);

              trackedSetTimeout(function() {
                if (firstConflictP) firstConflictP.focus();
              }, 100);
            } else {
              cancelBtn.focus();
              trackedSetTimeout(function() {
                var msg = assignedName
                  ? 'Assigned to ' + assignedName + '. No conflicts.'
                  : 'Binding assigned. No conflicts.';
                scheduleSpeak(msg, P.SYSTEM);
              }, 500);
            }
          }

          // Speak edit panel items when navigated to via controller
          function speakBindingEditItem(element, src) {
            if (!element) return false;
            var editPanel = document.querySelector('[ui-view="edit"] .controls-edit');
            if (!editPanel) return false;

            // --- Delete binding button (trash icon on assigned control row) ---
            if (element.tagName === 'MD-BUTTON' && element.classList.contains('md-warn')) {
              var assignedRow = closest(element, 'md-list-item');
              if (assignedRow && !closest(element, '.md-whiteframe-z3')) {
                scheduleSpeak('Delete binding', src);
                return true;
              }
            }

            // --- Reassign control span ---
            var reassign = element.getAttribute && element.getAttribute('ng-click');
            if (reassign && reassign.indexOf('captureBinding') !== -1) {
              var bindingEl = editPanel.querySelector('md-list-item binding, md-list-item .bng-binding');
              var name = bindingEl ? (getBindingFriendlyName(bindingEl) || cleanText(bindingEl.textContent)) : '';
              scheduleSpeak(name ? 'Reassign, currently ' + name : 'Reassign binding', src);
              return true;
            }

            // --- Conflict panel ---
            var conflictPanel = editPanel.querySelector('.md-whiteframe-z3');
            if (conflictPanel && conflictPanel.contains(element)) {
              var listItem = closest(element, 'md-list-item');
              if (listItem) {
                var p = listItem.querySelector('p');
                var cName = p ? cleanText(p.textContent) : '';
                var isResolved = p && (p.style.textDecoration || '').indexOf('line-through') !== -1;

                var btn = listItem.querySelector('md-button, button');
                if (btn && (element === btn || btn.contains(element))) {
                  var btnText = isResolved ? 'Restore' : 'Remove';
                  if (cName) btnText += ' ' + cName;
                  scheduleSpeak(btnText, src);
                  return true;
                }
                if (cName) {
                  var prefix = isResolved ? 'Removed: ' : 'Conflict: ';
                  scheduleSpeak(prefix + cName, src);
                  return true;
                }
              }
            }

            // --- Filter dropdown ---
            if (element.tagName === 'MD-SELECT') {
              var filterModel = element.getAttribute('ng-model') || '';
              if (filterModel.indexOf('filterType') !== -1) {
                var selected = element.querySelector('md-select-value .md-text, md-select-value');
                var val = selected ? cleanText(selected.textContent) : '';
                scheduleSpeak('Filter: ' + (val || 'unknown'), src);
                return true;
              }
              // Steering lock type select
              if (filterModel.indexOf('lockType') !== -1) {
                var selLock = element.querySelector('md-select-value .md-text, md-select-value');
                var lockVal = selLock ? cleanText(selLock.textContent) : '';
                scheduleSpeak('Lock type: ' + (lockVal || 'unknown'), src);
                return true;
              }
            }

            // --- Axis options: sliders and checkboxes ---
            if (element.tagName === 'MD-SLIDER') {
              var model = element.getAttribute('ng-model') || '';
              var row = closest(element, 'md-list-item');
              var labelEl = row ? row.querySelector('span[flex], span[flex="30"], span[flex="35"], p') : null;
              var label = labelEl ? cleanText(labelEl.textContent) : '';
              // Read current value from the sibling input
              var inputEl = row ? row.querySelector('input[type="number"]') : null;
              var val = inputEl ? inputEl.value : '';
              if (label) {
                scheduleSpeak(label + ': ' + val, src);
              } else if (model.indexOf('linearity') !== -1) {
                scheduleSpeak('Linearity: ' + val, src);
              } else if (model.indexOf('deadzoneResting') !== -1) {
                scheduleSpeak('Deadzone rest: ' + val, src);
              } else if (model.indexOf('deadzoneEnd') !== -1) {
                scheduleSpeak('Deadzone end: ' + val, src);
              } else if (model.indexOf('angle') !== -1) {
                scheduleSpeak('Steering angle: ' + val, src);
              } else {
                scheduleSpeak('Slider: ' + val, src);
              }
              return true;
            }

            if (element.tagName === 'MD-CHECKBOX') {
              var cbModel = element.getAttribute('ng-model') || '';
              var isChecked = element.classList.contains('md-checked');
              if (cbModel.indexOf('isInverted') !== -1) {
                scheduleSpeak('Inverted axis: ' + (isChecked ? 'on' : 'off'), src);
                return true;
              }
            }

            // --- Cancel / Apply buttons ---
            if (element.id === 'binding_edit_cancel' || closest(element, '#binding_edit_cancel')) {
              scheduleSpeak('Cancel', src);
              return true;
            }
            if (closest(element, '.md-primary.md-raised')) {
              scheduleSpeak('Apply', src);
              return true;
            }

            return false;
          }

          // ========== RADIAL MENU SPEECH MODULE ==========
          var _radialMenuWasOpen = false;
          var _radialLastSpokenItem = '';
          var _radialLastCategory = '';
          var _radialPollTimer = null;
          var _radialAttachTimer = null;
          var _radialWrap = null;

          // Radial menu SVG foreignObject layout (5 children of wrap):
          //   [0] svg icon  [1] empty div  [2] category/default text
          //   [3] item name (empty until hover)  [4] hotkey text
          // Vue replaces child nodes rather than mutating text, so we observe
          // the wrap element and re-read children by index on each mutation.
          function findRadialWrap(container) {
            var svgEl = container.querySelector('.radial-svg svg');
            if (!svgEl) { log('info', '[RADIAL] No .radial-svg svg found'); return null; }
            var fo = svgEl.querySelector('foreignObject');
            if (!fo) { log('info', '[RADIAL] No foreignObject in svg'); return null; }
            var body = fo.firstElementChild;
            if (!body) { log('info', '[RADIAL] No child in foreignObject'); return null; }
            var wrap = body.firstElementChild;
            if (!wrap) { log('info', '[RADIAL] No wrap element'); return null; }
            if (wrap.children.length < 4) { log('info', '[RADIAL] wrap has only ' + wrap.children.length + ' children, need at least 4'); return null; }
            return wrap;
          }

          function radialMenuOnOpen(container) {
            if (_radialPollTimer) { clearInterval(_radialPollTimer); _radialPollTimer = null; }
            if (_radialAttachTimer) { clearTimeout(_radialAttachTimer); _radialAttachTimer = null; }
            var attempts = 0;
            function tryAttach() {
              _radialAttachTimer = null;
              // Bail if the menu closed during retries — avoids leaking a poll
              // timer when the outer watcher fires open→close→open in quick succession.
              if (!container.isConnected || !document.querySelector('.radial-menu')) return;
              var wrap = findRadialWrap(container);
              if (!wrap) {
                attempts++;
                if (attempts < 10) { _radialAttachTimer = trackedSetTimeout(tryAttach, 100); }
                else { log('info', '[RADIAL] Gave up finding wrap after ' + attempts + ' attempts'); }
                return;
              }
              _radialWrap = wrap;
              var defaultText = (wrap.children[2] ? wrap.children[2].textContent : '').trim();
              var openMsg = 'Radial menu';
              if (defaultText && defaultText !== 'Select an option') {
                openMsg += ', ' + defaultText;
              }
              scheduleSpeak(openMsg, P.SYSTEM);

              var selectedCat = container.querySelector('.radial-category.selected .radial-category-label');
              if (selectedCat) {
                var catText = cleanText(selectedCat.textContent);
                if (catText) {
                  _radialLastCategory = catText;
                  trackedSetTimeout(function() { send({ type: "speak", text: catText }); }, 100);
                }
              }

              // Poll for changes — Vue's virtual DOM patching doesn't reliably
              // trigger MutationObserver, so we poll every 80ms instead.
              _radialPollTimer = trackedSetInterval(function() {
                if (!_radialWrap) return;
                // Self-check: if the radial menu has been removed from the DOM
                // (e.g., outer watcher was paused or missed the close), tear
                // ourselves down so we don't poll forever against a detached node.
                if (!_radialWrap.isConnected || !document.querySelector('.radial-menu')) {
                  _radialMenuWasOpen = false;
                  radialMenuOnClose();
                  return;
                }
                // Read all text children fresh each tick
                var children = _radialWrap.children;
                var itemText = '';
                var hotkeyText = '';
                var catDefault = '';
                if (DEBUG) {
                  for (var i = 0; i < children.length; i++) {
                    var t = (children[i].textContent || '').trim();
                    if (t) log('info', '[RADIAL-POLL] child[' + i + '] = "' + t.substring(0, 50) + '"');
                  }
                }
                if (children[3]) itemText = (children[3].textContent || '').trim();
                if (children[4]) hotkeyText = (children[4].textContent || '').trim();
                if (children[2]) catDefault = (children[2].textContent || '').trim();

                // If children[3] is empty, try children[2] as item text
                // (some states put the item name there instead)
                if (!itemText && catDefault && catDefault !== 'Select an option') {
                  itemText = catDefault;
                }

                if (!itemText) {
                  if (_radialLastSpokenItem !== '') {
                    _radialLastSpokenItem = '';
                    scheduleSpeak('empty', P.CONTROLLER);
                  }
                  return;
                }
                if (itemText === _radialLastSpokenItem) return;
                _radialLastSpokenItem = itemText;
                if (hotkeyText) {
                  var parts = hotkeyText.split(/\s+/);
                  hotkeyText = parts.length > 1 ? parts.slice(1).join(' ') : hotkeyText;
                }
                var speakText = hotkeyText ? itemText + ', ' + hotkeyText + ' key' : itemText;
                scheduleSpeak(speakText, P.CONTROLLER);

                // Check category change
                var sel = container.querySelector('.radial-category.selected .radial-category-label');
                if (sel) {
                  var newCat = cleanText(sel.textContent);
                  if (newCat && newCat !== _radialLastCategory) {
                    _radialLastCategory = newCat;
                    scheduleSpeak(newCat, P.CONTROLLER);
                  }
                }
              }, 80);

              log('info', '[bnvda] Radial menu polling started.');
            }
            tryAttach();
          }

          function radialMenuOnClose() {
            if (_radialPollTimer) { clearInterval(_radialPollTimer); _radialPollTimer = null; }
            if (_radialAttachTimer) { clearTimeout(_radialAttachTimer); _radialAttachTimer = null; }
            _radialLastSpokenItem = '';
            _radialLastCategory = '';
            _radialWrap = null;
          }

          function startRadialMenuWatcher() {
            function pollRadialMenu() {
              var nextDelay = 1000;
              try {
                var container = document.querySelector('.radial-menu');
                var isOpen = !!container;
                if (isOpen) nextDelay = 200;
                if (isOpen && !_radialMenuWasOpen) {
                  _radialMenuWasOpen = true;
                  radialMenuOnOpen(container);
                } else if (!isOpen && _radialMenuWasOpen) {
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
          var _vueHintTimer = null;
          var _vueHintSignature = '';
          var _vueHintAnnounced = {};
          var _vueHintGeneration = 0;

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

          function focusedVuePartsRow() {
            var browser = document.querySelector('.parts-browser');
            if (!browser || !visibleVueElement(browser)) return null;
            var marked = toArray(browser.querySelectorAll('.focus-visible'));
            if (browser.contains(document.activeElement)) marked.push(document.activeElement);
            for (var i = 0; i < marked.length; i++) {
              var row = closest(marked[i], '.parts-browser .bng-accitem');
              if (row && visibleVueElement(row)) return row;
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
                !closest(activation.row, '.parts-browser')) {
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
                var containers = toArray(hint.querySelectorAll('.binding-container')).filter(visibleVueElement);
                var bindings = containers.map(getBindingFriendlyName).filter(Boolean);
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

          function updateVuePauseHints(root, activityChanged) {
            var hintSet = vuePauseHintSet(root);
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

          function vueControlState(control, row) {
            var out = [];
            if (!control) control = row;
            var role = control && control.getAttribute ? control.getAttribute('role') : '';
            var checked = control && control.getAttribute ? control.getAttribute('aria-checked') : null;
            if (checked === null && control && (control.type === 'checkbox' || control.type === 'radio')) checked = control.checked ? 'true' : 'false';
            if (checked === null && row && row.querySelector && closest(row, '.options-item-checkbox')) {
              var optionsToggle = row.querySelector('.options-checkbox-toggle, .bng-switch-on');
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
              if (control.value !== undefined && control.value !== '' && control.type !== 'checkbox' && control.type !== 'radio') value = control.value;
              if (!value && control.getAttribute) value = control.getAttribute('aria-valuetext') || control.getAttribute('aria-valuenow') || '';
            }
            if (!value && row && row.querySelector) {
              var valueEl = row.querySelector('.dropdown-display, .options-item-value, .current-value, .value, output, input, select, [aria-valuetext], [aria-valuenow]');
              if (valueEl) value = valueEl.value !== undefined && valueEl.value !== '' ? valueEl.value : (valueEl.getAttribute('aria-valuetext') || valueEl.getAttribute('aria-valuenow') || valueEl.innerText || '');
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
            var toggle = row.querySelector('.options-checkbox-toggle, .bng-switch-on');
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
            var attr = el.getAttribute && (el.getAttribute('aria-label') || el.getAttribute('data-label') || el.getAttribute('data-name') || el.getAttribute('data-tooltip') || el.title);
            if (attr) return cleanText(attr);
            if (el.__bngTooltip && el.__bngTooltip.text) return cleanText(el.__bngTooltip.text);
            var label = el.querySelector && el.querySelector('.bng-row-label, .bng-accitem-caption-content, .pack-title, .tile-label, .label, label, .variable-title, .saveload-metadata-caption, .bng-card-heading');
            return cleanText((label && label.innerText) || vueOwnLabel(el));
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

            var partRow = closest(target, '.parts-browser .bng-accitem');
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
              out.push(vehicleConfigLabel(tuningRow));
              var tuningInput = q(tuningRow, 'input[data-testid="input"], input[type="range"], input[type="number"]');
              var tuningValue = tuningInput ? cleanText(tuningInput.value) : '';
              var tuningUnit = cleanText((q(tuningRow, '[data-testid="suffix"], .suffix, .unit') || {}).innerText);
              if (tuningValue) out.push(tuningValue + (tuningUnit ? ' ' + tuningUnit : ''));
              if ((tuningInput && tuningInput.disabled) || tuningRow.getAttribute('aria-disabled') === 'true') out.push('disabled');
              if (focusEntry) {
                if (_tuningHintTimer) clearTimeout(_tuningHintTimer);
                var tip = tuningRow.__bngTooltip;
                var hint = cleanText(tip && tip.text);
                if (hint && hint.toLowerCase() !== vehicleConfigLabel(tuningRow).toLowerCase()) {
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
            return true;
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

          subscribe($rootScope, 'UINavigation', function (_event, action, value) {
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
            var value = control && control.value !== undefined && control.value !== '' ? control.value : '';
            if (!value && control && control.getAttribute) value = control.getAttribute('aria-valuetext') || control.getAttribute('aria-valuenow') || '';
            if (!value && row && row.querySelector) {
              var valueEl = row.querySelector('.dropdown-display, .current-value, .options-item-value, output, ' +
                'input[type=number], input[type=range], select, [aria-valuetext], [aria-valuenow]');
              if (valueEl) value = valueEl.value !== undefined && valueEl.value !== '' ? valueEl.value :
                (valueEl.getAttribute('aria-valuetext') || valueEl.getAttribute('aria-valuenow') || valueEl.innerText || '');
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

          function speakVueScreen(element, src) {
            var bindingPopup = vueBindingEditorPopup();
            if (bindingPopup && speakVueBindingEditor(element, src, bindingPopup)) return true;
            var root = vueScreenRoot();
            if (!root) return false;
            if (speakVueVehicleConfig(element, src, root, _vueConfigFocusEntry)) return true;
            if (speakVueVehicleSelector(element, src, root)) return true;
            if (speakVueOptions(element, src, root)) return true;
            return speakVuePause(element, src, root);
          }

          function vueFocusSignature(el, root) {
            if (!el) return '';
            var bindingPopup = vueBindingEditorPopup();
            if (bindingPopup && bindingPopup.contains(el)) return vueBindingEditorSignature(el, bindingPopup);
            var screenKind = vehicleConfigRoot(root) ? 'vehicle-config' :
              (root.querySelector('.vehicle-grid, .grid-selector-screen-content, .grid-content') ? 'vehicle-selector' :
              (root.querySelector('.options-view, .options-categories, .options-category-button, .options-content-wrapper, .options-item-label-text, .options-toc, #binding_list, .binding-item-row') ? 'options' : 'pause'));
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
              if (!root) {
                clearPartsDropdownActivation(true);
                resetVueHintLifecycle();
                _vueWatchScreen = null; _vueWatchSignature = ''; _vueWatchElement = null;
                _vueOptionsCategory = '';
                _vueOptionsCategoryGeneration++;
                if (_vueOptionsCategoryTimer) { clearTimeout(_vueOptionsCategoryTimer); _vueOptionsCategoryTimer = null; }
                scheduleVuePoll(nextDelay);
                return;
              }
              nextDelay = 120;
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
              var activeTab = root.querySelector('[role="tab"][aria-selected="true"], .bng-tab.active, .bng-tab.selected');
              var subScreen = root.querySelector('.adjustment-container, .parts-packs, .parts-browser, .innerTuningCard, .paint-acc-wrapper, .saveload');
              var screenKey = (location.hash || '') + '|' + (location.pathname || '') + '|' +
                cleanText(activeTab && activeTab.innerText) + '|' + (subScreen ? subScreen.className.toString() : '') + '|' +
                (dialog ? ((dialog.id || '') + ':' + (dialog.className || '').toString()) : 'screen');
              if (screenKey !== _vueWatchScreen) {
                if (_vueWatchScreen !== null) clearPartsDropdownActivation(true);
                resetVueHintLifecycle();
                _vueWatchScreen = screenKey; _vueWatchSignature = ''; _vueWatchElement = null; lastFocusedElement = null;
                _lastTuningCategory = null; _lastTuningSubcategory = null;
                if (_tuningHintTimer) { clearTimeout(_tuningHintTimer); _tuningHintTimer = null; }
              }
              var activatedDropdownFocused = updatePartsDropdownActivation(root);
              var dropdownFocused = focusedVuePartsDropdownOption(activeVuePartsDropdown());
              var focused = vueBindingEditorFocused(bindingPopup) || activatedDropdownFocused || dropdownFocused || focusedVueMainMenuItem(root) || root.querySelector('.focus-visible') ||
                (root.contains(document.activeElement) ? document.activeElement : null) ||
                document.querySelector('.bng-dropdown-content .dropdown-option.focus-visible');
              if (!focused) {
                updateVuePauseHints(root, false);
                scheduleVuePoll(nextDelay);
                return;
              }
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
                if (speakBindingEditItem(element, src)) return;
                if (speakBindingElement(element, src)) return;
                if (speakMenuAccordionItem(element, src)) return;
                scheduleSpeak(extractText(element), src);
              }, 75);
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
                if (speakBindingEditItem(element, src)) return;
                if (speakBindingElement(element, src)) return;
                if (speakMenuAccordionItem(element, src)) return;
                scheduleSpeak(extractText(element), src);
              }, 75);
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

          // ---------- EVENT HOOKS AND INITIALIZATION ----------
          function initializeModules() {
            log("info", "[bnvda] Page loaded. Initializing modules...");
            toasterInterval = trackedSetInterval(attachToasterPatcher, 1000);
            startBindingEditWatcher();
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
            scheduleSpeak(extractText(e.target), P.POINTER);
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
