// Coherent window/screen geometry.
//
// `Emulation.setDeviceMetricsOverride` (observer.rs) sets window.inner* and
// screen.width/height, but the CDP command CANNOT touch window.outer* or
// screen.avail*. In headless the real OS window stays at its default (~756x556),
// so the tab reports window.innerWidth (1920) > window.outerWidth (756) — a
// relationship that is physically impossible on real hardware (content can never
// be wider than its window) — AND screen.availHeight === screen.height (no
// taskbar inset, the kiosk/headless signature). reese84/Incapsula reads window +
// screen geometry in one pass; both are cheap, deterministic, high-weight bot
// tells.
//
// Force one coherent desktop profile across EVERY geometry property (mirrors the
// known-good xilriws-targetfp screen.js): outer >= inner, availHeight < height.
(function () {
  const W = 1920;
  const H = 1080;
  const set = (obj, prop, val) =>
    __zdGetter(obj, prop, () => val, { enumerable: true });

  // screen.* — real monitor with a taskbar inset.
  set(window.screen, 'width', W);
  set(window.screen, 'height', H);
  set(window.screen, 'availWidth', W);
  set(window.screen, 'availHeight', H - 48); // Windows taskbar
  set(window.screen, 'availLeft', 0);
  set(window.screen, 'availTop', 0);
  set(window.screen, 'colorDepth', 24);
  set(window.screen, 'pixelDepth', 24);

  // window.* — outer is the whole window; inner is smaller by the browser chrome
  // (title + tabs + bookmarks + omnibox), so inner < outer always holds.
  set(window, 'outerWidth', W);
  set(window, 'outerHeight', H);
  set(window, 'innerWidth', W);
  set(window, 'innerHeight', H - 86);
  set(window, 'screenX', 0);
  set(window, 'screenY', 0);
  set(window, 'screenLeft', 0);
  set(window, 'screenTop', 0);
  set(window, 'devicePixelRatio', 1);

  if (window.screen.orientation) {
    set(window.screen.orientation, 'type', 'landscape-primary');
    set(window.screen.orientation, 'angle', 0);
  }
})();
