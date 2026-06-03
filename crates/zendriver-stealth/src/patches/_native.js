// Native-function masking prelude.
//
// Runs FIRST inside the bootstrap's single outer IIFE (see patches.rs
// `bootstrap_script`). Declares __zdReplace / __zdGetter / __zdMark as
// closure-locals — they are NOT placed on globalThis, so nothing leaks to the
// page (Object.keys / getOwnPropertyNames(window) stay clean). The only global
// side effect is overriding Function.prototype.toString, which MUST persist
// beyond the IIFE so page scripts that inspect patched functions see native
// code. Because the whole bootstrap is injected via
// Page.addScriptToEvaluateOnNewDocument (every frame), the override is
// re-installed per realm, so cross-realm probes
// (iframe.contentWindow.Function.prototype.toString.call(fn)) also see native.
const __zdFnToString = Function.prototype.toString;
const __zdMarks = new WeakMap(); // fn -> native display string
const __zdNativeStr = (name) => "function " + name + "() { [native code] }";

const __zdFakeToString = function toString() {
  const s = __zdMarks.get(this);
  return s !== undefined ? s : __zdFnToString.call(this);
};
__zdMarks.set(__zdFakeToString, __zdNativeStr("toString"));
__zdMarks.set(__zdFnToString, __zdNativeStr("toString"));
Object.defineProperty(Function.prototype, "toString", {
  value: __zdFakeToString,
  writable: true,
  enumerable: false,
  configurable: true,
});

// Match a native function's own name/length shape
// (writable:false, enumerable:false, configurable:true).
const __zdNameLen = (fn, name, length) => {
  Object.defineProperty(fn, "name", { value: name, configurable: true });
  Object.defineProperty(fn, "length", { value: length, configurable: true });
};

// Mark an arbitrary function native (value-function members, constructors).
const __zdMark = (fn, name, length) => {
  __zdNameLen(fn, name, length);
  __zdMarks.set(fn, __zdNativeStr(name));
  return fn;
};

// Replace obj[prop] (a method) with make(orig), copying the original's
// name/length and marking the result native.
const __zdReplace = (obj, prop, make) => {
  const orig = obj[prop];
  const fn = make(orig);
  const name = orig && orig.name ? orig.name : prop;
  const length =
    orig && typeof orig.length === "number" ? orig.length : fn.length;
  __zdMark(fn, name, length);
  obj[prop] = fn;
  return fn;
};

// Define an accessor whose getter (and optional setter) report the native
// `function get NAME() { [native code] }` form.
const __zdGetter = (obj, prop, getFn, opts) => {
  opts = opts || {};
  __zdNameLen(getFn, "get " + prop, 0);
  __zdMarks.set(getFn, "function get " + prop + "() { [native code] }");
  const desc = { get: getFn, enumerable: !!opts.enumerable, configurable: true };
  if (opts.setFn) {
    __zdNameLen(opts.setFn, "set " + prop, 1);
    __zdMarks.set(opts.setFn, "function set " + prop + "() { [native code] }");
    desc.set = opts.setFn;
  }
  Object.defineProperty(obj, prop, desc);
};
