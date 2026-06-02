// Coherent WebGPU adapter. Defeats DataDome's navigator.gpu.requestAdapter()
// inconsistency check (upstream #20). Values are dataset-derived from the
// spoofed WebGL renderer (never randomized).
(function (vendor, architecture, description, mode) {
  if (!('gpu' in navigator)) return;            // nothing to patch
  if (mode === 'block') {
    try { Object.defineProperty(navigator, 'gpu', { get: () => undefined }); } catch (e) {}
    return;
  }
  if (vendor === null) return;                  // Native → leave real gpu in place

  const info = { vendor: vendor, architecture: architecture, device: '', description: description };
  const realGpu = navigator.gpu;
  const realRequest = realGpu && realGpu.requestAdapter ? realGpu.requestAdapter.bind(realGpu) : null;

  async function requestAdapter(opts) {
    let adapter = realRequest ? await realRequest(opts) : null;
    if (!adapter) return adapter;               // headless w/o gpu: don't fabricate a whole adapter
    try {
      Object.defineProperty(adapter, 'info', { get: () => info, configurable: true });
      if (adapter.requestAdapterInfo) {
        adapter.requestAdapterInfo = async () => info;
      }
    } catch (e) {}
    return adapter;
  }
  try {
    Object.defineProperty(navigator.gpu, 'requestAdapter', {
      value: requestAdapter, writable: true, configurable: true,
    });
  } catch (e) {}
})(WEBGPU_VENDOR, WEBGPU_ARCHITECTURE, WEBGPU_DESCRIPTION, WEBGPU_MODE);
