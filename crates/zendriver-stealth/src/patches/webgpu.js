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
// Two fabrication cases (both only when `fabricate` is on AND the caller
// supplied vendor + limits — the Rust side already enforces that):
//   (a) `navigator.gpu` EXISTS but `requestAdapter()` resolves null (a real
//       adapter is present sometimes, absent others) → wrap
//       GPU.prototype.requestAdapter so a null/rejected result falls back to
//       the synthetic adapter, leaving a real adapter untouched.
//   (b) `navigator.gpu` is ENTIRELY ABSENT (`'gpu' in navigator === false` —
//       the common GPU-less case, e.g. Chrome launched with `--disable-gpu`)
//       → DEFINE a synthetic `navigator.gpu` on Navigator.prototype whose
//       `requestAdapter()` resolves the synthetic adapter. This flips
//       `'gpu' in navigator` to TRUE, which is COHERENT for a modern-Chrome
//       persona: real modern Chrome always exposes `navigator.gpu` even with
//       no usable GPU (there `requestAdapter()` just resolves null). Restoring
//       that presence is the caller's explicit opt-in.
//
// Coherence notes / remaining limitations (acceptable per scope):
//   - `navigator.gpu`, the fabricated adapter, and its `.info` inherit the real
//     GPU / GPUAdapter / GPUAdapterInfo prototypes (or a synthesized same-named
//     constructor, installed as a global, when the WebGPU IDL is absent), so
//     `instanceof` holds for all three. Their VALUES are own getters, not
//     prototype getters like a genuine instance — a deeper
//     Object.getOwnPropertyDescriptor probe still tells (unchanged from before);
//   - `.limits` / `.features` are still a plain object / Set, not real
//     GPUSupportedLimits / GPUSupportedFeatures instances — rebinding a
//     caller-supplied SUBSET of ~30 limit fields to those classes risks a
//     genuine brand-check throwing on the fields we did not override, a bigger
//     change than this closes (tracked as a follow-up); info omits
//     subgroupMinSize/subgroupMaxSize;
//   - the Block path can only shadow navigator.gpu, it cannot make
//     `'gpu' in navigator` false;
//   - the FABRICATED synthetic adapter's `requestDevice()` always REJECTS —
//     faking a working GPUDevice needs a real GPU behind it, which this patch
//     cannot provide, so fabrication only makes `requestAdapter()` resolve a
//     coherent adapter for detection scripts that stop there, never actual
//     WebGPU rendering on a GPU-less host;
//   - when case (b) synthesizes `navigator.gpu`, fabrication flips
//     `'gpu' in navigator` to true (coherent for a modern-Chrome persona, the
//     caller's explicit choice — see case (b) above).
(function (vendor, architecture, device, description, limits, features, mode, fabricate) {
  var gpuPresent = ('gpu' in navigator);

  // Block: shadow navigator.gpu → undefined. Only meaningful when it exists.
  if (mode === 'block') {
    if (gpuPresent) {
      try { __zdGetter(navigator, 'gpu', function () { return undefined; }, { enumerable: false }); } catch (e) {}
    }
    return;
  }

  if (vendor === null) return;

  // Get (or synthesize + install) the named WebGPU constructor's prototype so
  // objects built with Object.create(...) pass `instanceof` (see the header's
  // limitations note). When the real class exists — the WebGPU IDL is compiled
  // into this Chrome build, even with no hardware adapter behind it — its
  // prototype is reused directly. When the class is absent entirely, synthesize
  // a minimal same-named constructor and install it as a (non-enumerable)
  // global, mirroring how real Chrome always exposes the constructor whether or
  // not a usable instance is available (so `typeof window.GPU === 'function'`
  // stays true, coherent for a modern-Chrome persona). Values stay OWN getters
  // on each instance below — we deliberately do NOT override getters on a real
  // prototype here, so a real adapter passing through the case-(a) wrapper keeps
  // its own limits/features.
  function __zdGpuProto(globalName) {
    var root = (typeof self !== 'undefined') ? self : window;
    var Ctor = root[globalName];
    if (typeof Ctor === 'function' && Ctor.prototype) return Ctor.prototype;
    Ctor = __zdMark(function () {}, globalName, 0);
    Ctor.prototype = {};
    try {
      Object.defineProperty(root, globalName, { value: Ctor, writable: true, enumerable: false, configurable: true });
    } catch (e) {}
    return Ctor.prototype;
  }

  // Shared synthetic info / feature set used by both decorate and fabricate.
  // `info` inherits GPUAdapterInfo.prototype so `info instanceof GPUAdapterInfo`
  // holds (closing the cheapest deep probe); its fields are own getters, whose
  // values are always defined.
  var info = Object.create(__zdGpuProto('GPUAdapterInfo'));
  __zdGetter(info, 'vendor', function () { return vendor; }, { enumerable: true });
  __zdGetter(info, 'architecture', function () { return architecture; }, { enumerable: true });
  __zdGetter(info, 'device', function () { return device || ''; }, { enumerable: true });
  __zdGetter(info, 'description', function () { return description || ''; }, { enumerable: true });
  __zdGetter(info, 'isFallbackAdapter', function () { return false; }, { enumerable: true });
  var featureSet = null;
  if (features) {
    try { featureSet = new Set(features); } catch (e) { featureSet = null; }
  }

  // Decorate path: only relevant when a real GPUAdapter class is present.
  if (gpuPresent) {
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
  }

  // Fabrication: only when the caller explicitly opted in (Rust side already
  // refuses this unless both `vendor` and `limits` were explicitly set — see
  // `WebgpuSpec::fabricate_when_absent`). When fabricate is OFF and gpu is
  // absent, we simply fall through and return — no auto behavior.
  if (!fabricate) return;
  try {
    // The synthetic adapter, shared by both fabrication cases. Inherits
    // GPUAdapter.prototype (real or synthesized) so `synthetic instanceof
    // GPUAdapter` holds; info/limits/features stay own getters.
    var synthetic = Object.create(__zdGpuProto('GPUAdapter'));
    __zdGetter(synthetic, 'info', function () { return info; }, { enumerable: false });
    __zdGetter(synthetic, 'limits', function () { return limits || {}; }, { enumerable: false });
    __zdGetter(synthetic, 'features', function () { return featureSet || new Set(); }, { enumerable: false });
    __zdGetter(synthetic, 'isFallbackAdapter', function () { return false; }, { enumerable: false });
    synthetic.requestDevice = __zdMark(function requestDevice() {
      return Promise.reject(new DOMException(
        'WebGPU device creation is not supported for this adapter.',
        'NotSupportedError'
      ));
    }, 'requestDevice', 0);

    if (!gpuPresent) {
      // Case (b): navigator.gpu entirely absent → define a synthetic one on
      // Navigator.prototype (prototype accessor, like real Chrome — so
      // Object.getOwnPropertyDescriptor(navigator,'gpu') stays undefined).
      // Inherits GPU.prototype (real or synthesized) so `navigator.gpu
      // instanceof GPU` holds. requestAdapter / getPreferredCanvasFormat stay
      // own methods — moving them onto GPU.prototype would mutate the real
      // class globally; instanceof holds regardless.
      var syntheticGpu = Object.create(__zdGpuProto('GPU'));
      syntheticGpu.requestAdapter = __zdMark(function requestAdapter() {
        return Promise.resolve(synthetic);
      }, 'requestAdapter', 0);
      // Real navigator.gpu also exposes getPreferredCanvasFormat(); a gpu
      // object lacking it is itself a tell. Desktop Chrome returns
      // 'bgra8unorm' (personas here are desktop-only).
      syntheticGpu.getPreferredCanvasFormat = __zdMark(function getPreferredCanvasFormat() {
        return 'bgra8unorm';
      }, 'getPreferredCanvasFormat', 0);
      if (typeof Navigator !== 'undefined' && Navigator.prototype) {
        __zdGetter(Navigator.prototype, 'gpu', function () { return syntheticGpu; }, { enumerable: true });
      } else {
        __zdGetter(navigator, 'gpu', function () { return syntheticGpu; }, { enumerable: true });
      }
      return;
    }

    // Case (a): navigator.gpu exists but requestAdapter() can resolve null →
    // wrap GPU.prototype.requestAdapter so a REAL adapter passes through
    // untouched (already decorated above) and only a null/undefined/rejected
    // result falls back to the synthetic one.
    if (typeof GPU === 'undefined' || !GPU.prototype || typeof GPU.prototype.requestAdapter !== 'function') return;
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
