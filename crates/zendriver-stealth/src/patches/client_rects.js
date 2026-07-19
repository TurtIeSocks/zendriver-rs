(function (seed) {
  // Same PRNG-lifetime bug/fix as canvas.js: reset per rect, keyed by
  // (seed, [x, y, width, height]) so repeat reads of an unmoved element
  // reproduce the same sub-pixel noise instead of advancing one shared
  // stream across every rect read on the page.
  function noisyRect(r) {
    const rng = __zdKeyedRng(seed, [r.x, r.y, r.width, r.height]);
    return new DOMRect(
      r.x + (rng() - 0.5) * 1e-3,
      r.y + (rng() - 0.5) * 1e-3,
      r.width + (rng() - 0.5) * 1e-3,
      r.height + (rng() - 0.5) * 1e-3
    );
  }
  __zdReplace(Element.prototype, 'getBoundingClientRect', (orig) => function () {
    return noisyRect(orig.call(this));
  });
  __zdReplace(Element.prototype, 'getClientRects', (orig) => function () {
    const list = orig.call(this);
    const out = [];
    for (let i = 0; i < list.length; i++) out.push(noisyRect(list[i]));
    return out;
  });
})(SEED);
