// Coherent WebGPU adapter info + opt-in synthetic-adapter fabrication.
// Defeats DataDome's navigator.gpu.requestAdapter() inconsistency check
// (upstream #20). Decorate-path values are never randomized: `info`'s
// vendor/architecture are dataset-derived from the spoofed WebGL renderer, and
// `limits`/`features` are the measured values of the capability tier that same
// renderer resolves (gpu/tiers.rs, probed from a real adapter in the same run
// as the tier's WebGL blocks). Either can be replaced by a caller-supplied
// `WebgpuSpec` (persona/specs.rs) — same trust model as `webgl.js`'s
// unmasked vendor/renderer override. A tier whose machine had no adapter at
// all (SwiftShader) passes null for both, which leaves the host adapter's own
// values alone rather than claiming another tier's. Overrides the GPUAdapter.prototype
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
//       `navigator.gpu` is `[SecureContext]`-gated, so this is what an
//       opaque-origin page reports, e.g. `about:blank` or a bare `data:`
//       URL, regardless of launch flags or GPU hardware)
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
//   - `.limits` / `.features` are the REAL GPUSupportedLimits /
//     GPUSupportedFeatures instances: their prototypes' accessors and setlike
//     members are overridden rather than the objects replaced, so brand,
//     `constructor.name`, identity and own-property shape are a genuine
//     adapter's. This used to hand back a plain object / Set (four one-line
//     tells) and was deferred over the risk of a brand check throwing on a
//     field left unoverridden; overriding the prototype in place removes that
//     risk, since an unoverridden field simply keeps its own getter. What
//     remains: iterators from `features.keys()/values()/entries()` are Array
//     Iterators rather than GPUSupportedFeatures Iterators, and info omits
//     subgroupMinSize/subgroupMaxSize;
//   - `requestDevice` on the decorate path is wrapped so it agrees with those
//     served values: a `requiredLimits` / `requiredFeatures` request within the
//     advertisement resolves (translated down to what the hardware can give,
//     with the REQUESTED values reported on the device), one beyond it rejects
//     the way Chrome does. It closes the interrogation divergence only —
//     ACTUALLY allocating at the claimed capability still fails, since no patch
//     can conjure hardware. A page that invents a feature name gets the
//     adapter-level "Unsupported feature" TypeError rather than Chrome's IDL
//     enum-conversion one: same error type, different wording;
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
  // Serve the limits by overriding the ACCESSORS on GPUSupportedLimits.prototype
  // rather than by handing back a substitute object.
  //
  // Measured on Chrome 150 (Apple M4 Pro): a real `adapter.limits` is a
  // GPUSupportedLimits instance with ZERO own properties — all 36 limits are
  // enumerable, configurable getters on its prototype. Returning a plain object
  // in its place is four one-line tells at once — `constructor.name` reads
  // "Object", `instanceof GPUSupportedLimits` is false, `Object.keys()` returns
  // 36 where a real one returns 0, and so does getOwnPropertyNames. Overriding
  // the prototype's getters instead keeps the genuine instance — identity,
  // brand, own-property shape all untouched — and changes only what each getter
  // answers. That is the same thing this patch already does for
  // GPUAdapter.prototype.info.
  //
  // It also removes the reason the earlier version stopped short: only limits
  // the running Chrome ALREADY declares are overridden, so a limit the table
  // carries and this build lacks is skipped (defining it would invent a limit
  // no real build reports), and a limit this build has that the table lacks
  // keeps its own getter rather than hitting a brand check and throwing.
  //
  // One prototype, two kinds of instance. `GPUDevice.limits` is a
  // GPUSupportedLimits too, and it does NOT hold the adapter's numbers: real
  // Chrome reports exactly what `requestDevice` was asked for and the spec
  // DEFAULT for every limit that was not (measured: a device requested with no
  // `requiredLimits` reports maxBufferSize 268435456 next to an adapter's
  // 4294967292). Serving one value per name to every instance would answer the
  // adapter's number on a device — a divergence in the opposite direction from
  // the one this patch closes. So each device's limits object is registered in
  // `__zdDeviceLimits` by the `requestDevice` wrapper below, and those
  // instances answer the requested value, falling through to the untouched
  // real getter — the spec default — for the rest.
  var __zdDeviceLimits = new WeakMap(); // GPUSupportedLimits -> {name: requested value}
  var __zdDeviceFeatures = new WeakMap(); // GPUSupportedFeatures -> [name]
  var __zdRealLimit = Object.create(null); // limit name -> the real prototype getter
  var __zdRealHas = null; // the real GPUSupportedFeatures.prototype.has

  function __zdServeLimits(served) {
    if (!served || typeof GPUSupportedLimits === 'undefined' || !GPUSupportedLimits.prototype) return false;
    var proto = GPUSupportedLimits.prototype;
    Object.keys(served).forEach(function (name) {
      var d = Object.getOwnPropertyDescriptor(proto, name);
      if (!d || typeof d.get !== 'function') return;
      var v = served[name];
      var real = d.get;
      __zdRealLimit[name] = real;
      __zdGetter(proto, name, function () {
        var asked = __zdDeviceLimits.get(this);
        if (!asked) return v; // an adapter's limits: the tier's number
        return (name in asked) ? asked[name] : real.call(this); // a device's: asked-for, else the default
      }, { enumerable: d.enumerable });
    });
    return true;
  }

  // The same move for the feature set, which is setlike rather than a bag of
  // accessors: `size` is a getter and `has` / `keys` / `values` / `entries` /
  // `forEach` are writable prototype methods (measured, Chrome 150), with
  // `Symbol.iterator` alongside them. A `Set` in its place reads as "Set" from
  // `constructor.name` and fails `instanceof GPUSupportedFeatures`, so the
  // members are what get replaced and the instance stays real.
  //
  // Residual, and smaller than what it replaces: the iterators handed back are
  // ordinary Array Iterators, where a real one is a GPUSupportedFeatures
  // Iterator. That shows up only in `Object.prototype.toString.call(...)` on
  // the iterator itself, not in `has`, `size`, spread, or `for...of`.
  //
  // Split by instance for the same reason as the limits: a DEVICE's feature set
  // is the one it was created with, not the adapter's (measured: a device
  // requested with no `requiredFeatures` reports a single
  // "core-features-and-limits" beside an adapter's 22), so a device registered
  // in `__zdDeviceFeatures` reads its own list.
  function __zdServeFeatures(list) {
    if (!list || typeof GPUSupportedFeatures === 'undefined' || !GPUSupportedFeatures.prototype) return false;
    var proto = GPUSupportedFeatures.prototype;
    var names = list.slice();
    var of = function (self) { return __zdDeviceFeatures.get(self) || names; };
    var ds = Object.getOwnPropertyDescriptor(proto, 'size');
    if (ds && typeof ds.get === 'function') {
      __zdGetter(proto, 'size', function () { return of(this).length; }, { enumerable: ds.enumerable });
    }
    __zdReplace(proto, 'has', function (orig) {
      __zdRealHas = orig;
      return function has(name) { return of(this).indexOf(name) >= 0; };
    });
    // Setlike: keys() and values() both iterate the values, entries() yields
    // [value, value] pairs, and forEach's callback takes (value, value, set).
    __zdReplace(proto, 'keys', function () {
      return function keys() { return of(this).slice()[Symbol.iterator](); };
    });
    var values = __zdReplace(proto, 'values', function () {
      return function values() { return of(this).slice()[Symbol.iterator](); };
    });
    __zdReplace(proto, 'entries', function () {
      return function entries() {
        return of(this).map(function (n) { return [n, n]; })[Symbol.iterator]();
      };
    });
    __zdReplace(proto, 'forEach', function () {
      return function forEach(cb, thisArg) {
        var own = of(this);
        for (var i = 0; i < own.length; i++) cb.call(thisArg, own[i], own[i], this);
      };
    });
    // A setlike interface's default iterator IS its `values` method, so the two
    // must stay the same function object — `features[Symbol.iterator] ===
    // features.values` holds on a real one.
    var di = Object.getOwnPropertyDescriptor(proto, Symbol.iterator);
    if (di) {
      Object.defineProperty(proto, Symbol.iterator, {
        value: values, writable: !!di.writable, enumerable: !!di.enumerable, configurable: true
      });
    }
    return true;
  }

  // Both are no-ops when the WebGPU IDL is absent, and neither installs a
  // global, so they are safe to attempt unconditionally — including on the
  // opaque-origin page where `navigator.gpu` itself is missing.
  var limitsServed = false, featuresServed = false;
  var __zdServedFeatures = features ? features.slice() : [];
  try { limitsServed = __zdServeLimits(limits); } catch (e) {}
  try { featuresServed = __zdServeFeatures(features); } catch (e) {}

  // Fallbacks for the FABRICATED adapter below, used only where the real
  // classes do not exist and the overrides above could not run.
  var featureSet = null;
  if (features) {
    try { featureSet = new Set(features); } catch (e) { featureSet = null; }
  }

  // The prefix Chrome puts on every error `requestDevice` rejects with
  // (measured, Chrome 15x / Apple M4 Pro).
  var __zdRdPfx = "Failed to execute 'requestDevice' on 'GPUAdapter': ";

  // Reconcile a GPUDeviceDescriptor against what the adapter ADVERTISES.
  //
  // Returns `{ descriptor, limits, features }` — the descriptor to hand the
  // real adapter (null to pass the caller's through untouched), the limit
  // values to report on the resulting device, and the feature names to report
  // on it. Throws exactly what Chrome throws when the request exceeds the
  // advertisement.
  function __zdReconcile(adapter, desc, doLimits, doFeatures) {
    var claimed = Object.create(null), keepLimits = null, changed = false;
    var req = desc && desc.requiredLimits;
    if (doLimits && req && typeof req === 'object') {
      keepLimits = {};
      Object.keys(req).forEach(function (k) {
        var v = req[k];
        if (v === undefined) return; // Chrome skips undefined values, unknown key or not
        keepLimits[k] = v;
        var d = Object.getOwnPropertyDescriptor(GPUSupportedLimits.prototype, k);
        if (!d || typeof d.get !== 'function') {
          throw new DOMException(
            __zdRdPfx + 'The limit "' + k + '" with a non-undefined value is not recognized.',
            'OperationError');
        }
        v = Number(v);
        var advertised = adapter.limits[k]; // what the page was told, so what it is held to
        // Limits whose name starts with `min` are the spec's ALIGNMENT class,
        // where a smaller value is the better device — so there the rejection
        // is for asking BELOW the advertisement, and the translation below
        // hands the hardware the larger (weaker) of the two. Every other limit
        // is the "maximum" class, where it is the other way round. Measured
        // both ways on a real adapter, including the messages.
        var alignment = k.slice(0, 3) === 'min';
        if (alignment ? (v < advertised) : (v > advertised)) {
          throw new DOMException(
            __zdRdPfx + 'Required limit (' + v + ') is ' + (alignment ? 'lower' : 'greater') +
            ' than the supported limit (' + advertised + ').\n - While validating ' + k +
            '\n - While validating required limits\n',
            'OperationError');
        }
        claimed[k] = v;
        // Within the advertisement but beyond the hardware: ask the hardware
        // for what it can actually give, so the call succeeds, and report the
        // requested value on the device (see `__zdDeviceLimits`).
        var real = __zdRealLimit[k] ? __zdRealLimit[k].call(adapter.limits) : advertised;
        var give = alignment ? Math.max(v, real) : Math.min(v, real);
        if (give !== v) { keepLimits[k] = give; changed = true; }
      });
    }

    // Chrome adds this to every device it creates from a core adapter,
    // requested or not (measured: `requiredFeatures: []` still yields a
    // one-entry set). It is a feature name like any other, so it is only
    // reported when the advertisement carries it.
    var CORE = 'core-features-and-limits';
    var reported = null, keepFeatures = null;
    var want = desc ? desc.requiredFeatures : null;
    if (doFeatures && want !== undefined && want !== null) {
      var list = null;
      // A `requiredFeatures` that is not iterable rejects in Chrome's own IDL
      // conversion, with a message this patch has no business reproducing —
      // leave it alone and let the real call answer.
      try { list = Array.from(want); } catch (e) { list = null; }
      if (list) {
        reported = [];
        keepFeatures = [];
        if (__zdServedFeatures.indexOf(CORE) >= 0) reported.push(CORE);
        list.forEach(function (f) {
          var name = String(f);
          if (__zdServedFeatures.indexOf(name) < 0) {
            // Not advertised → the same rejection Chrome gives for a feature
            // the adapter lacks. Without this, a page could hold a device with
            // a feature `adapter.features.has(...)` just said was absent.
            throw new TypeError(__zdRdPfx + 'Unsupported feature: ' + name);
          }
          if (reported.indexOf(name) < 0) reported.push(name);
          // Advertised but not really there → drop it from the real request so
          // the call still succeeds, and report it anyway.
          if (__zdRealHas && __zdRealHas.call(adapter.features, name)) keepFeatures.push(name);
          else changed = true;
        });
      }
    } else if (doFeatures) {
      reported = __zdServedFeatures.indexOf(CORE) >= 0 ? [CORE] : [];
    }

    var out = { descriptor: null, limits: doLimits ? claimed : null, features: reported };
    if (changed) {
      // Only rebuild when something actually moved, so an untouched request
      // reaches the real adapter as the caller's own object. The two members
      // already read above are carried over rather than read again: Chrome's
      // own IDL conversion reads each dictionary member exactly once, and a
      // descriptor built out of getters would see the difference.
      out.descriptor = {};
      Object.keys(desc).forEach(function (k) {
        if (k !== 'requiredLimits' && k !== 'requiredFeatures') out.descriptor[k] = desc[k];
      });
      out.descriptor.requiredLimits = keepLimits || req;
      out.descriptor.requiredFeatures = keepFeatures || want;
    }
    return out;
  }

  // Decorate path: only relevant when a real GPUAdapter class is present.
  // `limits` / `features` are already served through their own prototypes
  // above, so what is left here is `info`, which has no class of its own to
  // hang values on, and `requestDevice`, which has to agree with them.
  if (gpuPresent) {
    try {
      if (typeof GPUAdapter !== 'undefined' && GPUAdapter.prototype) {
        var di2 = Object.getOwnPropertyDescriptor(GPUAdapter.prototype, 'info');
        if (di2 && typeof di2.get === 'function') {
          __zdGetter(GPUAdapter.prototype, 'info', function () { return info; }, { enumerable: di2.enumerable });
        }
      }
      // A device carries its adapter's identity too, and it was answering the
      // REAL one: measured on a spoofed Win32 persona over a Metal host, the
      // adapter's info named the claimed GPU while `device.adapterInfo.vendor`
      // named the host's, one line later. Same object, same reasoning as the
      // adapter's `info`.
      if (typeof GPUDevice !== 'undefined' && GPUDevice.prototype) {
        var di3 = Object.getOwnPropertyDescriptor(GPUDevice.prototype, 'adapterInfo');
        if (di3 && typeof di3.get === 'function') {
          __zdGetter(GPUDevice.prototype, 'adapterInfo', function () { return info; }, { enumerable: di3.enumerable });
        }
      }
    } catch (e) {}
    // The adapter now advertises the TIER's capabilities, so a page can ask for
    // exactly what it was just told and — before this — be refused by the real
    // hardware. Two lines, no corpus: `a.limits.maxStorageBuffersPerShaderStage`
    // reads 16 off the D3D11 tier, and
    // `a.requestDevice({requiredLimits:{maxStorageBuffersPerShaderStage:16}})`
    // rejected on a 10-buffer Metal host where real Chrome on such an adapter
    // resolves. It bites on exactly the limits that differ between tiers, which
    // are the only ones worth serving at all. Wrapping `requestDevice` makes the
    // claim self-consistent in both directions — a request ABOVE the
    // advertisement rejects like Chrome's, and one within it is translated down
    // to what the hardware can give so the call succeeds.
    //
    // The ceiling this deliberately does NOT clear: a page that goes on to
    // ALLOCATE at the claimed capability still fails, because no patch can
    // conjure hardware — a 16-storage-buffer bind group layout on a 10-buffer
    // Metal device is refused by the driver at creation time, and a shader
    // compiled against it fails to validate. What closes here is the
    // INTERROGATION divergence (what the API answers when asked), not the
    // capability gap (what the GPU can do). Same honest limit as SwiftShader's
    // pixels not being an NVIDIA GPU's pixels.
    try {
      if ((limitsServed || featuresServed) && typeof GPUAdapter !== 'undefined' &&
          GPUAdapter.prototype && typeof GPUAdapter.prototype.requestDevice === 'function') {
        __zdReplace(GPUAdapter.prototype, 'requestDevice', function (orig) {
          return function requestDevice(descriptor) {
            var adapter = this, args = arguments, plan;
            try {
              plan = __zdReconcile(adapter, descriptor, limitsServed, featuresServed);
            } catch (e) {
              return Promise.reject(e);
            }
            if (plan.descriptor) args = [plan.descriptor];
            return Promise.resolve(orig.apply(adapter, args)).then(function (device) {
              // `GPUDevice.limits` / `.features` are `[SameObject]`, so
              // registering what comes back once covers every later read.
              try {
                if (plan.limits && device && device.limits) __zdDeviceLimits.set(device.limits, plan.limits);
                if (plan.features && device && device.features) __zdDeviceFeatures.set(device.features, plan.features);
              } catch (e) {}
              return device;
            });
          };
        });
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
    // Where the real classes exist, the synthetic adapter hands back genuine
    // GPUSupportedLimits / GPUSupportedFeatures instances with no own
    // properties at all: the prototype overrides installed above are what
    // supply the values, so brand, `constructor.name` and own-property shape
    // are a real one's. Only a host with no WebGPU IDL at all — case (b), the
    // opaque-origin page — falls back to the plain object / Set.
    var syntheticLimits = limitsServed
      ? Object.create(GPUSupportedLimits.prototype)
      : (limits || {});
    var syntheticFeatures = featuresServed
      ? Object.create(GPUSupportedFeatures.prototype)
      : (featureSet || new Set());
    __zdGetter(synthetic, 'info', function () { return info; }, { enumerable: false });
    __zdGetter(synthetic, 'limits', function () { return syntheticLimits; }, { enumerable: false });
    __zdGetter(synthetic, 'features', function () { return syntheticFeatures; }, { enumerable: false });
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
