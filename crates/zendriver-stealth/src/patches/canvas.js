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
    if (ctx) {
      const w = this.width, h = this.height;
      if (w > 0 && h > 0) {
        const img = origGetImageData.call(ctx, 0, 0, w, h);
        farble(img.data);
        ctx.putImageData(img, 0, 0);
      }
    }
    return origToDataURL.apply(this, args);
  };
})(SEED);
