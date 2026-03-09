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
            if (hasNonAscii(txt)) {
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
            var cleaned0 = cleanText(rawText);
            if (cleaned0.length > 0 && cleaned0.length <= 2) {
              var parent = el.parentElement;
              var parentText = parent ? cleanText(parent.innerText || '') : '';
              log('info', '[SHORTTEXT] "' + cleaned0 + '" tag=' + el.tagName + ' class=' + (el.className || '').toString().substring(0, 80) + ' parentTag=' + (parent ? parent.tagName : 'none') + ' parentText="' + (parentText || '').substring(0, 120) + '" outerHTML=' + el.outerHTML.substring(0, 300));
            }
            if (rawText.startsWith('{{') && rawText.endsWith('}}')) {
              try {
                var scope = angular.element(targetElement).scope();
                if (scope) {
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
            return function(key) { return key.substring(key.lastIndexOf('.') + 1).replace(/_/g, ' '); };
          }
          function speakOptionRow(focusedElement, src) {
            var optionRow = closest(focusedElement, 'md-list-item');
            if (!optionRow) return false;
            var ngScope;
            try { ngScope = window.angular.element(optionRow).scope(); } catch(e) { return false; }
            if (!ngScope) return false;
            var optionsData = (ngScope.options && ngScope.options.data) || (ngScope.$parent && ngScope.$parent.options && ngScope.$parent.options.data);
            if (!optionsData) {
              var simpleTextElem = optionRow.querySelector('h3, p, md-button');
              if (simpleTextElem) {
                 var simpleText = cleanText(simpleTextElem.innerText);
                 if (simpleText) { scheduleSpeak(simpleText, src); return true; }
              }
              return false;
            }
            var translator = findTranslateFunc();
            var parts = [];
            var labelElem = optionRow.querySelector('p');
            if (labelElem) {
              var labelMatch = labelElem.innerHTML.match(/['"](ui\.(?:options|debug|inputActions)\.[^'"]+)['"]/);
              if (labelMatch && labelMatch[1]) {
                parts.push(cleanText(translator(labelMatch[1])));
              } else {
                 var cleanedLabel = cleanText(labelElem.innerText);
                 if (cleanedLabel) parts.push(cleanedLabel);
              }
            }
            var inputElem = optionRow.querySelector('md-select, md-checkbox, md-slider');
            if (inputElem && inputElem.hasAttribute('ng-model')) {
              var modelString = inputElem.getAttribute('ng-model');
              var currentValue;
              try { currentValue = ngScope.$eval(modelString); } catch(e) { currentValue = '[error]'; }
              var valueText = '';
              var tagName = inputElem.tagName.toLowerCase();
              if (tagName === 'md-select') {
                var optionsPath = modelString.replace('.values.', '.options.') + '.modes';
                var modes;
                try { modes = ngScope.$eval(optionsPath); } catch (e) { modes = null; }
                if (modes && modes.keys && modes.values) {
                  var valueIndex = modes.keys.indexOf(currentValue);
                  if (valueIndex !== -1 && modes.values[valueIndex]) { valueText = translator(modes.values[valueIndex]); }
                }
              } else if (tagName === 'md-checkbox') {
                valueText = currentValue ? translator('ui.common.on') : translator('ui.common.off');
              } else if (tagName === 'md-slider') {
                var valueDisplay = optionRow.querySelector('span:not([flex]), input[type=number]');
                if (valueDisplay) { valueText = valueDisplay.tagName.toLowerCase() === 'input' ? valueDisplay.value : valueDisplay.innerText.trim(); }
              }
              if (!valueText) valueText = String(currentValue);
              parts.push(cleanText(valueText));
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
          function processAndSpeakMessage(payload) {
            var finalText = '';
            if (typeof payload === 'string') { finalText = stripHtml(payload); }
            else if (typeof payload === 'object' && payload !== null && payload.txt) { var translator = findTranslateFunc(); finalText = translator(payload.txt, payload.context); }
            else if (typeof payload === 'object' && payload !== null) { log('warn', '[UNSPEAKABLE] object without .txt: ' + JSON.stringify(payload)); return; }
            else { log('warn', '[UNSPEAKABLE] unexpected payload type (' + typeof payload + '): ' + String(payload)); return; }
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

          // ========== GENERIC UI CONTROL HANDLERS ==========
          function speakCheckboxRow(focusedElement, src) {
            var row = closest(focusedElement, 'md-list-item');
            if (!row) return false;
            var checkboxEl = row.querySelector('md-checkbox');
            if (!checkboxEl) return false;
            var parts = [];
            var labelEl = row.querySelector('p');
            if (labelEl) { parts.push(cleanText(labelEl.innerText)); }
            var isChecked = checkboxEl.classList.contains('md-checked');
            parts.push(isChecked ? 'checked' : 'unchecked');
            var finalText = parts.join(', ');
            scheduleSpeak(finalText, src);
            return true;
          }

          function speakSliderRow(focusedElement, src) {
            var row = closest(focusedElement, 'md-list-item');
            if (!row) return false;
            var sliderEl = row.querySelector('md-slider');
            if (!sliderEl) return false;
            var parts = [];
            var labelEl = row.querySelector('p');
            if (labelEl) { parts.push(cleanText(labelEl.innerText)); }
            var valueEl = row.querySelector('input[type="number"], span.md-body-1');
            if (valueEl) {
              var value = valueEl.tagName.toLowerCase() === 'input' ? valueEl.value : valueEl.innerText;
              parts.push(cleanText(value));
            } else {
              var ariaValue = sliderEl.getAttribute('aria-valuenow');
              if (ariaValue) { parts.push(cleanText(ariaValue)); }
            }
            if (parts.length >= 2) {
              var finalText = parts.join(', ');
              scheduleSpeak(finalText, src);
              return true;
            }
            return false;
          }
          
          // ========== Specialized handler for Vehicle Tuning Sliders (Vue.js version) ==========
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
              scheduleSpeak(finalText, src);
              return true;
            }
            return false;
          }

          // ========== Specialized handler for Vehicle Parts Screen ==========
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
              var finalText = parts.join(', ');
              scheduleSpeak(finalText, src);
              return true;
            }
            return false;
          }


          // ========== CONTROLS BINDINGS NAVIGATION FIX ==========
          // Crossfire's isTarget scoring favors elements with high horizontal
          // overlap. The small right-aligned [ng-click] binding buttons inside
          // accordion panes have minimal overlap with full-width pane headers,
          // so navigation jumps header-to-header, skipping all bindings.
          // Fix: add bng-nav-item to the individual interactive elements inside
          // each row (label, each binding, add button) so crossfire can navigate
          // left/right across columns and up/down between rows.
          function patchBindingsNavigation() {
            var bindingList = document.getElementById('binding_list');
            if (!bindingList) return;
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
            }, 300);
          }

          // ========== RADIAL MENU SPEECH MODULE ==========
          var _radialMenuWasOpen = false;
          var _radialDefaultText = '';
          var _radialLastSpokenItem = '';
          var _radialLastCategory = '';
          var _radialItemObserver = null;
          var _radialCategoryObserver = null;
          var _radialLabelDiv = null;
          var _radialHotkeyDiv = null;

          function findRadialInfoElements(container) {
            var svgEl = container.querySelector('.radial-svg svg');
            if (!svgEl) return null;
            var fo = svgEl.querySelector('foreignObject');
            if (!fo) return null;
            var body = fo.firstElementChild;
            if (!body) return null;
            var wrap = body.firstElementChild;
            if (!wrap || !wrap.children || wrap.children.length < 5) return null;
            return { label: wrap.children[2], hotkey: wrap.children[4] };
          }

          function radialMenuOnOpen(container) {
            var attempts = 0;
            function tryAttach() {
              var els = findRadialInfoElements(container);
              if (!els) {
                attempts++;
                if (attempts < 3) { setTimeout(tryAttach, 50); }
                return;
              }
              _radialLabelDiv = els.label;
              _radialHotkeyDiv = els.hotkey;
              _radialDefaultText = (_radialLabelDiv.textContent || '').trim();
              scheduleSpeak('Radial menu', P.SYSTEM);

              var selectedCat = container.querySelector('.radial-category.selected .radial-category-label');
              if (selectedCat) {
                var catText = cleanText(selectedCat.textContent);
                if (catText) {
                  _radialLastCategory = catText;
                  setTimeout(function() { send({ type: "speak", text: catText }); }, 100);
                }
              }

              _radialItemObserver = new MutationObserver(function() {
                var text = (_radialLabelDiv.textContent || '').trim();
                if (!text || text === _radialDefaultText || text === _radialLastSpokenItem) return;
                _radialLastSpokenItem = text;
                var hotkeyText = _radialHotkeyDiv ? (_radialHotkeyDiv.textContent || '').trim() : '';
                if (hotkeyText) {
                  var parts = hotkeyText.split(/\s+/);
                  hotkeyText = parts.length > 1 ? parts.slice(1).join(' ') : hotkeyText;
                }
                var speakText = hotkeyText ? text + ', ' + hotkeyText + ' key' : text;
                scheduleSpeak(speakText, P.CONTROLLER);
              });
              _radialItemObserver.observe(_radialLabelDiv, { childList: true, characterData: true, subtree: true });

              var categoriesContainer = container.querySelector('.radial-categories');
              if (categoriesContainer) {
                _radialCategoryObserver = new MutationObserver(function() {
                  var sel = container.querySelector('.radial-category.selected .radial-category-label');
                  if (!sel) return;
                  var catText = cleanText(sel.textContent);
                  if (catText && catText !== _radialLastCategory) {
                    _radialLastCategory = catText;
                    scheduleSpeak(catText, P.CONTROLLER);
                  }
                });
                _radialCategoryObserver.observe(categoriesContainer, { attributes: true, subtree: true, attributeFilter: ['class'] });
              }

              log('info', '[bnvda] Radial menu observers attached.');
            }
            tryAttach();
          }

          function radialMenuOnClose() {
            if (_radialItemObserver) { _radialItemObserver.disconnect(); _radialItemObserver = null; }
            if (_radialCategoryObserver) { _radialCategoryObserver.disconnect(); _radialCategoryObserver = null; }
            _radialDefaultText = '';
            _radialLastSpokenItem = '';
            _radialLastCategory = '';
            _radialLabelDiv = null;
            _radialHotkeyDiv = null;
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

          // ---------- Non-Invasive Observer for UI Changes ----------
          var mainObserver = null;
          var lastFocusedElement = null;
          var focusDebounceTimer = null;

          function processFocusChange(element) {
            clearTimeout(focusDebounceTimer);
            focusDebounceTimer = setTimeout(function() {
              if (!element || element === lastFocusedElement) return;
              lastFocusedElement = element;
              
              var src = P.CONTROLLER;

              // --- Primary Logic ---
              if (speakTuningControl(element, src)) return;
              if (speakPartRow(element, src)) return;
              if (speakCheckboxRow(element, src)) return;
              if (speakSliderRow(element, src)) return;
              if (speakBindingEditItem(element, src)) return;
              if (speakBindingElement(element, src)) return;
              if (optionsObserverAttached && speakOptionRow(element, src)) return;

              scheduleSpeak(extractText(element), src);
            }, 75);
          }

          function attachMainObserver() {
            if (mainObserver) return;
            var targetNode = document.body;
            if (!targetNode) return;

            var observerConfig = {
              attributes: true,
              subtree: true,
              attributeFilter: ['class']
            };

            var callback = function(mutationsList, observer) {
              for(var mutation of mutationsList) {
                if (mutation.type === 'attributes' && mutation.attributeName === 'class') {
                  var targetElement = mutation.target;
                  if (targetElement && typeof targetElement.classList.contains === 'function' && targetElement.classList.contains('focus-visible')) {
                    processFocusChange(targetElement);
                  }
                }
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

          // ---------- EVENT HOOKS AND INITIALIZATION ----------
          function initializeModules() {
            log("info", "[bnvda] Page loaded. Initializing modules...");
            optionsInterval = setInterval(attachOptionsObserver, 1000);
            toasterInterval = setInterval(attachToasterPatcher, 1000);
            setInterval(patchBindingsNavigation, 500);
            startBindingEditWatcher();
            attachMainObserver();
            startRadialMenuWatcher();
          }

          connectWS();

          addEventListener("mouseover", function (e) {
            if ((nowTS() - lastControllerTs) < CONTROLLER_DOMINANCE_MS) return;
            scheduleSpeak(extractText(e.target), P.POINTER);
          }, true);
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