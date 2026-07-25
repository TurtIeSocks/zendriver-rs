// Coherent WebGL surface driven by one substituted profile object.
//
// Every value a fingerprinter can read comes from the table: the two
// DEBUG_renderer_info UNMASKED strings, the plain VENDOR/RENDERER, every
// spec-defined getParameter enum, getShaderPrecisionFormat, and a
// per-context-version extension list. Enums outside the table fall through to
// the real backend, which is correct for vendor-specific enums we do not model.
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
  function precisionFormat(p) {
    var Ctor = window.WebGLShaderPrecisionFormat;
    var o = Ctor && Ctor.prototype ? Object.create(Ctor.prototype) : {};
    __zdGetter(o, 'rangeMin', function () { return p[0]; }, { enumerable: true });
    __zdGetter(o, 'rangeMax', function () { return p[1]; }, { enumerable: true });
    __zdGetter(o, 'precision', function () { return p[2]; }, { enumerable: true });
    return o;
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
      try {
        return isContextLost.call(ctx);
      } catch (e) {
        return true; // not a real context; let the original raise its own error
      }
    }
    // Extension names match case-insensitively per the WebGL spec, so index
    // the claimable list by lower-case name and keep the table's canonical
    // spelling as the value.
    var claimable = Object.create(null);
    for (var i = 0; i < exts.length; i++) claimable[exts[i].toLowerCase()] = exts[i];

    __zdReplace(proto, 'getParameter', function (orig) {
      return function (param) {
        if (lost(this)) return orig.call(this, param);
        var name = profile.enumNames[param];
        if (name && Object.prototype.hasOwnProperty.call(table, name)) {
          return decode(table[name]);
        }
        return orig.call(this, param);
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
        // claiming it. Inert extensions carry nothing but constants, so there
        // is nothing to break and they are claimed unconditionally.
        var out = [];
        for (var j = 0; j < exts.length; j++) {
          if (realSet[exts[j].toLowerCase()] || INERT_STUBS[exts[j]]) out.push(exts[j]);
        }
        return out;
      };
    });

    __zdReplace(proto, 'getExtension', function (orig) {
      return function (name) {
        if (lost(this)) return orig.call(this, name);
        var canonical = claimable[String(name).toLowerCase()];
        if (!canonical) return null; // never hand over what we did not claim
        var stub = INERT_STUBS[canonical];
        if (stub) {
          // Inert extension: pure constants, nothing to break. Synthesize it
          // so the claimed list and getExtension agree.
          var real = orig.call(this, canonical);
          if (real) return real;
          var o = {};
          for (var k in stub) o[k] = stub[k];
          return o;
        }
        // Functional extension: the list above only claims it when the backend
        // really has it, so both answers agree either way.
        return orig.call(this, canonical);
      };
    });
  }

  if (window.WebGLRenderingContext) patch(WebGLRenderingContext.prototype, false);
  if (window.WebGL2RenderingContext) patch(WebGL2RenderingContext.prototype, true);
})(WEBGL_PROFILE);
