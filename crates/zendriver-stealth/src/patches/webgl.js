// Defeats: bot.sannysoft.com `WebGL Vendor` + `WebGL Renderer` rows.
// Headless reports vendor="Brian Paul" / renderer="Mesa OffScreen" or SwiftShader.
// Spoof to common Intel desktop values matching the fingerprint platform.
const VENDOR = 'Google Inc. (Intel)';
const RENDERER = 'ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)';
[WebGLRenderingContext.prototype, WebGL2RenderingContext.prototype].forEach(proto => {
    const orig = proto.getParameter;
    proto.getParameter = function(param) {
        if (param === 37445) return VENDOR;    // UNMASKED_VENDOR_WEBGL
        if (param === 37446) return RENDERER;  // UNMASKED_RENDERER_WEBGL
        return orig.call(this, param);
    };
});
