(function () {
  function readCookie(name) {
    var m = document.cookie.match(new RegExp('(?:^|; )' + name + '=([^;]*)'));
    return m ? decodeURIComponent(m[1]) : null;
  }
  function findCaptchaUrl(root) {
    var iframes = root.querySelectorAll ? root.querySelectorAll('iframe') : [];
    for (var i = 0; i < iframes.length; i++) {
      var f = iframes[i];
      if (f.src && f.src.indexOf('captcha-delivery.com') !== -1) return f.src;
    }
    var all = root.querySelectorAll ? root.querySelectorAll('*') : [];
    for (var j = 0; j < all.length; j++) {
      if (all[j].shadowRoot) {
        var sub = findCaptchaUrl(all[j].shadowRoot);
        if (sub) return sub;
      }
    }
    return null;
  }
  var dd = (typeof window.dd === 'object' && window.dd) ? window.dd : null;
  var captchaUrl = findCaptchaUrl(document);
  var datadome = readCookie('datadome');
  var surface = 'none';
  if (dd && String(dd.t) === 'bv') {
    surface = 'block';
  } else if (captchaUrl) {
    surface = 'captcha';
  } else if (dd) {
    surface = 'device_check';
  }
  var bodyClean = !dd && !captchaUrl;
  return {
    surface: surface,
    datadome: datadome,
    dd: dd ? { cid: dd.cid || null, hsh: dd.hsh || null, t: dd.t || null, host: dd.host || null } : null,
    captcha_url: captchaUrl,
    body_clean: bodyClean
  };
})()
