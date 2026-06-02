(function (allow, seed) {
  const rng = __zdRng(seed);
  const orig = CanvasRenderingContext2D.prototype.measureText;
  CanvasRenderingContext2D.prototype.measureText = function (text) {
    const m = orig.call(this, text);
    // Sub-pixel width noise, deterministic.
    try { Object.defineProperty(m, 'width', { value: m.width + (rng() - 0.5) * 1e-3 }); }
    catch (e) {}
    return m;
  };
  // If an allow-list is provided, hide other fonts from FontFaceSet checks.
  if (Array.isArray(allow) && document.fonts && document.fonts.check) {
    const origCheck = document.fonts.check.bind(document.fonts);
    document.fonts.check = function (font, text) {
      const fam = (font || '').split(/\s+/).pop();
      if (fam && allow.indexOf(fam.replace(/["']/g, '')) === -1) return false;
      return origCheck(font, text);
    };
  }
})(FONT_ALLOW, SEED);
