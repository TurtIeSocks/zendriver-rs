// Coherent WebGPU adapter info + opt-in synthetic-adapter fabrication.
// Defeats DataDome's navigator.gpu.requestAdapter() inconsistency check
// (upstream #20). Decorate-path values are dataset-derived from the spoofed
// WebGL renderer by default (never randomized), or caller-supplied via
// `WebgpuSpec` (persona/specs.rs) — same trust model as `webgl.js`'s
// unmasked vendor/renderer override. Overrides the GPUAdapter.prototype
// `info` / `limits` / `features` getters — matching how real Chrome exposes
// them (prototype accessors, NOT own properties), so
// Object.getOwnPropertyDescriptor(adapter, 'info') stays undefined like a
// genuine adapter.
//
// Validated against native Chrome (Apple M4 Pro): info = { vendor,
// architecture, device:"", description:"" } — Chrome masks device +
// description, so we emit them empty by default. `isFallbackAdapter:false`
// mirrors a real hardware adapter.
//
// Known v1 limitations (acceptable per scope): the returned info / limits /
// features are plain objects, not real GPUAdapterInfo / GPUSupportedLimits /
// GPUSupportedFeatures instances (an `instanceof` check would tell); info
// omits subgroupMinSize/subgroupMaxSize; the Block path can only shadow
// navigator.gpu, it cannot make `'gpu' in navigator` false; the FABRICATED
// synthetic adapter's `requestDevice()` always REJECTS — faking a working
// GPUDevice needs a real GPU behind it, which this patch cannot provide, so
// it only makes `requestAdapter()` resolve a coherent adapter for detection
// scripts that stop there, never actual WebGPU rendering on a GPU-less host.
(function (vendor, architecture, device, description, limits, features, mode, fabricate) {
  if (!('gpu' in navigator)) return;
  if (mode === 'block') {
    try { __zdGetter(navigator, 'gpu', function () { return undefined; }, { enumerable: false }); } catch (e) {}
    return;
  }
  if (vendor === null) return;

  var info = { vendor: vendor, architecture: architecture, device: device || '', description: description || '', isFallbackAdapter: false };
  var featureSet = null;
  if (features) {
    try { featureSet = new Set(features); } catch (e) { featureSet = null; }
  }

  try {
    if (typeof GPUAdapter !== 'undefined' && GPUAdapter.prototype) {
      var di = Object.getOwnPropertyDescriptor(GPUAdapter.prototype, 'info');
      if (di && typeof di.get === 'function') {
        __zdGetter(GPUAdapter.prototype, 'info', function () { return info; }, { enumerable: di.enumerable });
      }
      if (limits) {
        var dl = Object.getOwnPropertyDescriptor(GPUAdapter.prototype, 'limits');
        if (dl && typeof dl.get === 'function') {
          __zdGetter(GPUAdapter.prototype, 'limits', function () { return limits; }, { enumerable: dl.enumerable });
        }
      }
      if (featureSet) {
        var df = Object.getOwnPropertyDescriptor(GPUAdapter.prototype, 'features');
        if (df && typeof df.get === 'function') {
          __zdGetter(GPUAdapter.prototype, 'features', function () { return featureSet; }, { enumerable: df.enumerable });
        }
      }
    }
  } catch (e) {}

  // Fabrication: only when the caller explicitly opted in (Rust side already
  // refuses this unless both `vendor` and `limits` were explicitly set — see
  // `WebgpuSpec::fabricate_when_absent`). Wraps GPU.prototype.requestAdapter
  // so a REAL adapter still passes through untouched (just decorated above);
  // only a null/undefined/rejected result falls back to the synthetic one.
  if (!fabricate) return;
  try {
    if (typeof GPU === 'undefined' || !GPU.prototype || typeof GPU.prototype.requestAdapter !== 'function') return;
    var synthetic = { isFallbackAdapter: false };
    __zdGetter(synthetic, 'info', function () { return info; }, { enumerable: false });
    __zdGetter(synthetic, 'limits', function () { return limits || {}; }, { enumerable: false });
    __zdGetter(synthetic, 'features', function () { return featureSet || new Set(); }, { enumerable: false });
    synthetic.requestDevice = __zdMark(function requestDevice() {
      return Promise.reject(new DOMException(
        'WebGPU device creation is not supported for this adapter.',
        'NotSupportedError'
      ));
    }, 'requestDevice', 0);
    __zdReplace(GPU.prototype, 'requestAdapter', function (orig) {
      return function () {
        var self = this, args = arguments;
        try {
          return Promise.resolve(orig.apply(self, args)).then(
            function (adapter) { return adapter || synthetic; },
            function () { return synthetic; }
          );
        } catch (e) {
          return Promise.resolve(synthetic);
        }
      };
    });
  } catch (e) {}
})(WEBGPU_VENDOR, WEBGPU_ARCHITECTURE, WEBGPU_DEVICE, WEBGPU_DESCRIPTION, WEBGPU_LIMITS, WEBGPU_FEATURES, WEBGPU_MODE, WEBGPU_FABRICATE);
