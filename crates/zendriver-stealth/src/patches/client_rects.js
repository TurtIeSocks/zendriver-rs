(function (seed) {
  const rng = __zdRng(seed);
  function noisy(v) { return v + (rng() - 0.5) * 1e-3; }
  const origRect = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function () {
    const r = origRect.call(this);
    return new DOMRect(noisy(r.x), noisy(r.y), noisy(r.width), noisy(r.height));
  };
  const origRects = Element.prototype.getClientRects;
  Element.prototype.getClientRects = function () {
    const list = origRects.call(this);
    const out = [];
    for (let i = 0; i < list.length; i++) {
      const r = list[i];
      out.push(new DOMRect(noisy(r.x), noisy(r.y), noisy(r.width), noisy(r.height)));
    }
    return out;
  };
})(SEED);
