// Pixel-readback farbling, applied identically through every path that can
// read a canvas back.
//
// The perturbation is a **palette**: a pure function from an 8-bit channel
// value to a perturbed one, built once from the seed. That is what makes it
// coherent, and the previous position-keyed scheme was detectable twice over
// without it.
//
// 1. A flat fill must come back flat. Keying the noise on a pixel's index gave
//    every pixel an independent draw, so a canvas cleared to one colour read
//    back as several. No GPU does that: rendering is a function of its input,
//    so identical pixels come back identical. Measured before this rewrite — a
//    uniform WebGL clear read back with red values {62, 64, 66}.
// 2. Every readback path must agree. `readPixels` returns rows bottom-up and
//    `getImageData` returns them top-down, so a position-keyed scheme cannot
//    agree with itself across paths even in principle. A page could render one
//    scene, read it both ways and compare. A palette has no position to
//    disagree about.
//
// Anti-linkage survives. A different seed permutes the palette differently, so
// the image hash still differs per persona, and it is still stable across
// repeat reads because the mapping is a pure function.
(function (seed) {
  // One table per colour channel, mapping a value to itself +/-1. Alpha is
  // left alone: it is rarely part of a fingerprint and perturbing it changes
  // compositing.
  var DELTAS = (function () {
    var out = [];
    for (var ch = 0; ch < 3; ch++) {
      var rng = __zdKeyedRng(seed, [ch]);
      var table = new Int8Array(256);
      for (var v = 0; v < 256; v++) {
        // Clamp at the ends so the table never leaves the byte range.
        table[v] = v === 0 ? 1 : v === 255 ? -1 : (rng() < 0.5 ? -1 : 1);
      }
      out.push(table);
    }
    return out;
  })();

  function farble(data) {
    for (var i = 0; i + 3 < data.length; i += 4) {
      var r = data[i], g = data[i + 1], b = data[i + 2];
      data[i] = r + DELTAS[0][r];
      data[i + 1] = g + DELTAS[1][g];
      data[i + 2] = b + DELTAS[2][b];
    }
    return data;
  }

  var origGetImageData = CanvasRenderingContext2D.prototype.getImageData;

  __zdReplace(CanvasRenderingContext2D.prototype, 'getImageData', function (orig) {
    return function () {
      var img = orig.apply(this, arguments);
      farble(img.data);
      return img;
    };
  });

  // Draw the farbled pixels, let the browser encode, then restore the
  // originals so the visible canvas is untouched.
  //
  // Shared by toDataURL and toBlob. Patching one and not the other is the same
  // contradiction this file exists to remove, just read through two export
  // paths instead of two readback paths.
  function withFarbledPixels(canvas, encode) {
    var ctx = canvas.getContext('2d');
    if (!ctx || canvas.width <= 0 || canvas.height <= 0) return encode();
    var original = origGetImageData.call(ctx, 0, 0, canvas.width, canvas.height);
    var copy = new ImageData(
      new Uint8ClampedArray(original.data),
      canvas.width,
      canvas.height
    );
    farble(copy.data);
    ctx.putImageData(copy, 0, 0);
    try {
      return encode();
    } finally {
      ctx.putImageData(original, 0, 0);
    }
  }

  __zdReplace(HTMLCanvasElement.prototype, 'toDataURL', function (orig) {
    return function () {
      var self = this;
      var args = arguments;
      return withFarbledPixels(this, function () {
        return orig.apply(self, args);
      });
    };
  });

  __zdReplace(HTMLCanvasElement.prototype, 'toBlob', function (orig) {
    return function () {
      var self = this;
      var args = arguments;
      withFarbledPixels(this, function () {
        return orig.apply(self, args);
      });
    };
  });

  // A read shaped like a fingerprint probe rather than like real work.
  //
  // Probes read the whole (small) drawing buffer as bytes. GPU picking reads a
  // 1x1 or small sub-rectangle, usually off a large buffer, and compares the
  // result against an exact id colour, so perturbing it breaks the page. Float
  // readbacks are compute output, where a +/-1 LSB change is meaningless and
  // possibly ruinous.
  //
  // The residue, stated rather than hidden: a page that reads 1x1 *and* reads
  // the full buffer can see that only one of them moved. That is a contrived
  // probe, and the alternative is breaking picking on real sites.
  function fingerprintShaped(gl, w, h, format, type) {
    if (type !== gl.UNSIGNED_BYTE || format !== gl.RGBA) return false;
    var bw = gl.drawingBufferWidth;
    var bh = gl.drawingBufferHeight;
    if (!bw || !bh || bw > 512 || bh > 512) return false;
    return w * h >= 0.9 * bw * bh;
  }

  [window.WebGLRenderingContext, window.WebGL2RenderingContext].forEach(function (Ctor) {
    if (!Ctor || !Ctor.prototype) return;
    __zdReplace(Ctor.prototype, 'readPixels', function (orig) {
      return function (x, y, w, h, format, type, pixels) {
        var out = orig.apply(this, arguments);
        if (
          pixels &&
          pixels.BYTES_PER_ELEMENT === 1 &&
          fingerprintShaped(this, w, h, format, type)
        ) {
          farble(pixels);
        }
        return out;
      };
    });
  });
})(SEED);
