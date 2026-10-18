(function (allow, seed) {
  __zdReplace(CanvasRenderingContext2D.prototype, 'measureText', (orig) => function (text) {
    const m = orig.call(this, text);
    // Reset for the same native metrics: repeated reads, different contexts
    // and page reloads must agree instead of advancing a shared PRNG.
    const rng = __zdKeyedRng(seed, [m.width, m.actualBoundingBoxLeft,
      m.actualBoundingBoxRight, m.actualBoundingBoxAscent, m.actualBoundingBoxDescent]);
    try { Object.defineProperty(m, 'width', { value: m.width + (rng() - 0.5) * 1e-3 }); }
    catch (e) {}
    return m;
  });
  // If an allow-list is provided, hide other fonts from FontFaceSet checks.
  if (Array.isArray(allow) && document.fonts && document.fonts.check) {
    const allowed = new Set(allow.map((name) => name.toLowerCase()));
    const generic = new Set(['serif', 'sans-serif', 'monospace', 'cursive',
      'fantasy', 'system-ui', 'ui-serif', 'ui-sans-serif', 'ui-monospace', 'ui-rounded']);
    __zdReplace(document.fonts, 'check', (origCheck) => function (font, text) {
      // Let the browser parse CSS font syntax: splitting on spaces truncates
      // multi-word names such as "Times New Roman" from the allow-list.
      const style = document.createElement('span').style;
      style.font = font;
      const families = style.fontFamily.match(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|[^,]+/g) || [];
      for (const family of families) {
        const name = family.trim().replace(/^["']|["']$/g, '').toLowerCase();
        if (!allowed.has(name) && !generic.has(name)) return false;
      }
      return origCheck.call(document.fonts, font, text);
    });
  }
})(FONT_ALLOW, SEED);
