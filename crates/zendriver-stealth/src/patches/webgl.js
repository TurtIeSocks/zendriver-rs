// Defeats: bot.sannysoft.com `WebGL Vendor` + `WebGL Renderer` rows.
const VENDOR = 'Google Inc. (Intel)';
const RENDERER = 'ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)';
[WebGLRenderingContext.prototype, WebGL2RenderingContext.prototype].forEach(proto => {
    __zdReplace(proto, 'getParameter', (orig) => function(param) {
        if (param === 37445) return VENDOR;    // UNMASKED_VENDOR_WEBGL
        if (param === 37446) return RENDERER;  // UNMASKED_RENDERER_WEBGL
        return orig.call(this, param);
    });
});

// Persona-driven value substitution (WEBGL_VENDOR / WEBGL_RENDERER tokens).
(function (vendor, renderer) {
  const VENDOR = 0x9245, RENDERER = 0x9246; // UNMASKED_VENDOR_WEBGL / RENDERER
  function patch(proto) {
    __zdReplace(proto, 'getParameter', (orig) => function (p) {
      if (vendor && p === VENDOR) return vendor;
      if (renderer && p === RENDERER) return renderer;
      return orig.call(this, p);
    });
  }
  if (window.WebGLRenderingContext) patch(WebGLRenderingContext.prototype);
  if (window.WebGL2RenderingContext) patch(WebGL2RenderingContext.prototype);
})(WEBGL_VENDOR, WEBGL_RENDERER);
