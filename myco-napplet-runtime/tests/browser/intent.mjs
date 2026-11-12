// NAP-INTENT binding in the Myco supplement: both call forms, URI
// normalization, hint fragments, delivery buffering.
// node intent.mjs — no browser needed: the binding only posts and listens.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const supplement = readFileSync(new URL('../../assets/myco-prelude.js', import.meta.url), 'utf8');

function harness() {
  const posted = [];
  const listeners = [];
  const parent = { postMessage: (m) => posted.push(structuredClone(m)) };
  const window = {
    parent,
    addEventListener: (type, fn) => { if (type === 'message') listeners.push(fn); },
  };
  const context = vm.createContext({
    window, crypto, TextDecoder, Uint8Array, Promise, Set, Map, Object, Error,
    JSON, setTimeout, clearTimeout, decodeURIComponent, Array, navigator: {},
    Navigator: function () {},
  });
  context.Navigator.prototype = {};
  vm.runInContext(supplement + '\nMycoPrelude.install({ domains: ["shell", "intent"] });', context);
  const deliver = (data) => listeners.forEach((fn) => fn({ source: parent, data }));
  return { napplet: window.napplet, posted, deliver };
}

/** Answer the last `intent.invoke` with `result`. */
function answer(h, result) {
  const call = h.posted.filter((m) => m.type === 'intent.invoke').at(-1);
  h.deliver({ type: 'intent.invoke.result', id: call.id, result });
  return call.request;
}

// A real naddr: kind 35129, pubkey 0x01…, d "profiles", one relay.
function naddr() {
  const BECH32 = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
  const enc = new TextEncoder();
  const d = enc.encode('profiles'), relay = enc.encode('wss://relay.example');
  const pk = new Uint8Array(32).fill(1);
  const bytes = [0, d.length, ...d, 1, relay.length, ...relay, 2, 32, ...pk, 3, 4, 0, 0, 0x89, 0x39];
  const words = []; let acc = 0, bits = 0;
  for (const b of bytes) { acc = (acc << 8) | b; bits += 8; while (bits >= 5) { bits -= 5; words.push((acc >> bits) & 31); } }
  if (bits) words.push((acc << (5 - bits)) & 31);
  const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
  const polymod = (values) => { let chk = 1; for (const v of values) { const top = chk >>> 25; chk = ((chk & 0x1ffffff) << 5) ^ v; for (let i = 0; i < 5; i++) if ((top >>> i) & 1) chk ^= GEN[i]; } return chk; };
  const hrp = 'naddr';
  const expand = [...hrp].map((c) => c.charCodeAt(0) >> 5).concat([0], [...hrp].map((c) => c.charCodeAt(0) & 31));
  const mod = polymod(expand.concat(words, [0, 0, 0, 0, 0, 0])) ^ 1;
  const checksum = [0, 1, 2, 3, 4, 5].map((i) => (mod >>> (5 * (5 - i))) & 31);
  return hrp + '1' + words.concat(checksum).map((w) => BECH32[w]).join('');
}
const PK = '01'.repeat(32);

// URI form: query → text payload, nothing coerced, `+` literal.
{
  const h = harness();
  const p = h.napplet.intent.open('napplet:profile/open?pubkey=ab%20c&n=1&x=a+b');
  const req = answer(h, { ok: true, archetype: 'profile', action: 'open', convention: 'napplet:profile/open', handler: 'h', handled: true });
  assert.deepEqual(req, { archetype: 'profile', action: 'open', convention: 'napplet:profile/open',
    payload: { pubkey: 'ab c', n: '1', x: 'a+b' } });
  assert.equal((await p).handler, 'h');
}

// URI form with a recommending fragment and structured payload.
{
  const h = harness();
  const p = h.napplet.intent.invoke('napplet:profile/open#' + naddr(), { payload: { target: { id: 'x' } } });
  const req = answer(h, { ok: false, error: 'no handler' });
  assert.deepEqual(req.handlerHint, { address: `35129:${PK}:profiles`, relays: ['wss://relay.example'] });
  assert.deepEqual(req.payload, { target: { id: 'x' } });
  assert.equal(req.convention, 'napplet:profile/open', 'the fragment is not identity');
  assert.equal((await p).ok, false);
}

// First-draft forms still work, unchanged on the wire.
{
  const h = harness();
  const p = h.napplet.intent.open('profile', { pubkey: 'abc' }, { convention: 'napplet:profile/open' });
  const req = answer(h, { ok: true, archetype: 'profile', action: 'open', handled: true, handler: 'h', convention: 'napplet:profile/open' });
  assert.deepEqual(req, { archetype: 'profile', action: 'open', payload: { pubkey: 'abc' }, convention: 'napplet:profile/open' });
  assert.equal((await p).handled, true);

  const q = h.napplet.intent.invoke({ archetype: 'note', action: 'edit', payload: { id: 1 } });
  assert.deepEqual(answer(h, { ok: false, archetype: 'note', action: 'edit', handled: false, error: 'no handler' }),
    { archetype: 'note', action: 'edit', payload: { id: 1 } });
  assert.equal((await q).error, 'no handler');
}

// Invalid URIs reject before anything is sent.
{
  const h = harness();
  const bad = [
    () => h.napplet.intent.invoke('napplet:profile'),
    () => h.napplet.intent.invoke('napplet:profile/open?a=1&a=2'),
    () => h.napplet.intent.invoke('napplet:profile/open?a=%zz'),
    () => h.napplet.intent.invoke('napplet:profile/open?a=1', { payload: {} }),
    () => h.napplet.intent.invoke('napplet:profile/open#npub1xyz'),
    () => h.napplet.intent.invoke('napplet:profile/open#' + naddr().slice(0, -1) + 'q'),
    () => h.napplet.intent.invoke('napplet:profile/open#' + naddr(), { handlerHint: { address: `35129:${PK}:x` } }),
    () => h.napplet.intent.invoke('napplet:profile/open', { handlerHint: { address: `1:${PK}:x` } }),
    () => h.napplet.intent.open('napplet:profile/edit'),
  ];
  for (const call of bad) await assert.rejects(call());
  assert.equal(h.posted.filter((m) => m.type === 'intent.invoke').length, 0);
}

// Deliveries are held until a handler registers, then handed over in order.
{
  const h = harness();
  const delivery = (n) => ({ sender: 's', archetype: 'profile', action: 'open', convention: 'napplet:profile/open', payload: { n } });
  h.deliver({ type: 'intent.deliver', delivery: delivery(1) });
  h.deliver({ type: 'intent.deliver', delivery: { ...delivery(9), convention: 'napplet:note/open' } });
  const seen = [];
  const sub = h.napplet.intent.onDelivery((d) => seen.push(d.payload.n));
  h.deliver({ type: 'intent.deliver', delivery: delivery(2) });
  assert.deepEqual(seen, [1, 2], 'an inconsistent delivery is dropped');
  sub.close();
}

// available / handlers / onChanged.
{
  const h = harness();
  const a = h.napplet.intent.available('profile');
  const call = h.posted.at(-1);
  assert.equal(call.type, 'intent.available');
  h.deliver({ type: 'intent.available.result', id: call.id, availability: { archetype: 'profile', available: false, candidates: [], hasDefault: false } });
  assert.equal((await a).available, false);
  const changed = [];
  h.napplet.intent.onChanged((x) => changed.push(x.archetype));
  h.deliver({ type: 'intent.changed', availability: { archetype: 'feed' } });
  assert.deepEqual(changed, ['feed']);
}

console.log('PASS: NAP-INTENT binding');

// launch: a bare naddr → target; root napplets too; anything else rejected.
{
  const h = harness();
  const p = h.napplet.intent.launch(naddr(), { behavior: { focus: true } });
  const call = h.posted.at(-1);
  assert.equal(call.type, 'intent.launch');
  assert.deepEqual(call.target, { address: `35129:${PK}:profiles`, relays: ['wss://relay.example'] });
  assert.deepEqual(call.behavior, { focus: true });
  assert.equal(call.request, undefined, 'no payload');
  h.deliver({ type: 'intent.launch.result', id: call.id, result: { ok: true, handler: 'x' } });
  assert.deepEqual(await p, { ok: true, handler: 'x' });

  await assert.rejects(h.napplet.intent.launch('npub1xyz'));
  await assert.rejects(h.napplet.intent.launch('napplet:profile/open'));
  await assert.rejects(h.napplet.intent.launch(naddr().slice(0, -1) + 'q'));
  assert.equal(h.posted.filter((m) => m.type === 'intent.launch').length, 1);
}
console.log('PASS: NAP-INTENT launch');
