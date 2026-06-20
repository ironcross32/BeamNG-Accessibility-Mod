angular.module('beamng.apps')
.directive('bnvdaHook', ['$rootScope', function ($rootScope) {
  return {
    template: '<div style="width:1px;height:1px;opacity:0;pointer-events:none;"></div>',
    replace: true,
    restrict: 'EA',
    link: function (scope, element, attrs) {
      if (window.__BNvDA_INSTALLED__) return;
      window.__BNvDA_INSTALLED__ = true;

      try {
        (function () {
          "use strict";

          // ---------- CONFIG ----------
          var WS_URL = "ws://127.0.0.1:8765";
          var DEBOUNCE_MS = 50;
          var CONTROLLER_DOMINANCE_MS = 900;
          var MIN_CHARS = 2;
          var MAX_LEN = 160;
          var DEBUG = !!window.BNVDA_DEBUG;

          // ---------- WS ----------
          var ws = null;
          function send(obj) {
            if (ws && ws.readyState === 1) {
              try { ws.send(JSON.stringify(obj)); } catch (e) {}
            }
          }
          function log(level, msg) { send({ type: "log", level: level, msg: msg }); }
          function connectWS() {
            try {
              ws = new WebSocket(WS_URL);
              ws.onopen = function () { window._bnvdaWS = ws; log("info", "[bnvda] WebSocket connected."); };
              ws.onclose = function () { if (window._bnvdaWS === ws) window._bnvdaWS = null; setTimeout(connectWS, 2000); };
              ws.onerror = function () { log("error", "[bnvda] WebSocket connection error."); };
              ws.onmessage = function (evt) {
                try {
                  var data = JSON.parse(evt.data);
                  if (data.type === 'context_action') {
                    handleContextAction(data.action);
                  } else if (data.type === 'dom_dump') {
                    performDomDump();
                  }
                } catch (e) {}
              };
            } catch (e) { setTimeout(connectWS, 2500); }
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
                lastFunc = setTimeout(function() {
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
          function q(el, sel) { try { return el.querySelector(sel); } catch (e) { return null; } }

          var P = { POINTER: 1, KEYBOARD: 2, CONTROLLER: 3, SYSTEM: 4 };
          var lastSpoken = "", lastSource = 0, lastSpeakTs = 0, lastControllerTs = 0, speakTimer = null;
          var _lastOpenedSelect = null;


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
            if (looksLikeCss(txt)) return;
            if (DEBUG && hasNonAscii(txt)) {
              log('info', '[GLYPH] "' + txt + '" chars=' + charDump(txt));
            }
            var t = nowTS();
            if (src === P.CONTROLLER) lastControllerTs = t;
            if (src === P.POINTER && (t - lastControllerTs) < CONTROLLER_DOMINANCE_MS) return;
            if (txt === lastSpoken && src <= lastSource && (t - lastSpeakTs) < 400) return;
            if (speakTimer) try { clearTimeout(speakTimer); } catch (e) {}
            speakTimer = setTimeout(function () {
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
            return closest(el, "[role='option'],[role='menuitem'],[role='treeitem'],[role='tab'],[role='button'],md-option,md-tab-item,button,a,[tabindex],[class*='vehicle'],[class*='variant'],[class*='config'],[class*='part'],[class*='slot'],[class*='item'],[class*='entry'],[class*='tile'],[class*='card']");
          }
          
          function extractText(el) {
            if (!el) return "";
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
            if (mdSel) {
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
            var n = inter.parentElement, hops = 0;
            while (n && hops < 2) {
              var via = cleanText((n.innerText || n.textContent || "")); if (via) return via; n = n.parentElement; hops++;
            }
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
                    if (translatedHelp && translatedHelp.length > 2) { setTimeout(function() { send({ type: "speak", text: cleanText(translatedHelp) }); }, 750); }
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
                var translated = translator(payload);
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
              finalText = translator(payload.txt, ctx);
            }
            else if (typeof payload === 'object' && payload !== null) { log('warn', '[UNSPEAKABLE] object without .txt: ' + JSON.stringify(payload)); return; }
            else { log('warn', '[UNSPEAKABLE] unexpected payload type (' + typeof payload + '): ' + String(payload)); return; }
            // Replace [action=XXX] tokens with friendly action names
            finalText = finalText.replace(/\[action=([^\]]+)\]/g, function(match, actionName) {
              // Try translation key first (e.g. ui.inputActions.vehicle.clutch.title)
              var translator = findTranslateFunc();
              var segments = actionName.split('.');
              var candidates = [
                'ui.inputActions.vehicle.' + actionName + '.title',
                'ui.inputActions.' + actionName + '.title'
              ];
              for (var i = 0; i < candidates.length; i++) {
                var result = translator(candidates[i]);
                if (result && result !== candidates[i]) return result;
              }
              // Fallback: humanize camelCase (e.g. "activateStarterMotor" -> "Activate Starter Motor")
              var humanized = actionName.replace(/([a-z])([A-Z])/g, '$1 $2');
              // Also split on dots and take last segment
              if (humanized.indexOf('.') !== -1) humanized = humanized.substring(humanized.lastIndexOf('.') + 1);
              return humanized.charAt(0).toUpperCase() + humanized.slice(1);
            });
            finalText = cleanText(finalText);
            if (finalText.toLowerCase() === 'switched' && (nowTS() - lastCameraSwitchTs) < 250) { return; }
            if (finalText && finalText.length >= MIN_CHARS) { scheduleSpeak(finalText, P.SYSTEM); }
            else if (payload && !finalText) { log('warn', '[UNSPEAKABLE] empty after processing: ' + JSON.stringify(payload)); }
          }

          // ---------- GLOBAL EVENT LISTENERS ----------
          $rootScope.$on('Message', function (event, args) {
            if (!args || !args.msg) return;
            try { var injector = angular.element(document.body).injector(); if (injector) { var toasterService = injector.get('MessageToasterService'); if (toasterService.activeCategories.includes(args.category)) return false; } } catch (e) {}
            processAndSpeakMessage(args.msg);
          });
          $rootScope.$on('DamageMessage', function (event, args) {
            if (!args || !args.damageText) return;
            var translator = findTranslateFunc();
            var translatedText = translator(args.damageText);
            if (translatedText && translatedText.length >= MIN_CHARS) { scheduleSpeak(cleanText(translatedText), P.SYSTEM); }
          });
          $rootScope.$on('onCameraNameChanged', function (event, data) {
            if (data && data.name) {
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

          $rootScope.$on('SettingsChanged', function (event, data) {
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

          $rootScope.$on('CruiseControlState', function (event, data) {
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
              // Bare <div> consent rows (online/telemetry): delegate to speakOptionRow
              // which knows how to walk a <div> container for labels.
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
                _tuningHintTimer = setTimeout(function () {
                  _tuningHintTimer = null;
                  send({ type: "speak", text: hint, interrupt: false });
                }, 250);
              }
              return true;
            }
            return false;
          }

          // ========== Specialized handler for Vehicle Parts Screen ==========
          var _partsExpandHinted = false;
          function speakPartRow(focusedElement, src) {
            var row = closest(focusedElement, '.bng-accitem');
            if (!row) return false;
            if (!closest(row, '.parts-browser')) return false;
            var slotNameEl = row.querySelector('.bng-accitem-caption-content');
            var equippedPartEl = row.querySelector('.dropdown-display');
            var visibilityButton = row.querySelector('.visibility-toggle');
            if (slotNameEl) {
              var parts = [];
              parts.push(cleanText(slotNameEl.innerText));
              if (equippedPartEl) {
                var equippedText = cleanText(equippedPartEl.innerText);
                if (equippedText.toLowerCase() === 'empty') {
                  parts.push("slot is empty");
                } else {
                  parts.push("currently equipped: " + equippedText);
                }
              }
              if (visibilityButton) {
                var isVisible = visibilityButton.classList.contains('visibility-toggle-on');
                parts.push(isVisible ? "visible" : "hidden");
              }
              var isExpandable = row.classList.contains('bng-accitem-expandable');
              if (isExpandable) {
                var isExpanded = row.classList.contains('bng-accitem-expanded');
                parts.push(isExpanded ? "expanded" : "collapsed");
                if (!_partsExpandHinted) {
                  _partsExpandHinted = true;
                  parts.push("press Y to expand or collapse");
                }
              }
              var finalText = parts.join(', ');
              scheduleSpeak(finalText, src);
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
              var icons = window.bngVue && window.bngVue.icons;
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

          function getBindingFriendlyName(bindingEl) {
            try {
              if (!_glyphToName) buildGlyphMap();
              // Check for modifier combo: multiple <kbd>/<div> children inside the binding
              var parts = bindingEl.querySelectorAll(':scope > kbd, :scope > div');
              if (parts.length > 1) {
                var names = [];
                for (var i = 0; i < parts.length; i++) {
                  var n = resolveSingleBindingPart(parts[i]);
                  if (n) names.push(n);
                }
                if (names.length > 0) return names.join(' + ');
              }
              // Single binding: try the whole element
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
            setInterval(function() {
              try {
                // The edit panel lives in [ui-view="edit"] and has .controls-edit
                var editPanel = document.querySelector('[ui-view="edit"] .controls-edit');
                var isOpen = !!editPanel;

                if (isOpen && !_bindingEditWasOpen) {
                  _bindingEditWasOpen = true;
                  _bindingEditHandled = false;
                }
                if (!isOpen && _bindingEditWasOpen) {
                  _bindingEditWasOpen = false;
                  _bindingEditHandled = false;
                  return;
                }
                if (!isOpen || _bindingEditHandled) return;

                // Wait until capture is done: cancel button is visible
                var cancelBtn = document.getElementById('binding_edit_cancel');
                if (!cancelBtn) return;

                _bindingEditHandled = true;
                handleBindingEditResult(editPanel, cancelBtn);
              } catch (e) {
                log('info', '[BIND-EDIT] Watcher error: ' + e.message);
              }
            }, 200);
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

              setTimeout(function() {
                if (firstConflictP) firstConflictP.focus();
              }, 100);
            } else {
              cancelBtn.focus();
              setTimeout(function() {
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

          // ========== Vehicle Details Watcher ==========
          // Watches for .details-content inside the Vue vehicle selector,
          // parses children into structured lines, and sends them via WS
          // for arrow-key browsing in beamtel.py. Also detects vehicle
          // selector open/close state via breadcrumbs.
          var _detailsCheckInterval = null;
          var _lastDetailLines = [];
          var _vehicleSelectorWasOpen = false;
          var _detailsSendTimer = null;

          function arraysEqual(a, b) {
            if (a.length !== b.length) return false;
            for (var i = 0; i < a.length; i++) {
              if (a[i] !== b[i]) return false;
            }
            return true;
          }

          var _detailsFilterWords = ['spawn new', 'cancel', 'spawn', 'select'];
          function parseDetailLines(detailsEl) {
            var raw = (detailsEl.innerText || '').trim();
            if (!raw) return [];
            var split = raw.split(/\n/);
            var lines = [];
            for (var i = 0; i < split.length; i++) {
              var line = split[i].replace(/\s+/g, ' ').trim();
              if (!line || line.length < 2) continue;
              var lower = line.toLowerCase();
              var dominated = false;
              for (var f = 0; f < _detailsFilterWords.length; f++) {
                if (lower === _detailsFilterWords[f]) { dominated = true; break; }
              }
              if (!dominated) lines.push(line);
            }
            return lines;
          }

          function isVehicleSelectorOpen() {
            try {
              var bc = document.querySelector('.bng-path.header-breadcrumbs');
              if (bc && bc.innerText && bc.innerText.indexOf('Vehicle Selector') !== -1) return true;
            } catch (e) {}
            return false;
          }

          function startVehicleDetailsWatcher() {
            if (_detailsCheckInterval) return;
            _detailsCheckInterval = setInterval(function() {
              try {
                var selectorOpen = isVehicleSelectorOpen();
                if (selectorOpen !== _vehicleSelectorWasOpen) {
                  _vehicleSelectorWasOpen = selectorOpen;
                  send({ type: "vehicle_selector_state", open: selectorOpen });
                  if (!selectorOpen) {
                    _lastDetailLines = [];
                  }
                }
                if (!selectorOpen) return;

                var detailsEl = document.querySelector('.details-content');
                if (!detailsEl) {
                  if (_lastDetailLines.length > 0) _lastDetailLines = [];
                  return;
                }
                var lines = parseDetailLines(detailsEl);
                if (lines.length > 0 && !arraysEqual(lines, _lastDetailLines)) {
                  _lastDetailLines = lines;
                  if (_detailsSendTimer) clearTimeout(_detailsSendTimer);
                  var snapshot = lines.slice();
                  _detailsSendTimer = setTimeout(function() {
                    send({ type: "vehicle_details", lines: snapshot });
                  }, 500);
                }
              } catch (e) {
                log('info', '[bnvda] Vehicle details watcher error: ' + e.message);
              }
            }, 1000);
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
                if (attempts < 10) { _radialAttachTimer = setTimeout(tryAttach, 100); }
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
                  setTimeout(function() { send({ type: "speak", text: catText }); }, 100);
                }
              }

              // Poll for changes — Vue's virtual DOM patching doesn't reliably
              // trigger MutationObserver, so we poll every 80ms instead.
              _radialPollTimer = setInterval(function() {
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
            setInterval(function() {
              try {
                var container = document.querySelector('.radial-menu');
                var isOpen = !!container;
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
            }, 200);
          }

          // ========== TUNING SLIDER ACCELERATION ==========
          // BeamNG's vehicle tuning sliders (BngSlider with the property-slider class)
          // change by a single `step` per D-pad repeat (~10/sec after a 400ms hold).
          // Mods can define tiny steps over wide ranges, so a change like final drive
          // 4.1 -> 3.55 takes many seconds of held input. This module watches the
          // focused slider and, while a direction is held, injects extra steps so the
          // effective increment grows the longer the control is held. It also speaks
          // the value as it changes, since the game gives no audio feedback while
          // adjusting and acceleration would otherwise make overshooting easy.
          //
          // The D-pad path updates the slider through a Vue directive (callback -> ref)
          // and fires NO DOM 'input' event, so we poll the focused range input rather
          // than listen. To apply our own change we set the range value and dispatch a
          // native 'input' event, which drives the slider's v-model + notify() exactly
          // like a real interaction (updating the Vue ref and committing the change).
          var TUNE_POLL_MS = 40;
          var TUNE_IDLE_RESET_TICKS = 6;   // ~240ms with no change ends a "hold"
          var TUNE_HOLD_GAP_TICKS = 4;     // <=160ms gap still counts as the same hold
          var TUNE_SPEAK_INTERVAL = 150;   // min ms between spoken value updates
          // Step multiplier keyed by consecutive held repeats (extra = mult - 1).
          function tuneMultiplier(count) {
            if (count < 3) return 1;
            if (count < 8) return 2;
            if (count < 15) return 4;
            if (count < 25) return 8;
            if (count < 40) return 16;
            return 32;
          }
          var _tune = {
            input: null, lastValue: null, lastDir: 0, count: 0,
            idleTicks: 0, pendingFinal: false, lastSpeakTs: 0, lastSpokenVal: null
          };
          function tuneRoundToStep(val, step) {
            var s = String(step);
            var dot = s.indexOf('.');
            var decimals = dot >= 0 ? (s.length - dot - 1) : 0;
            var p = Math.pow(10, decimals);
            return Math.round(val * p) / p;
          }
          function tuneSetRangeValue(el, val) {
            try {
              var desc = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
              if (desc && desc.set) desc.set.call(el, String(val));
              else el.value = String(val);
            } catch (e) { el.value = String(val); }
            el.dispatchEvent(new Event('input', { bubbles: true }));
          }
          function tuneFocusedRange() {
            var cont = document.querySelector('.bng-slider-container.property-slider.focus-visible');
            if (cont) return cont.querySelector('input.bng-slider[type="range"]');
            // In some states focus-visible lands on the inner range input itself.
            return document.querySelector('.property-slider input.bng-slider.focus-visible');
          }
          function tuneAnnounce(el, force) {
            var step = parseFloat(el.step) || 0;
            var raw = parseFloat(el.value);
            if (isNaN(raw)) return;
            var disp = step > 0 ? tuneRoundToStep(raw, step) : raw;
            if (disp === _tune.lastSpokenVal) return; // never repeat the same value
            var now = nowTS();
            if (!force && (now - _tune.lastSpeakTs) < TUNE_SPEAK_INTERVAL) return;
            _tune.lastSpeakTs = now;
            _tune.lastSpokenVal = disp;
            var cont = closest(el, '.bng-slider-container');
            var suffixEl = cont ? cont.querySelector('[data-testid="suffix"]') : null;
            var unit = suffixEl ? cleanText(suffixEl.innerText) : '';
            var txt = unit ? (disp + ' ' + unit) : String(disp);
            scheduleSpeak(cleanText(txt), P.CONTROLLER);
          }
          function tuneReset(el) {
            _tune.input = el || null;
            _tune.lastValue = el ? parseFloat(el.value) : null;
            _tune.lastDir = 0;
            _tune.count = 0;
            _tune.idleTicks = 0;
            _tune.pendingFinal = false;
          }
          function startTuningSliderAcceleration() {
            setInterval(function () {
              try {
                var el = tuneFocusedRange();
                if (!el) { if (_tune.input) tuneReset(null); return; }
                if (el !== _tune.input) { tuneReset(el); return; }

                var v = parseFloat(el.value);
                if (isNaN(v)) return;

                if (v === _tune.lastValue) {
                  _tune.idleTicks++;
                  if (_tune.idleTicks >= TUNE_IDLE_RESET_TICKS) {
                    if (_tune.pendingFinal) { tuneAnnounce(el, true); _tune.pendingFinal = false; }
                    _tune.count = 0;
                    _tune.lastDir = 0;
                  }
                  return;
                }

                // Value moved since the last poll.
                var dir = v > _tune.lastValue ? 1 : -1;
                if (dir === _tune.lastDir && _tune.idleTicks <= TUNE_HOLD_GAP_TICKS) {
                  _tune.count++;
                } else {
                  _tune.count = 0; // direction change or a fresh tap after release
                }
                _tune.lastDir = dir;
                _tune.idleTicks = 0;
                _tune.lastValue = v;
                _tune.pendingFinal = true;

                var step = parseFloat(el.step) || 0;
                if (step > 0) {
                  var mult = tuneMultiplier(_tune.count);
                  var min = el.min !== '' ? parseFloat(el.min) : -Infinity;
                  var max = el.max !== '' ? parseFloat(el.max) : Infinity;
                  // Cap so one tick never crosses more than ~1/15 of the range.
                  if (isFinite(min) && isFinite(max) && max > min) {
                    var cap = Math.max(1, Math.round(Math.abs((max - min) / step) / 15));
                    if (mult > cap) mult = cap;
                  }
                  var extra = mult - 1;
                  if (extra > 0) {
                    var target = v + dir * step * extra;
                    if (target < min) target = min;
                    if (target > max) target = max;
                    target = tuneRoundToStep(target, step);
                    if (target !== v) {
                      tuneSetRangeValue(el, target);
                      _tune.lastValue = target;
                    }
                  }
                }
                tuneAnnounce(el, false);
              } catch (e) {
                log('info', '[TUNE] accel error: ' + e.message);
              }
            }, TUNE_POLL_MS);
            log('info', '[bnvda] Tuning slider acceleration started.');
          }

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
            send({ type: 'nav_inject', dir: dir > 0 ? 1 : -1, count: count });
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
              _dropSpeak.timer = setTimeout(function () {
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
            window.addEventListener('keydown', function (e) {
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
            setInterval(function () {
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

          // ---------- Non-Invasive Observer for UI Changes ----------
          var mainObserver = null;
          var lastFocusedElement = null;
          var focusDebounceTimer = null;   // controller path
          var kbFocusDebounceTimer = null; // keyboard path
          function processFocusChange(element, src) {
            src = (src !== undefined) ? src : P.CONTROLLER;
            // Track the most recently focused md-select so the dropdown-close
            // watcher can re-speak the new value after the user picks one.
            if (element) {
              var sel = closest(element, 'md-select');
              if (sel) _lastOpenedSelect = sel;
            }

            if (src === P.CONTROLLER) {
              // Controller wins: cancel any pending keyboard debounce as well.
              clearTimeout(focusDebounceTimer);
              clearTimeout(kbFocusDebounceTimer);
              focusDebounceTimer = setTimeout(function() {
                if (!element || element === lastFocusedElement) return;
                lastFocusedElement = element;
                if (speakTuningControl(element, src)) return;
                if (speakDropdownOption(element, src)) return;
                if (speakPartRow(element, src)) return;
                if (speakCheckboxRow(element, src)) return;
                if (speakSliderRow(element, src)) return;
                if (speakBindingEditItem(element, src)) return;
                if (speakBindingElement(element, src)) return;
                if (optionsObserverAttached && speakOptionRow(element, src)) return;
                if (speakMenuAccordionItem(element, src)) return;
                scheduleSpeak(extractText(element), src);
              }, 75);
            } else {
              // Keyboard path: separate timer, does not cancel controller timer.
              clearTimeout(kbFocusDebounceTimer);
              kbFocusDebounceTimer = setTimeout(function() {
                // If a controller became active during the debounce window, yield.
                if ((nowTS() - lastControllerTs) < CONTROLLER_DOMINANCE_MS) return;
                if (!element || element === lastFocusedElement) return;
                lastFocusedElement = element;
                if (speakTuningControl(element, src)) return;
                if (speakDropdownOption(element, src)) return;
                if (speakPartRow(element, src)) return;
                if (speakCheckboxRow(element, src)) return;
                if (speakSliderRow(element, src)) return;
                if (speakBindingEditItem(element, src)) return;
                if (speakBindingElement(element, src)) return;
                if (optionsObserverAttached && speakOptionRow(element, src)) return;
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

            mainObserver = new MutationObserver(callback);
            mainObserver.observe(targetNode, observerConfig);
            startVehicleDetailsWatcher();
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
            var focused = document.querySelector('.focus-visible');
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
              if (rect && (rect.width === 0 || rect.height === 0)) return;
              if (node.style && node.style.display === 'none') return;

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
            document.addEventListener('click', function(e) {
              rememberOpenedSelect(e.target);
            }, true);
            document.addEventListener('keydown', function(e) {
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
            selectCloseObserver = new MutationObserver(function(muts) {
              for (var i = 0; i < muts.length; i++) {
                var removed = muts[i].removedNodes;
                for (var j = 0; j < removed.length; j++) {
                  var n = removed[j];
                  if (n && n.nodeType === 1 && n.classList &&
                      n.classList.contains('md-select-menu-container')) {
                    var target = _lastOpenedSelect;
                    setTimeout(function(sel) {
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
            optionsInterval = setInterval(attachOptionsObserver, 1000);
            toasterInterval = setInterval(attachToasterPatcher, 1000);
            setInterval(patchBindingsNavigation, 2000);
            setInterval(patchOptionsNavigation, 1500);
            setInterval(pollActiveElementForOptions, 250);
            startBindingEditWatcher();
            attachMainObserver();
            startRadialMenuWatcher();
            startSelectCloseWatcher();
            startTuningSliderAcceleration();
            startDropdownKeyProbe();
            startDropdownListAcceleration();

          }

          connectWS();

          addEventListener("focusin", function (e) {
            processFocusChange(e.target, P.KEYBOARD);
          }, true);

          addEventListener("mouseover", throttle(function (e) {
            if ((nowTS() - lastControllerTs) < CONTROLLER_DOMINANCE_MS) return;
            scheduleSpeak(extractText(e.target), P.POINTER);
          }, 150), true);
          var pointerMoveHandler = throttle(function (e) {
            if ((nowTS() - lastControllerTs) < CONTROLLER_DOMINANCE_MS) return;
            var el = document.elementFromPoint(e.clientX, e.clientY);
            scheduleSpeak(extractText(el), P.POINTER);
          }, 150);
          addEventListener("pointermove", pointerMoveHandler, { passive: true, capture: true });

          if (document.readyState === 'complete') {
            initializeModules();
          } else {
            window.addEventListener('load', initializeModules, { once: true });
          }
        
        })();
      } catch (e) { log('error', '[bnvda] Main script error: ' + e); }

    }
  };
}]);