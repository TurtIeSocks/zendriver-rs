(function (seed) {
  if (typeof AnalyserNode === 'undefined') return;
  // Same PRNG-lifetime bug/fix as canvas.js: reset per call, keyed by
  // (seed, content) so repeat reads of the identical native sample buffer
  // (no new audio processed between calls) reproduce the same noise.
  __zdReplace(AnalyserNode.prototype, 'getFloatFrequencyData', (orig) => function (array) {
    orig.call(this, array);
    const rng = __zdKeyedRng(seed, array);
    for (let i = 0; i < array.length; i++) array[i] += (rng() - 0.5) * 1e-4;
  });
  if (AnalyserNode.prototype.getByteTimeDomainData) {
    __zdReplace(AnalyserNode.prototype, 'getByteTimeDomainData', (orig) => function (array) {
      orig.call(this, array);
      const rng = __zdKeyedRng(seed, array);
      for (let i = 0; i < array.length; i++) {
        array[i] = Math.max(0, Math.min(255, array[i] + (rng() < 0.5 ? -1 : 1)));
      }
    });
  }
})(SEED);
