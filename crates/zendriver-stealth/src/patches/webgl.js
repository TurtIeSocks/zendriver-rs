// Coherent WebGL surface driven by one substituted profile object.
//
// The table serves the values that identify the *device*: the two
// DEBUG_renderer_info UNMASKED strings (only once that extension has been
// fetched, exactly as Chrome gates them), the plain VENDOR/RENDERER, the
// static capability enums (MAX_*, ALIASED_*_RANGE, SUBPIXEL_BITS, the version
// strings, ...), getShaderPrecisionFormat, and a per-context-version extension
// list.
//
// Everything else falls through to the real backend, which is not a gap but
// the design. `getParameter` also answers per-context *mutable state* —
// VIEWPORT, SCISSOR_BOX, BLEND, every STENCIL_*/PACK_*/UNPACK_*, DRAW_BUFFERn,
// and the rest — plus values fixed by the context attributes the page asked
// for (RED_BITS, STENCIL_BITS, SAMPLES, ...) and the extension-dependent
// COMPRESSED_TEXTURE_FORMATS. None of those carry device entropy (every real
// Chrome reports the same defaults), and serving them from a table is a live
// tell rather than a spoof: `gl.enable(gl.BLEND); gl.getParameter(gl.BLEND)`
// would answer false forever, a resized canvas would report the stale
// [0, 0, 300, 150] viewport beside an 800-wide drawingBufferWidth, and every
// state-caching renderer that saves and restores through getParameter would
// restore the wrong state. The Rust side (`gpu-tier-gen`'s SERVED_CAPS) is
// what enforces the split, and it needs almost no support here: a name the
// table does not carry already delegates — the same path an unknown
// vendor-specific enum takes. The one exception is DRAW_BUFFERn, where a
// served capability can outrun the backend that answers for it; see
// `drawBufferGap` in `patch` below.
//
// Why per-context extension lists: about sixteen WebGL1 extensions are core in
// WebGL2 and a real WebGL2 context does not list them. Serving one array to
// both prototypes claims extensions that cannot exist, which is its own tell.
//
// Why getExtension is patched too: getSupportedExtensions and getExtension must
// agree in both directions. Claiming an extension whose getExtension returns
// null is a one-line detection, and so is handing over an extension the list
// never claimed. Keeping that promise is why the claimed list is the table's
// intersected with what the backend really supports (plus the inert ones,
// which have no behavior to get wrong) rather than the table verbatim.
(function (profile) {
  if (!profile) return;

  var INERT_STUBS = profile.inertStubs || {};

  // Real Chrome gates UNMASKED_VENDOR_WEBGL / UNMASKED_RENDERER_WEBGL on
  // WEBGL_debug_renderer_info having been fetched: before that call
  // getParameter(37445) answers null and raises INVALID_ENUM. Enablement is
  // per context, never per page — two contexts on one document have
  // independent extension state — so the set is keyed by the context object,
  // and it does not survive a context loss (see `lost` below).
  var DEBUG_RENDERER_INFO = 'WEBGL_debug_renderer_info';
  var debugInfoEnabled = new WeakSet();

  function paramsFor(isV2) {
    return isV2 ? profile.params2 : profile.params1;
  }

  function decode(v) {
    // The Rust side tags each value with its GL type so the right typed array
    // reaches the page. An Int32Array where Chrome returns a Float32Array is
    // caught by one instanceof check.
    if (v === null || typeof v !== 'object') return v;
    switch (v.t) {
      case 'i32pair': return new Int32Array(v.v);
      case 'i32quad': return new Int32Array(v.v);
      case 'f32pair': return new Float32Array(v.v);
      case 'f32quad': return new Float32Array(v.v);
      case 'u32list': return new Uint32Array(v.v);
      default: return v.v;
    }
  }

  // Real Chrome answers with a WebGLShaderPrecisionFormat, so a plain object
  // literal fails both `instanceof` and Object.prototype.toString — the same
  // one-line tell the WebGPU patch builds its objects through a prototype to
  // avoid. Accessors rather than data properties so the members report the
  // native `function get rangeMin() { [native code] }` shape, and enumerable
  // so `for (k in fmt)` still yields the three names a real instance inherits.
  //
  // They belong on the interface's own prototype, which is where IDL
  // attributes live: a real fmt has no own properties at all, so
  // `Object.keys(fmt)` is `[]` and `JSON.stringify(fmt)` is `{}`, AND every
  // instance shares one prototype, so
  // `Object.getPrototypeOf(a) === Object.getPrototypeOf(b)` and both are
  // `WebGLShaderPrecisionFormat.prototype`. Building a fresh intermediate
  // prototype per call satisfies only the first and fails the other two — a
  // cheaper one-line probe than the own-key tell it was meant to close.
  //
  // The values therefore live in a WeakMap keyed by instance (the same shape
  // `stubCache` below uses) and the three accessors are installed once on the
  // real prototype. An instance that never came through here — from an
  // unpatched path, or the real backend on a lost context — is absent from the
  // map and falls through to the accessor that was already there, so genuine
  // objects still answer their genuine values and an illegal receiver still
  // earns the browser's own TypeError.
  var PRECISION_KEYS = ['rangeMin', 'rangeMax', 'precision'];
  var precisionValues = new WeakMap();

  function installPrecisionAccessors(proto) {
    for (var i = 0; i < PRECISION_KEYS.length; i++) {
      installPrecisionAccessor(proto, PRECISION_KEYS[i], i);
    }
  }

  function installPrecisionAccessor(proto, key, index) {
    var desc = Object.getOwnPropertyDescriptor(proto, key);
    var origGet = desc && desc.get;
    __zdGetter(
      proto,
      key,
      function () {
        var v = precisionValues.get(this);
        if (v) return v[index];
        return origGet ? origGet.call(this) : undefined;
      },
      { enumerable: desc ? desc.enumerable : true }
    );
  }

  // Chrome exposes WebGLShaderPrecisionFormat as a global, so the synthesized
  // prototype below is only a fallback for a build that does not. It is still
  // shared across instances rather than rebuilt per call, so prototype
  // identity holds there too.
  var precisionProtoPatched = false;
  var fallbackPrecisionProto = null;

  function precisionFormat(p) {
    var Ctor = window.WebGLShaderPrecisionFormat;
    var proto;
    if (Ctor && Ctor.prototype) {
      proto = Ctor.prototype;
      if (!precisionProtoPatched) {
        precisionProtoPatched = true;
        installPrecisionAccessors(proto);
      }
    } else {
      if (!fallbackPrecisionProto) {
        fallbackPrecisionProto = {};
        if (typeof Symbol !== 'undefined' && Symbol.toStringTag) {
          Object.defineProperty(fallbackPrecisionProto, Symbol.toStringTag, {
            value: 'WebGLShaderPrecisionFormat',
            configurable: true,
          });
        }
        installPrecisionAccessors(fallbackPrecisionProto);
      }
      proto = fallbackPrecisionProto;
    }
    var fmt = Object.create(proto);
    precisionValues.set(fmt, p);
    return fmt;
  }

  // Real Chrome answers getExtension with an instance of the extension's own
  // interface. A plain object literal is three one-liners away from telling:
  // Object.prototype.toString reports [object Object] instead of
  // [object WEBGL_debug_renderer_info], `instanceof` fails, and — because IDL
  // constants live on the *prototype* — Object.keys lists the constants where
  // a real instance has no own properties at all. Same fix as precisionFormat
  // above: build over the real interface prototype, which already carries both
  // the constants and the toStringTag. Chrome exposes these interfaces as
  // globals, so the synthesized prototype below is only a fallback; it mirrors
  // the IDL's own shape (constants enumerable and non-writable, on the
  // prototype, plus the toStringTag).
  //
  // Cached per context because real Chrome caches too: two getExtension calls
  // with the same name hand back the identical object, so a stub rebuilt each
  // call fails `gl.getExtension(x) === gl.getExtension(x)`.
  var stubCache = new WeakMap();
  function inertExtension(ctx, name, consts) {
    var perContext = stubCache.get(ctx);
    if (!perContext) {
      perContext = Object.create(null);
      stubCache.set(ctx, perContext);
    }
    if (perContext[name]) return perContext[name];

    var Ctor = window[name];
    if (Ctor && Ctor.prototype) {
      perContext[name] = Object.create(Ctor.prototype);
      return perContext[name];
    }
    var proto = {};
    if (typeof Symbol !== 'undefined' && Symbol.toStringTag) {
      Object.defineProperty(proto, Symbol.toStringTag, {
        value: name,
        configurable: true,
      });
    }
    for (var k in consts) {
      Object.defineProperty(proto, k, { value: consts[k], enumerable: true });
    }
    perContext[name] = Object.create(proto);
    return perContext[name];
  }

  function patch(proto, isV2) {
    var table = paramsFor(isV2);
    var exts = isV2 ? profile.extensions2 : profile.extensions1;
    // Blink short-circuits getParameter, getShaderPrecisionFormat and
    // getExtension once the context is lost — all three answer null — and
    // getSupportedExtensions below already honors that. Answering the other
    // three from the table lets one loseContext() call catch the patch
    // contradicting itself in two adjacent expressions. Delegating rather
    // than hardcoding null leaves the semantics to the browser, including
    // the TypeError an illegal receiver earns.
    var isContextLost = proto.isContextLost;
    function lost(ctx) {
      if (typeof isContextLost !== 'function') return false;
      var isLost;
      try {
        isLost = isContextLost.call(ctx);
      } catch (e) {
        return true; // not a real context; let the original raise its own error
      }
      // Blink drops extension enablement when the context is lost: after a
      // restore the page must call getExtension again before the two UNMASKED
      // enums exist. Forgetting it here keeps the patch honest across
      // loseContext()/restoreContext(), where it otherwise answered from the
      // table while Chrome answered null. Restoration is asynchronous, so a
      // context is observably lost for at least one turn — any read in that
      // window lands here. (A page that touches nothing at all between the
      // loss and the restore keeps the stale enablement; closing that too
      // would mean registering a 'webglcontextlost' listener, which a page can
      // see by hooking addEventListener — a louder tell than the one it fixes.)
      if (isLost) debugInfoEnabled.delete(ctx);
      return isLost;
    }
    // Extension names match case-insensitively per the WebGL spec, so index
    // the claimable list by lower-case name and keep the table's canonical
    // spelling as the value.
    var claimable = Object.create(null);
    for (var i = 0; i < exts.length; i++) claimable[exts[i].toLowerCase()] = exts[i];

    // DRAW_BUFFERn, keyed by enum number, for every index the served
    // MAX_DRAW_BUFFERS claims to have.
    //
    // Whether a DRAW_BUFFERn enum answers at all is a property of the *real*
    // backend, not of the number this table serves. So a persona serving
    // MAX_DRAW_BUFFERS 8 over a backend that has 6 leaves DRAW_BUFFER6/7
    // answering null beside a cap that says they must exist — and no driver
    // reports 8 and then refuses DRAW_BUFFER6, because ES 3.0 derives the
    // valid range from MAX_DRAW_BUFFERS itself. That pair is the default
    // configuration rather than a corner case: `StealthProfile::spoofed()`
    // runs on SwiftShader (6) while a Win32 or MacIntel persona serves 8.
    //
    // The gap is filled with what a real device of the claimed size answers,
    // which is not one value — it depends on what is bound:
    //
    //   * Default framebuffer: BACK, at *every* in-range index. Measured, not
    //     assumed: all three committed captures record BACK (1029) for
    //     DRAW_BUFFER0 through MAX_DRAW_BUFFERS-1, on the 6-buffer SwiftShader
    //     and on both 8-buffer devices alike.
    //   * A framebuffer object: NONE. BACK is an outright illegal value on a
    //     bound FBO, where ES 3.0 admits only NONE and COLOR_ATTACHMENTi, and
    //     NONE is the initial value it specifies for every index past 0.
    //
    // Answering one of those everywhere would leave the other distinguishable
    // from the device being claimed, so the fill reads the binding instead.
    // That read goes through the captured original `getParameter`, never the
    // patched one, so it can neither recurse nor be perturbed by the table.
    //
    // Filling cannot freeze state a page wrote: drawBuffers() rejects an index
    // the backend does not have, so any index this case reaches is one the page
    // could never have assigned. Every index the backend does know keeps
    // delegating, so a page that calls drawBuffers() still reads back exactly
    // what it set.
    //
    // Indices come from the profile's own enum table rather than from
    // arithmetic on DRAW_BUFFER0, so the numbering stays whatever the captures
    // measured. The three constants below cannot come from there: the captures'
    // `enums` blocks carry only the core enums the probe passed to
    // getParameter, and FRAMEBUFFER_BINDING and NONE are never among them. Both
    // are fixed by the spec rather than measured, the same category (and for
    // the same reason) as the ten `gpu::EXTRA_ENUMS` spells out in Rust.
    //
    // One residue this does not close: the delegated read below has already
    // synthesized the backend's own INVALID_ENUM by the time we answer, and
    // swallowing it would mean consuming an error the page may not have read
    // yet.
    var FRAMEBUFFER_BINDING = 0x8ca6;
    var BACK = 0x0405;
    var NONE = 0;
    var drawBufferGap = Object.create(null);
    var servedMaxDraw = decode(table['MAX_DRAW_BUFFERS']);
    if (typeof servedMaxDraw === 'number') {
      for (var num in profile.enumNames) {
        var m = /^DRAW_BUFFER(\d+)$/.exec(profile.enumNames[num]);
        if (!m) continue;
        if (Number(m[1]) < servedMaxDraw) drawBufferGap[num] = true;
      }
    }

    __zdReplace(proto, 'getParameter', function (orig) {
      return function (param) {
        if (lost(this)) return orig.call(this, param);
        var name = profile.enumNames[param];
        if (
          (name === 'UNMASKED_VENDOR_WEBGL' || name === 'UNMASKED_RENDERER_WEBGL') &&
          !debugInfoEnabled.has(this)
        ) {
          // WEBGL_debug_renderer_info has not been fetched on this context, so
          // these two enums do not exist for it yet — delegating gets the
          // null + INVALID_ENUM the real backend answers with.
          return orig.call(this, param);
        }
        if (name && Object.prototype.hasOwnProperty.call(table, name)) {
          return decode(table[name]);
        }
        var real = orig.call(this, param);
        // A DRAW_BUFFERn the served cap claims but this backend does not have
        // answers null here; see `drawBufferGap` above for why that pair
        // cannot stand, why filling it is safe, and why the value follows the
        // bound framebuffer rather than being fixed.
        if (real === null && drawBufferGap[param]) {
          return orig.call(this, FRAMEBUFFER_BINDING) === null ? BACK : NONE;
        }
        return real;
      };
    });

    __zdReplace(proto, 'getShaderPrecisionFormat', function (orig) {
      return function (shaderType, precisionType) {
        if (lost(this)) return orig.call(this, shaderType, precisionType);
        var key =
          profile.enumNames[shaderType] + '/' + profile.enumNames[precisionType];
        var p = profile.precision[key];
        if (!p) return orig.call(this, shaderType, precisionType);
        return precisionFormat(p);
      };
    });

    __zdReplace(proto, 'getSupportedExtensions', function (orig) {
      return function () {
        var real = orig.call(this);
        if (!real) return real; // a lost context answers null; so do we
        var realSet = Object.create(null);
        for (var i = 0; i < real.length; i++) realSet[real[i].toLowerCase()] = true;
        // A functional extension is claimed only where the backend really
        // provides it, so every claimed method actually works — a stub that
        // lies about a capability the page then CALLS is worse than not
        // claiming it. The inert set is claimed unconditionally, and its bar
        // is narrow: the object must carry constants AND nothing else in the
        // API may consume them, or the stub satisfies getExtension while the
        // consuming call still fails. See `gpu::devices::inert_stubs`.
        var out = [];
        for (var j = 0; j < exts.length; j++) {
          if (realSet[exts[j].toLowerCase()] || INERT_STUBS[exts[j]]) out.push(exts[j]);
        }
        return out;
      };
    });

    __zdReplace(proto, 'getExtension', function (orig) {
      return function (name) {
        // `name` is a required WebIDL argument, so a bare getExtension()
        // raises TypeError in Chrome. Coercing a missing argument to the
        // string "undefined" instead answers null, and arity/throw-shape
        // probing is standard fingerprinting — getParameter and
        // getShaderPrecisionFormat both keep the throw by delegating on a
        // miss, so this one must too. Delegating rather than throwing our own
        // leaves the message (which names the interface) to the browser.
        if (arguments.length < 1) return orig.apply(this, arguments);
        if (lost(this)) return orig.call(this, name);
        var canonical = claimable[String(name).toLowerCase()];
        if (!canonical) return null; // never hand over what we did not claim
        var ext;
        var stub = INERT_STUBS[canonical];
        if (stub) {
          // Inert extension: pure constants, nothing to break. Synthesize it
          // so the claimed list and getExtension agree.
          ext = orig.call(this, canonical) || inertExtension(this, canonical, stub);
        } else {
          // Functional extension: the list above only claims it when the
          // backend really has it, so both answers agree either way.
          ext = orig.call(this, canonical);
        }
        // Handing over WEBGL_debug_renderer_info is what brings its two enums
        // into existence for this context; getParameter above stays silent
        // about them until then.
        if (ext && canonical === DEBUG_RENDERER_INFO) debugInfoEnabled.add(this);
        return ext;
      };
    });
  }

  if (window.WebGLRenderingContext) patch(WebGLRenderingContext.prototype, false);
  if (window.WebGL2RenderingContext) patch(WebGL2RenderingContext.prototype, true);
})(WEBGL_PROFILE);
