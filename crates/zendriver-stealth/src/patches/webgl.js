// Coherent WebGL fingerprint (defeats bot.sannysoft.com WebGL rows AND the
// advanced-WAF cross-checks that a partial spoof fails).
//
// A single getParameter / getSupportedExtensions override that spoofs EVERY
// value a fingerprinter reads — the two DEBUG_renderer_info UNMASKED strings
// (37445/37446) PLUS the plain VENDOR/RENDERER, the supported-extension list,
// and the MAX_* caps — so no native backend value ever leaks alongside the
// spoofed pair. Spoofing only the unmasked pair (the old behaviour) left
// getParameter(RENDERER)/getSupportedExtensions()/MAX_* reporting the real
// backend — e.g. "Google SwiftShader" under `--use-angle=swiftshader`, or the
// host GPU otherwise — a three-way incoherence Imperva/Incapsula flag as bot.
//
// Defaults to a coherent ANGLE / Direct3D11 Intel profile; the persona's
// WEBGL_VENDOR / WEBGL_RENDERER (JS string literals, or `null` under the Native
// strategy → the defaults) override the UNMASKED pair.
(function (personaVendor, personaRenderer) {
  const UNMASKED_VENDOR_WEBGL = 37445; // 0x9245
  const UNMASKED_RENDERER_WEBGL = 37446; // 0x9246
  const VENDOR = 0x1f00; // 7936  → "WebKit"      (Chrome's masked vendor)
  const RENDERER = 0x1f01; // 3415 → "WebKit WebGL" (Chrome's masked renderer)
  const MAX_VERTEX_UNIFORM_VECTORS = 0x8dfb;
  const MAX_VIEWPORT_DIMS = 0x0d3a;

  const unmaskedVendor = personaVendor || 'Google Inc. (Intel)';
  const unmaskedRenderer =
    personaRenderer ||
    'ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)';

  // The supported-extension list a real ANGLE / Direct3D11 backend reports —
  // coherent with the spoofed D3D11 renderer above (a software backend reports
  // a shorter, distinctive list).
  const EXTENSIONS = [
    'ANGLE_instanced_arrays', 'EXT_blend_minmax', 'EXT_clip_control',
    'EXT_color_buffer_half_float', 'EXT_depth_clamp', 'EXT_disjoint_timer_query',
    'EXT_float_blend', 'EXT_frag_depth', 'EXT_polygon_offset_clamp',
    'EXT_shader_texture_lod', 'EXT_texture_compression_bptc',
    'EXT_texture_compression_rgtc', 'EXT_texture_filter_anisotropic',
    'EXT_texture_mirror_clamp_to_edge', 'EXT_sRGB', 'KHR_parallel_shader_compile',
    'OES_element_index_uint', 'OES_fbo_render_mipmap', 'OES_standard_derivatives',
    'OES_texture_float', 'OES_texture_float_linear', 'OES_texture_half_float',
    'OES_texture_half_float_linear', 'OES_vertex_array_object',
    'WEBGL_blend_func_extended', 'WEBGL_color_buffer_float',
    'WEBGL_compressed_texture_s3tc', 'WEBGL_compressed_texture_s3tc_srgb',
    'WEBGL_debug_renderer_info', 'WEBGL_debug_shaders', 'WEBGL_depth_texture',
    'WEBGL_draw_buffers', 'WEBGL_lose_context', 'WEBGL_multi_draw',
    'WEBGL_polygon_mode',
  ];

  function patch(proto) {
    __zdReplace(proto, 'getParameter', (orig) => function (param) {
      if (param === UNMASKED_VENDOR_WEBGL) return unmaskedVendor;
      if (param === UNMASKED_RENDERER_WEBGL) return unmaskedRenderer;
      if (param === VENDOR) return 'WebKit';
      if (param === RENDERER) return 'WebKit WebGL';
      if (param === MAX_VERTEX_UNIFORM_VECTORS) return 4096;
      if (param === MAX_VIEWPORT_DIMS) return new Int32Array([32767, 32767]);
      return orig.call(this, param);
    });
    __zdReplace(proto, 'getSupportedExtensions', () => function () {
      return EXTENSIONS.slice();
    });
  }

  if (window.WebGLRenderingContext) patch(WebGLRenderingContext.prototype);
  if (window.WebGL2RenderingContext) patch(WebGL2RenderingContext.prototype);
})(WEBGL_VENDOR, WEBGL_RENDERER);
