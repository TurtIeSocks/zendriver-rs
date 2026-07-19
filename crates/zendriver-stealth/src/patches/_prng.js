// mulberry32 — deterministic PRNG seeded by the persona seed.
function __zdRng(seed) {
  let a = seed >>> 0;
  return function () {
    a |= 0; a = (a + 0x6D2B79F5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// Derive a FRESH `__zdRng` stream keyed by (seed, content) instead of one
// continuously-advancing stream shared across an entire page lifetime. Used
// by every noise surface (canvas/audio/clientRects) so repeat reads of
// IDENTICAL content reproduce the SAME noise — the previous single-advancing
// PRNG made even `Strategy::Seeded` unstable across reads, since read #2
// consumed the next slice of the shared stream instead of reproducing read
// #1's noise. Content is folded into the seed via an FNV-ish mix; values are
// scaled + rounded before folding so non-integer content (AnalyserNode's
// float dB samples, sub-pixel rect geometry) still hashes deterministically
// across repeat reads of the same underlying buffer.
function __zdKeyedRng(seed, values) {
  let h = seed >>> 0;
  for (let i = 0; i < values.length; i++) {
    const v = Math.round(values[i] * 1000) | 0;
    h = Math.imul(h ^ v, 0x01000193) >>> 0;
  }
  return __zdRng(h);
}
