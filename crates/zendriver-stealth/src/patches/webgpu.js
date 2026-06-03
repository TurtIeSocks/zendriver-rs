// Coherent WebGPU adapter info. Defeats DataDome's navigator.gpu.requestAdapter()
// inconsistency check (upstream #20). Values are dataset-derived from the spoofed
// WebGL renderer (never randomized). Overrides the GPUAdapter.prototype `info`
// getter — matching how real Chrome exposes it (a prototype accessor, NOT an own
// property), so Object.getOwnPropertyDescriptor(adapter,'info') stays undefined
// like a genuine adapter.
//
// Validated against native Chrome (Apple M4 Pro): info = { vendor, architecture,
// device:"", description:"" } — Chrome masks device + description, so we emit
// them empty. `isFallbackAdapter:false` mirrors a real hardware adapter.
//
// Known v1 limitations (acceptable per scope): the returned info is a plain
// object, not a real GPUAdapterInfo instance (an `instanceof` check would tell);
// it omits subgroupMinSize/subgroupMaxSize; the Block path can only shadow
// navigator.gpu, it cannot make `'gpu' in navigator` false.
(function (vendor, architecture, mode) {
  if (!('gpu' in navigator)) return;
  if (mode === 'block') {
    try { __zdGetter(navigator, 'gpu', function () { return undefined; }, { enumerable: false }); } catch (e) {}
    return;
  }
  if (vendor === null) return;
  if (typeof GPUAdapter === 'undefined' || !GPUAdapter.prototype) return;
  var info = { vendor: vendor, architecture: architecture, device: '', description: '', isFallbackAdapter: false };
  try {
    var d = Object.getOwnPropertyDescriptor(GPUAdapter.prototype, 'info');
    if (d && typeof d.get === 'function') {
      __zdGetter(GPUAdapter.prototype, 'info', function () { return info; }, { enumerable: d.enumerable });
    }
  } catch (e) {}
})(WEBGPU_VENDOR, WEBGPU_ARCHITECTURE, WEBGPU_MODE);
