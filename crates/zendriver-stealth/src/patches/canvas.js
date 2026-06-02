(function (seed) {
  const rng = __zdRng(seed);
  function farble(data) {
    for (let i = 0; i < data.length; i += 4) {
      // +/-1 LSB perturbation on RGB, deterministic per seed.
      data[i]     = Math.max(0, Math.min(255, data[i]     + (rng() < 0.5 ? -1 : 1)));
      data[i + 1] = Math.max(0, Math.min(255, data[i + 1] + (rng() < 0.5 ? -1 : 1)));
      data[i + 2] = Math.max(0, Math.min(255, data[i + 2] + (rng() < 0.5 ? -1 : 1)));
    }
    return data;
  }
  const origGetImageData = CanvasRenderingContext2D.prototype.getImageData;
  CanvasRenderingContext2D.prototype.getImageData = function (...args) {
    const img = origGetImageData.apply(this, args);
    farble(img.data);
    return img;
  };
  const origToDataURL = HTMLCanvasElement.prototype.toDataURL;
  HTMLCanvasElement.prototype.toDataURL = function (...args) {
    const ctx = this.getContext('2d');
    if (ctx && this.width > 0 && this.height > 0) {
      const orig = origGetImageData.call(ctx, 0, 0, this.width, this.height);
      const copy = new ImageData(new Uint8ClampedArray(orig.data), this.width, this.height);
      farble(copy.data);
      ctx.putImageData(copy, 0, 0);
      const url = origToDataURL.apply(this, args);
      ctx.putImageData(orig, 0, 0);
      return url;
    }
    return origToDataURL.apply(this, args);
  };
})(SEED);
