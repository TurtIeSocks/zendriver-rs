(function (seed) {
  const rng = __zdRng(seed);
  const orig = AnalyserNode.prototype.getFloatFrequencyData;
  AnalyserNode.prototype.getFloatFrequencyData = function (array) {
    orig.call(this, array);
    for (let i = 0; i < array.length; i++) array[i] += (rng() - 0.5) * 1e-4;
  };
  const origTime = AnalyserNode.prototype.getByteTimeDomainData;
  if (origTime) {
    AnalyserNode.prototype.getByteTimeDomainData = function (array) {
      origTime.call(this, array);
      for (let i = 0; i < array.length; i++) {
        array[i] = Math.max(0, Math.min(255, array[i] + (rng() < 0.5 ? -1 : 1)));
      }
    };
  }
})(SEED);
