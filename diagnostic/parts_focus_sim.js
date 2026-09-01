// Deterministic checks for the parts-screen focus guards in bnvdaRuntime.js.
// Run with: node diagnostic/parts_focus_sim.js

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const runtimePath = path.resolve(__dirname, '..', 'bng_mod', 'ui', 'ui-vue', 'mods', 'bnvda', 'bnvdaRuntime.js');
const source = fs.readFileSync(runtimePath, 'utf8');

let failures = 0;
function check(name, condition, detail) {
  if (condition) console.log('  PASS  ' + name);
  else {
    failures++;
    console.log('  FAIL  ' + name + (detail ? ' -> ' + detail : ''));
  }
}

function extractFunction(name) {
  const marker = 'function ' + name + '(';
  const start = source.indexOf(marker);
  if (start < 0) throw new Error('Missing runtime function: ' + name);
  const open = source.indexOf('{', start);
  let depth = 0;
  let quote = '';
  let escaped = false;
  for (let i = open; i < source.length; i++) {
    const ch = source[i];
    if (quote) {
      if (escaped) escaped = false;
      else if (ch === '\\') escaped = true;
      else if (ch === quote) quote = '';
      continue;
    }
    if (ch === '\'' || ch === '"' || ch === '`') {
      quote = ch;
      continue;
    }
    if (ch === '{') depth++;
    else if (ch === '}' && --depth === 0) return source.slice(start, i + 1);
  }
  throw new Error('Unterminated runtime function: ' + name);
}

function installFunctions(context, names) {
  vm.createContext(context);
  for (const name of names) {
    vm.runInContext(extractFunction(name) + '\nthis.' + name + ' = ' + name + ';', context);
  }
  return context;
}

console.log('\nRuntime integration');
check('native ui_nav listener is capture-phase',
  source.includes("listen(document, 'ui_nav', handlePartsUiNavCapture, true)"));
check('Angular event no longer tries to stop DOM navigation',
  !source.includes('discardPartsDropdownDirection(_event'));
check('dropdown readiness uses the real active option',
  source.includes('var option = activeVuePartsDropdownOption(dropdown);'));
check('visible popup is detected before lazy options render',
  source.includes('if (visibleVueElement(dropdowns[i])) return dropdowns[i];'));
check('Search announcements are guarded by real focus',
  source.includes('if (!ensurePartsSearchRealFocus(search)) return true;'));
check('focus-change processing rejects visual-only Search focus',
  source.includes('if (!ensurePartsSearchRealFocus(element)) return;'));
check('dropdown safety timeout is five seconds',
  source.includes('var PARTS_DROPDOWN_ACTIVATION_TIMEOUT_MS = 5000;'));

console.log('\nDropdown direction gate');
{
  const spoken = [];
  const context = installFunctions({
    _partsDropdownActivation: { loadingAnnounced: false },
    _partsDropdownBlockedDirections: {},
    P: { CONTROLLER: 3 },
    scheduleSpeak: text => spoken.push(text),
  }, ['navigationValuePressed', 'partsDirectionState', 'discardPartsDropdownDirection']);

  function event() {
    return {
      prevented: false,
      stopped: false,
      preventDefault() { this.prevented = true; },
      stopPropagation() { this.stopped = true; },
    };
  }

  let e = event();
  check('discrete press is consumed while pending',
    context.discardPartsDropdownDirection(e, 'focus_d', 1) && e.prevented && e.stopped);
  check('loading feedback is spoken once', spoken.length === 1 && spoken[0] === 'Loading options.');

  e = event();
  context.discardPartsDropdownDirection(e, 'focus_d', 1);
  check('repeat does not repeat loading feedback', spoken.length === 1);

  context._partsDropdownActivation = null;
  e = event();
  check('latched press remains consumed after readiness',
    context.discardPartsDropdownDirection(e, 'focus_d', 1) && e.prevented);
  e = event();
  check('release is consumed and clears the latch',
    context.discardPartsDropdownDirection(e, 'focus_d', 0) && e.prevented);
  e = event();
  check('fresh movement passes after release',
    !context.discardPartsDropdownDirection(e, 'focus_d', 1) && !e.prevented);

  context._partsDropdownActivation = { loadingAnnounced: false };
  e = event();
  check('positive scalar direction is consumed',
    context.discardPartsDropdownDirection(e, 'focus_ud', 0.8) && e.prevented);
  context._partsDropdownActivation = null;
  e = event();
  check('scalar neutral is consumed and clears the latch',
    context.discardPartsDropdownDirection(e, 'focus_ud', 0.1) && e.prevented);
  e = event();
  check('fresh negative scalar passes after neutral',
    !context.discardPartsDropdownDirection(e, 'focus_ud', -0.8));
  check('non-navigation action is never consumed',
    !context.discardPartsDropdownDirection(event(), 'ok', 1));
}

console.log('\nDropdown readiness');
{
  const option = { visible: true };
  const markerOnly = { visible: true };
  const dropdown = {
    contains: element => element === option,
  };
  const context = installFunctions({
    document: { activeElement: markerOnly },
    closest: element => element === option ? option : null,
    visibleVueElement: element => element.visible,
  }, ['activeVuePartsDropdownOption']);

  check('visual marker alone is not ready',
    context.activeVuePartsDropdownOption(dropdown) === null);
  context.document.activeElement = option;
  check('real active option is ready',
    context.activeVuePartsDropdownOption(dropdown) === option);
}

console.log('\nDropdown activation lifecycle');
{
  let okCalls = 0;
  let searchCalls = 0;
  let discardCalls = 0;
  const context = installFunctions({
    handleVueOptionsOk: () => { okCalls++; },
    navigationValuePressed: value => !!value,
    focusedVuePartsRow: () => ({}),
    armPartsSearchFocus: () => { searchCalls++; },
    discardPartsDropdownDirection: () => { discardCalls++; return false; },
  }, ['handlePartsUiNavCapture']);
  const okEvent = { detail: { name: 'ok', value: 1 }, prevented: false };
  context.handlePartsUiNavCapture(okEvent);
  check('A/Cross arms handling without consuming activation',
    okCalls === 1 && discardCalls === 1 && !okEvent.prevented);
  context.handlePartsUiNavCapture({ detail: { name: 'context', value: 1 } });
  check('Y/Triangle starts Search reconciliation', searchCalls === 1);
}

{
  let clicked = 0;
  let restored = 0;
  const spoken = [];
  const trigger = {
    querySelector: () => ({}),
    click: () => { clicked++; },
  };
  const row = {
    isConnected: true,
    querySelector: selector => selector === '.bng-dropdown' ? trigger : null,
  };
  const activation = { row, route: '#parts|/', dropdownSeen: true };
  const context = {
    _partsDropdownActivation: activation,
    _partsDropdownPopup: {},
    loadingActive: false,
    location: { hash: '#parts', pathname: '/' },
    PARTS_CONTAINER_SELECTOR: '.parts',
    clearPartsDropdownActivation: () => { context._partsDropdownActivation = null; },
    closest: () => row,
    focusVuePartsRow: () => { restored++; return true; },
    scheduleSpeak: text => spoken.push(text),
    P: { CONTROLLER: 3 },
  };
  installFunctions(context, ['partsDropdownActivationTimedOut']);
  context.partsDropdownActivationTimedOut(activation);
  check('timeout closes an open dropdown', clicked === 1);
  check('timeout restores the row and announces once',
    restored === 1 && spoken.length === 1 &&
    spoken[0] === 'Options did not open; try again.');
}

function makeSearchContext(focusOnAttempt) {
  const frames = [];
  const spoken = [];
  let attempts = 0;
  const rowTarget = {
    focus() { context.document.activeElement = rowTarget; },
  };
  const row = {
    isConnected: true,
    querySelector: () => rowTarget,
  };
  const container = {
    querySelector: () => input,
  };
  const input = {
    visible: true,
    focus() {
      attempts++;
      if (attempts >= focusOnAttempt) context.document.activeElement = input;
    },
  };
  const context = {
    _partsSearchFocus: null,
    _partsSearchFocusGeneration: 0,
    PARTS_SEARCH_FOCUS_MAX_FRAMES: 60,
    PARTS_SEARCH_FOCUS_STABLE_FRAMES: 3,
    PARTS_CONTAINER_SELECTOR: '.parts',
    loadingActive: false,
    location: { hash: '#/pause/vehicle/configurationcombined', pathname: '/' },
    document: {
      activeElement: rowTarget,
      querySelectorAll: () => [input],
    },
    visibleVueElement: element => element.visible !== false,
    toArray: value => Array.prototype.slice.call(value || []),
    closest: (element, selector) => {
      if (selector === '.pause-tab-combined-search') return element === input ? container : null;
      if (selector === '.parts') return element === row ? row : null;
      if (selector.indexOf('.bng-accitem-caption') >= 0) return element;
      return null;
    },
    focusedVuePartsRow: () => row,
    trackedRequestAnimationFrame: fn => frames.push(fn),
    focusVuePartsRow(targetRow) {
      if (!targetRow) return false;
      targetRow.querySelector().focus();
      return true;
    },
    clearTimeout() {},
    focusDebounceTimer: null,
    kbFocusDebounceTimer: null,
    lastFocusedElement: null,
    scheduleSpeak: text => spoken.push(text),
    P: { CONTROLLER: 3 },
  };
  installFunctions(context, [
    'combinedPartsSearchInput',
    'currentCombinedPartsSearchInput',
    'cancelPartsSearchFocus',
    'failPartsSearchFocus',
    'retryPartsSearchFocus',
    'armPartsSearchFocus',
    'ensurePartsSearchRealFocus',
  ]);
  return {
    context, input, row, rowTarget, spoken,
    attempts: () => attempts,
    runFrame() {
      const frame = frames.shift();
      if (frame) frame();
    },
    pendingFrames: () => frames.length,
    hideInput: () => { context.document.querySelectorAll = () => []; },
  };
}

console.log('\nSearch real-focus reconciliation');
{
  const sim = makeSearchContext(3);
  check('visual-only Search focus is rejected',
    sim.context.ensurePartsSearchRealFocus(sim.input) === false);
  check('visual-only state does not announce Search', sim.spoken.length === 0);
  sim.runFrame();
  sim.runFrame();
  sim.runFrame();
  sim.runFrame();
  check('retry eventually gives Search real DOM focus',
    sim.context.document.activeElement === sim.input, 'attempts=' + sim.attempts());
  check('successful retry clears pending request',
    sim.context._partsSearchFocus === null);
  check('real Search focus is accepted',
    sim.context.ensurePartsSearchRealFocus(sim.input) === true);
}

{
  const sim = makeSearchContext(1);
  check('Search takes real focus immediately on the request',
    sim.context.ensurePartsSearchRealFocus(sim.input) === false &&
    sim.context.document.activeElement === sim.input);
  sim.context.document.activeElement = sim.rowTarget;
  sim.runFrame();
  sim.runFrame();
  sim.runFrame();
  check('retry reclaims focus stolen by asynchronous scope activation',
    sim.context.document.activeElement === sim.input &&
    sim.context._partsSearchFocus === null);
}

{
  const sim = makeSearchContext(Infinity);
  sim.context.ensurePartsSearchRealFocus(sim.input);
  sim.runFrame();
  sim.context.location.hash = '#/play';
  sim.runFrame();
  check('route change cancels stale Search request',
    sim.context._partsSearchFocus === null && sim.spoken.length === 0);
}

{
  const sim = makeSearchContext(Infinity);
  sim.context.ensurePartsSearchRealFocus(sim.input);
  sim.runFrame();
  sim.hideInput();
  sim.runFrame();
  check('input removal cancels silently after it was seen',
    sim.context._partsSearchFocus === null && sim.spoken.length === 0);
}

{
  const sim = makeSearchContext(Infinity);
  sim.context.ensurePartsSearchRealFocus(sim.input);
  for (let i = 0; i < 60; i++) sim.runFrame();
  check('retry timeout announces one failure',
    sim.spoken.length === 1 &&
    sim.spoken[0] === 'Search did not receive keyboard focus; try again.');
  check('retry timeout restores the originating row',
    sim.context.document.activeElement === sim.rowTarget);
  check('retry timeout clears pending request',
    sim.context._partsSearchFocus === null && sim.pendingFrames() === 0);
}

{
  const sim = makeSearchContext(1);
  sim.context.document.activeElement = sim.input;
  check('immediate real focus needs no retry',
    sim.context.ensurePartsSearchRealFocus(sim.input) === true &&
    sim.pendingFrames() === 0 && sim.spoken.length === 0);
}

if (failures) {
  console.error('\n' + failures + ' parts-focus check(s) failed.');
  process.exit(1);
}
console.log('\nAll parts-focus checks passed.');
