// Coherent WebGPU adapter info. Defeats DataDome's navigator.gpu.requestAdapter()
// inconsistency check (upstream #20). Values are dataset-derived from the spoofed
// WebGL renderer (never randomized). Overrides the GPUAdapter.prototype `info`
// getter — matching how real Chrome exposes it (a prototype accessor, NOT an own
// property), so Object.getOwnPropertyDescriptor(adapter,'info') stays undefined
// like a genuine adapter.
//
// Known v1 limitations (acceptable per scope; validated/iterated via the nightly
// coherence test): the returned info is a plain object, not a real GPUAdapterInfo
// instance (an `instanceof GPUAdapterInfo` check would tell); the Block path can
// only shadow navigator.gpu, it cannot make `'gpu' in navigator` false.
(function (vendor, architecture, description, mode) {
  if (!('gpu' in navigator)) return;                 // no WebGPU → nothing to do
  if (mode === 'block') {
    try { Object.defineProperty(navigator, 'gpu', { get: function () { return undefined; }, configurable: true }); } catch (e) {}
    return;
  }
  if (vendor === null) return;                       // Native → leave real gpu in place
  if (typeof GPUAdapter === 'undefined' || !GPUAdapter.prototype) return;
  var info = { vendor: vendor, architecture: architecture, device: '', description: description };
  try {
    var d = Object.getOwnPropertyDescriptor(GPUAdapter.prototype, 'info');
    if (d && typeof d.get === 'function') {
      Object.defineProperty(GPUAdapter.prototype, 'info', {
        get: function () { return info; },
        configurable: true,
        enumerable: d.enumerable,
      });
    }
  } catch (e) {}
})(WEBGPU_VENDOR, WEBGPU_ARCHITECTURE, WEBGPU_DESCRIPTION, WEBGPU_MODE);
