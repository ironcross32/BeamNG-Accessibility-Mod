// Replays bnvdaRuntime.js's mod repository details reader against a fake DOM built to
// the shape repository-details.html and info.html actually render.
//
//   node diagnostic/mod_details_sim.js
//
// The functions under test are LIFTED OUT OF THE SOURCE by name, never copied, for the
// reason binding_readout_sim.js gives: this area's failure mode is a readout that is
// merely wrong -- a paragraph silently cut in half, a list read as one run-on sentence --
// so a sim carrying its own copy would keep passing across exactly the edit that breaks
// the mod.
//
// Every scenario also asserts what the naive form answers, so no check can pass for free.

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

// Regex literals and plain numbers are lifted by line rather than by balancing.
function sliceLine(decl) {
  const start = source.indexOf('\n          ' + decl);
  if (start === -1) throw new Error('could not find ' + decl);
  return source.slice(start, source.indexOf('\n', start + 1));
}

const lifted = [
  sliceLine('var MOD_BLOCK_MAX ='),
  sliceLine('var MOD_MAX_LINES ='),
  sliceLine('var MOD_BLOCK_TAGS ='),
  sliceBalanced('toArray', 'fn'),
  sliceBalanced('cleanText', 'fn'),
  sliceBalanced('blockText', 'fn'),
  sliceBalanced('pushBlock', 'fn'),
  sliceBalanced('collectBlocks', 'fn'),
].join('\n');

const prelude = 'var MAX_LEN = 160;';

// ---------- fake DOM ----------
// Only the surface collectBlocks touches: tagName, children, childNodes, nodeType and
// the two text properties. innerText is deliberately the concatenation of descendants,
// which is what a browser gives for a rendered block.
class El {
  constructor(tag, text) {
    this.nodeType = 1;
    this.tagName = tag.toUpperCase();
    this.childNodes = [];
    if (text) this.add(new Txt(text));
  }
  add(node) { this.childNodes.push(node); return this; }
  get children() { return this.childNodes.filter(n => n.nodeType === 1); }
  get textContent() { return this.childNodes.map(n => n.textContent).join(''); }
  get innerText() { return this.textContent; }
}
class Txt {
  constructor(value) { this.nodeType = 3; this.nodeValue = value; }
  get textContent() { return this.nodeValue; }
}
function el(tag, text) { return new El(tag, text); }

const ctx = { console };
const run = new Function('ctx', prelude + '\n' + lifted + '\n' + `
  return { blockText, pushBlock, collectBlocks, cleanText, MOD_BLOCK_MAX, MOD_MAX_LINES };
`);
const M = run(ctx);

function blockTextOf(t) { return M.blockText(t); }

// ---------- harness ----------
let failures = 0;
function scenario(name) { console.log('\n' + name); }
function check(label, got, want) {
  const ok = JSON.stringify(got) === JSON.stringify(want);
  if (!ok) failures++;
  console.log('  ' + (ok ? 'ok  ' : 'FAIL') + ' ' + label + (ok ? '' : '\n       got  ' + JSON.stringify(got) + '\n       want ' + JSON.stringify(want)));
}
function assert(label, cond, detail) {
  if (!cond) failures++;
  console.log('  ' + (cond ? 'ok  ' : 'FAIL') + ' ' + label + (cond || detail === undefined ? '' : '\n       ' + detail));
}
function grep(label, needle, present) {
  const has = source.indexOf(needle) !== -1;
  assert(label, has === present, 'looked for ' + JSON.stringify(needle));
}

// ---------- 1: paragraphs stay separate and in order ----------
scenario('1  a BBCode body of paragraphs reads one line per paragraph');
{
  const body = el('div');
  body.add(el('p', 'This mod adds a delivery truck.'));
  body.add(el('p', 'Requires the latest game version.'));
  body.add(el('p', 'Thanks to everyone who tested it.'));
  const out = [];
  M.collectBlocks(body, out);
  check('three lines, in order', out, [
    'This mod adds a delivery truck.',
    'Requires the latest game version.',
    'Thanks to everyone who tested it.',
  ]);
  // The naive form -- one innerText for the whole body -- runs the paragraphs together
  // with no boundary the listener can arrow to.
  const naive = M.blockText(body.innerText);
  assert('the whole-body form is one run-on line', naive.split('.').length > 3 && naive.indexOf('truck.Requires') !== -1,
    'naive=' + JSON.stringify(naive));
}

// ---------- 2: cleanText is the wrong tool for body text ----------
scenario('2  a long paragraph is split on sentences, never truncated');
{
  const sentence = 'The suspension has been retuned for heavy loads. ';
  const long = (sentence.repeat(9)).trim();
  assert('the fixture is longer than MOD_BLOCK_MAX', long.length > M.MOD_BLOCK_MAX, 'len=' + long.length);

  const body = el('div');
  body.add(el('p', long));
  const out = [];
  M.collectBlocks(body, out);

  assert('it was split into more than one line', out.length > 1, 'lines=' + out.length);
  check('nothing was lost', out.join(' '), long);
  assert('no line ends in an ellipsis', out.every(l => !l.endsWith('...')));
  assert('every line ends on a sentence', out.every(l => /[.!?]$/.test(l)), JSON.stringify(out));

  // The naive form -- reusing cleanText, which every other readout in the file uses --
  // truncates at MAX_LEN and reads as though the author stopped writing mid-sentence.
  const naive = M.cleanText(long);
  assert('cleanText would have truncated it', naive.length === 160 && naive.endsWith('...'),
    'naive=' + JSON.stringify(naive));
}

// ---------- 3: lists and line breaks ----------
scenario('3  lists get bullets and BR splits a block');
{
  const list = el('ul');
  list.add(el('li', 'Working lights'));
  list.add(el('li', 'Openable doors'));
  const out = [];
  M.collectBlocks(list, out);
  check('one bulleted line per item', out, ['• Working lights', '• Openable doors']);

  const div = el('div');
  div.add(new Txt('Version 1.2'));
  div.add(el('br'));
  div.add(new Txt('Fixed the mirrors'));
  const out2 = [];
  M.collectBlocks(div, out2);
  check('BR ends a line', out2, ['Version 1.2', 'Fixed the mirrors']);
  // Without BR in MOD_BLOCK_TAGS the div has no block child and collapses to one line.
  assert('and BR is what makes that happen', /\|BR\|/.test(source) || source.indexOf('|BR|') !== -1);
}

// ---------- 4: images and scripts contribute nothing ----------
scenario('4  screenshots and scripts are skipped, links keep their text');
{
  const body = el('div');
  const p = el('p');
  p.add(new Txt('See the '));
  p.add(el('a', 'forum thread'));
  p.add(new Txt(' for details.'));
  body.add(p);
  const shot = el('div');
  shot.add(el('img'));
  body.add(shot);
  const out = [];
  M.collectBlocks(body, out);
  check('link text stays inline, image contributes nothing', out, ['See the forum thread for details.']);
}

// ---------- 5: a runaway description cannot become a runaway list ----------
scenario('5  the line cap holds');
{
  const body = el('div');
  for (let i = 0; i < M.MOD_MAX_LINES + 50; i++) body.add(el('p', 'Line ' + i + '.'));
  const out = [];
  M.collectBlocks(body, out);
  assert('capped at MOD_MAX_LINES', out.length === M.MOD_MAX_LINES, 'lines=' + out.length);
}

// ---------- 6: the rating row ----------
scenario('6  the rating row is counted, not read as the word "star" five times');
{
  const lifted2 = sliceBalanced('modRatingText', 'fn');
  const rate = new Function('return ' + lifted2.trim().replace(/^function /, 'function ') + '; ');
  // A fake cell exposing only querySelectorAll, which is all modRatingText touches.
  function cell(total, lit) {
    const stars = [];
    for (let i = 0; i < total; i++) stars.push({ on: i < lit });
    return {
      innerText: 'star '.repeat(total).trim(),
      textContent: 'star '.repeat(total).trim(),
      querySelectorAll(sel) {
        if (sel === '.star-button') return stars;
        if (sel === '.star-button.star-on') return stars.filter(s => s.on);
        return [];
      },
    };
  }
  const fn = rate();
  check('four of five', fn(cell(5, 4)), '4 out of 5 stars');
  check('a cell with no stars declines', fn(cell(0, 0)), '');
  // The naive form is what the live page actually produced before this rule existed.
  check('the text form says nothing about how many are lit', blockTextOf(cell(5, 4).innerText), 'star star star star star');
}

// ---------- 7: the wiring no fake DOM can reach ----------
scenario('7  source contract');
{
  grep('the transport answers a page_text request', "data.type === 'page_text') sendModDetailsSheet()", true);
  grep('the route latch is subscribed', "subscribe($rootScope, '$stateChangeSuccess'", true);
  grep('both detail routes are covered', "'menu.mods.automationDetails': 1", true);
  grep('the latch is re-pushed on a transport pong', 'sendModDetailContext();\n              return;', true);
  // modData.message is an Angular $sce.trustAsHtml wrapper OBJECT, not a string
  // (repository.js:296). Reading it would bypass the rendered DOM the user is actually
  // looking at. Comment lines are skipped, because the prose explaining this rule has to
  // write the field name itself -- the trap vehicle_geometry_sim.lua scenario 12 already
  // fell into.
  const code = source.split(/\r?\n/).filter(l => !/^\s*(\/\/|\*|\/\*)/.test(l)).join('\n');
  assert('the description is never read from modData.message', code.indexOf('modData.message') === -1);
  // cleanText truncates at MAX_LEN; body text must not go through it.
  const sheet = source.slice(source.indexOf('function readModDetailsSheet'), source.indexOf('function sendModDetailsSheet'));
  assert('readModDetailsSheet does not call cleanText', sheet.indexOf('cleanText(') === -1);
}

console.log(failures === 0 ? '\nall scenarios passed' : '\n' + failures + ' FAILURES');
process.exit(failures === 0 ? 0 : 1);
