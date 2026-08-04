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
// This patch REPAIRS those two gaps. It does not choose a resolution OR the
// insets: the size comes from whatever `setDeviceMetricsOverride` already
// established, and the insets come from the caller when it has measured them.
// A caller
// that configures 1366x768 gets a coherent 1366x768 and not a silent 1920x1080.
// Earlier versions hardcoded 1920x1080 here, which overrode the caller's own
// metrics — invisible while every caller happened to use 1920x1080, and wrong the
// moment one did not.
(function () {
  // Read the size CDP already applied rather than asserting one. Falls back to the
  // window's own inner size, then to a desktop default, so the patch still produces
  // a coherent set if it somehow runs before the metrics override lands.
  const W = window.screen.width || window.innerWidth || 1920;
  const H = window.screen.height || window.innerHeight || 1080;

  // Insets. The caller substitutes these when it is REPLAYING a measured device;
  // `null` falls back to the derived defaults below.
  //
  // The defaults are a plausible fiction and nothing more: they exist so a caller
  // who supplies no capture still gets `inner < outer` and `availHeight < height`,
  // which is where the tell lives. They CANNOT describe a real machine — a real
  // macOS reports `height - 25` minus the dock, a real Windows `- 40/48/72`
  // depending on DPI scaling, or `- 0` with the taskbar auto-hidden. A capture
  // taken on someone else's hardware is presentable only because these are now
  // parameters.
  const AVAIL_W = ZD_AVAIL_WIDTH;
  const AVAIL_H = ZD_AVAIL_HEIGHT;
  const INNER_H = ZD_INNER_HEIGHT;
  const CHROME_H = 86;
  const TASKBAR_H = 48;

  const set = (obj, prop, val) =>
    __zdGetter(obj, prop, () => val, { enumerable: true });

  // screen.* — a real monitor has a work area smaller than the panel. width/height
  // are deliberately NOT re-set: CDP owns them, and rewriting them here is what
  // overrode the caller.
  set(window.screen, 'availWidth', AVAIL_W === null ? W : AVAIL_W);
  set(window.screen, 'availHeight', Math.max(1, AVAIL_H === null ? H - TASKBAR_H : AVAIL_H));
  set(window.screen, 'availLeft', 0);
  set(window.screen, 'availTop', 0);

  // window.* — outer is the whole window; inner is smaller by the browser chrome,
  // so inner < outer always holds. outer is bounded by the screen so a window can
  // never be reported larger than the display containing it.
  set(window, 'outerWidth', W);
  set(window, 'outerHeight', H);
  set(window, 'innerWidth', W);
  set(window, 'innerHeight', Math.max(1, INNER_H === null ? H - CHROME_H : INNER_H));
  set(window, 'screenX', 0);
  set(window, 'screenY', 0);
  set(window, 'screenLeft', 0);
  set(window, 'screenTop', 0);

  if (window.screen.orientation) {
    const landscape = W >= H;
    set(window.screen.orientation, 'type', landscape ? 'landscape-primary' : 'portrait-primary');
    set(window.screen.orientation, 'angle', 0);
  }
})();
