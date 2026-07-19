(function (seed) {
  function farble(data) {
    // Reset the PRNG per call, keyed by (seed, content) — NOT one stream
    // shared/advanced across the whole page. `getImageData`/`toDataURL`
    // both farble a snapshot of the identical native pixel data on repeat
    // reads (see call sites below), so keying by content makes the noise
    // stable across reads: same seed + same content => same noise.
    const rng = __zdKeyedRng(seed, data);
    for (let i = 0; i < data.length; i += 4) {
      // +/-1 LSB perturbation on RGB, deterministic per (seed, content).
      data[i]     = Math.max(0, Math.min(255, data[i]     + (rng() < 0.5 ? -1 : 1)));
      data[i + 1] = Math.max(0, Math.min(255, data[i + 1] + (rng() < 0.5 ? -1 : 1)));
      data[i + 2] = Math.max(0, Math.min(255, data[i + 2] + (rng() < 0.5 ? -1 : 1)));
    }
    return data;
  }
  const origGetImageData = CanvasRenderingContext2D.prototype.getImageData;
  __zdReplace(CanvasRenderingContext2D.prototype, 'getImageData', (orig) => function (...args) {
    const img = orig.apply(this, args);
    farble(img.data);
    return img;
  });
  __zdReplace(HTMLCanvasElement.prototype, 'toDataURL', (orig) => function (...args) {
    const ctx = this.getContext('2d');
    if (ctx && this.width > 0 && this.height > 0) {
      const imgData = origGetImageData.call(ctx, 0, 0, this.width, this.height);
      const copy = new ImageData(new Uint8ClampedArray(imgData.data), this.width, this.height);
      farble(copy.data);
      ctx.putImageData(copy, 0, 0);
      const url = orig.apply(this, args);
      ctx.putImageData(imgData, 0, 0);
      return url;
    }
    return orig.apply(this, args);
  });
})(SEED);
