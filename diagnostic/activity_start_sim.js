// Checks the native mission prompt readout directly from bnvdaRuntime.js source.
// Run with: node diagnostic/activity_start_sim.js

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const srcPath = process.env.BNVDA_RUNTIME_PATH || path.join(
  __dirname, '..', 'bng_mod', 'ui', 'ui-vue', 'mods', 'bnvda', 'bnvdaRuntime.js');
const source = fs.readFileSync(srcPath, 'utf8');

function liftFunction(name) {
  const needle = '\n          function ' + name + '(';
  const start = source.indexOf(needle);
  if (start < 0) throw new Error('missing function ' + name);
  const brace = source.indexOf('{', start);
  let depth = 0, quote = null, escape = false;
  for (let i = brace; i < source.length; i++) {
    const ch = source[i];
    if (quote) {
      if (escape) escape = false;
      else if (ch === '\\') escape = true;
      else if (ch === quote) quote = null;
      continue;
    }
    if (ch === '"' || ch === "'") { quote = ch; continue; }
    if (ch === '{') depth++;
    if (ch === '}' && --depth === 0) return source.slice(start, i + 1);
  }
  throw new Error('unbalanced function ' + name);
}

const context = {
  cleanText: value => String(value || '').replace(/\s+/g, ' ').trim(),
  toArray: value => Array.prototype.slice.call(value || []),
};
vm.createContext(context);
vm.runInContext(liftFunction('activityStartText'), context);

const heading = { innerText: 'Proving Grounds Hill Climb: Reach the summit' };
const props = [{ innerText: 'Current vehicle' }, { innerText: 'One lap' }];
const buttons = [{ innerText: 'Start the Challenge' }, { innerText: 'Close' }];
const root = {
  matches: selector => selector === '.activity-start',
  querySelector: selector => selector === '.bng-screen-heading' ? heading : null,
  querySelectorAll: selector => selector.includes('activity-props') ? props : buttons,
};
const spoken = context.activityStartText(root);
if (!spoken.includes('Challenge available')) throw new Error('missing prompt prefix');
if (!spoken.includes('Proving Grounds Hill Climb')) throw new Error('missing heading');
if (!spoken.includes('Current vehicle') || !spoken.includes('One lap')) throw new Error('missing properties');
if (!spoken.includes('Start the Challenge') || !spoken.includes('Close')) throw new Error('missing actions');
if (!spoken.includes('gameplay interact')) throw new Error('missing control instruction');
if (!source.includes("document.querySelector('.activity-start, .mission-details-layout')")) {
  throw new Error('native mission screens are not registered as Vue roots');
}
if (!source.includes("activityText !== _activityStartSignature")) {
  throw new Error('prompt announcement is not deduplicated');
}

console.log('activity start simulation passed');
