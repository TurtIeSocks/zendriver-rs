(function (seed) {
  if (typeof AnalyserNode === 'undefined') return;
  const rng = __zdRng(seed);
  __zdReplace(AnalyserNode.prototype, 'getFloatFrequencyData', (orig) => function (array) {
    orig.call(this, array);
    for (let i = 0; i < array.length; i++) array[i] += (rng() - 0.5) * 1e-4;
  });
  if (AnalyserNode.prototype.getByteTimeDomainData) {
    __zdReplace(AnalyserNode.prototype, 'getByteTimeDomainData', (orig) => function (array) {
      orig.call(this, array);
      for (let i = 0; i < array.length; i++) {
        array[i] = Math.max(0, Math.min(255, array[i] + (rng() < 0.5 ? -1 : 1)));
      }
    });
  }
})(SEED);
