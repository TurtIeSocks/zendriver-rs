// Synthetic pointer entropy.
//
// reese84/Incapsula is a behavioral-biometrics sensor whose flagship signal is
// mouse/pointer motion; a challenge session with ZERO pointer events is a
// top-tier automation signature. When the page subscribes to mousemove /
// mouseover / mouseout on `document`, feed the listener a human-looking moving
// trajectory (mirrors the known-good xilriws-targetfp screen.js). Native
// Math.random / setTimeout are intentional here — behavioral signal wants real
// jitter, not the seeded determinism the fingerprint surfaces use.
(function () {
  const rnd = (a, b) => a + Math.floor(Math.random() * (b - a + 1));
  const W = 1920;
  const H = 1080;
  let active = true;
  let x = rnd(1, W);
  let y = rnd(1, H - 60);
  let outCb = null;
  let overCb = null;

  async function fakeOverOut() {
    let n = 2;
    while (active && n-- > 0) {
      await new Promise((r) => setTimeout(r, rnd(50, 150)));
      const d = { clientX: x, clientY: y, screenX: x, screenY: y - 13 };
      if (outCb) outCb(new MouseEvent('mouseout', d));
      if (overCb) overCb(new MouseEvent('mouseover', d));
    }
  }
  async function fakeMove(cb) {
    let n = 4;
    while (active && n-- > 0) {
      await new Promise((r) => setTimeout(r, rnd(50, 200)));
      x += rnd(2, 20);
      y += rnd(4, 27);
      cb(
        new MouseEvent('mousemove', {
          clientX: x,
          clientY: y,
          screenX: x,
          screenY: y - 13,
        })
      );
    }
  }

  // Activate most (~70-80%) sessions, matching targetfp's gating.
  const doMove = rnd(0, 10) > 2;
  const doOver = rnd(0, 10) > 2;

  __zdReplace(Document.prototype, 'addEventListener', (orig) =>
    function (type, cb) {
      try {
        if (typeof cb === 'function') {
          if (doMove && type === 'mousemove') fakeMove(cb);
          else if (doOver && type === 'mouseout') {
            outCb = cb;
            if (overCb) fakeOverOut();
          } else if (doOver && type === 'mouseover') {
            overCb = cb;
            if (outCb) fakeOverOut();
          }
        }
      } catch (e) {
        /* never let the shim break the page's real listener */
      }
      return orig.apply(this, arguments);
    }
  );
  __zdReplace(Document.prototype, 'removeEventListener', (orig) =>
    function () {
      active = false;
      return orig.apply(this, arguments);
    }
  );
})();
