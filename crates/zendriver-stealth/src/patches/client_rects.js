(function (seed) {
  const rng = __zdRng(seed);
  function noisy(v) { return v + (rng() - 0.5) * 1e-3; }
  __zdReplace(Element.prototype, 'getBoundingClientRect', (orig) => function () {
    const r = orig.call(this);
    return new DOMRect(noisy(r.x), noisy(r.y), noisy(r.width), noisy(r.height));
  });
  __zdReplace(Element.prototype, 'getClientRects', (orig) => function () {
    const list = orig.call(this);
    const out = [];
    for (let i = 0; i < list.length; i++) {
      const r = list[i];
      out.push(new DOMRect(noisy(r.x), noisy(r.y), noisy(r.width), noisy(r.height)));
    }
    return out;
  });
})(SEED);
