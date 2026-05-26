(function () {
    function cookieMap() {
        var out = {};
        var cookies = document.cookie ? document.cookie.split(/; */) : [];
        for (var i = 0; i < cookies.length; i++) {
            var idx = cookies[i].indexOf("=");
            if (idx < 0) continue;
            var name = cookies[i].slice(0, idx).trim();
            var value = cookies[i].slice(idx + 1);
            if (name) out[name] = value;
        }
        return out;
    }

    var cookies = cookieMap();
    var cookieNames = Object.keys(cookies);

    // reese84 may be exactly named or prefixed with __Host- / __Secure-.
    var reese84Key = null;
    for (var i = 0; i < cookieNames.length; i++) {
        var n = cookieNames[i];
        if (n === "reese84" || n.indexOf("reese84") !== -1) {
            reese84Key = n;
            break;
        }
    }
    var reese84 = reese84Key ? cookies[reese84Key] : null;
    if (reese84 === "" || reese84 === "undefined" || reese84 === "null") {
        reese84 = null;
    }

    var hasLegacyCookies = false;
    for (var j = 0; j < cookieNames.length; j++) {
        var n2 = cookieNames[j];
        if (
            n2 === "___utmvc" ||
            n2.indexOf("incap_ses_") === 0 ||
            n2.indexOf("visid_incap_") === 0 ||
            n2 === "nlbi"
        ) {
            hasLegacyCookies = true;
            break;
        }
    }

    var html = document.documentElement
        ? document.documentElement.outerHTML || ""
        : "";

    var bodyHasIncapsulaResource =
        html.indexOf("/_Incapsula_Resource") !== -1 ||
        html.indexOf("/_Incapsula_") !== -1;
    var bodyHasReese =
        html.indexOf("Reese.js") !== -1 ||
        html.indexOf("reese.js") !== -1;
    var bodyHasChallengeMarker =
        html.indexOf("Request unsuccessful. Incapsula") !== -1 ||
        html.indexOf("incident ID") !== -1;

    var captchaKind = null;
    var iframes = document.querySelectorAll("iframe");
    for (var k = 0; k < iframes.length; k++) {
        var src = iframes[k].src || "";
        if (src.indexOf("hcaptcha.com") !== -1 || src.indexOf("hcap.cloud") !== -1) {
            captchaKind = "HCaptcha";
            break;
        }
        if (
            src.indexOf("google.com/recaptcha") !== -1 ||
            src.indexOf("recaptcha.net") !== -1
        ) {
            captchaKind = "Recaptcha";
            break;
        }
        if (src.indexOf("imperva.com/captcha") !== -1) {
            captchaKind = "ImpervaNative";
            break;
        }
    }
    if (
        !captchaKind &&
        (html.indexOf("g-recaptcha") !== -1 || html.indexOf("h-captcha") !== -1)
    ) {
        captchaKind = "Unknown";
    }

    var bodyClean =
        !bodyHasIncapsulaResource && !bodyHasReese && !bodyHasChallengeMarker;

    var hasImpervaSignal =
        !!reese84Key ||
        hasLegacyCookies ||
        bodyHasIncapsulaResource ||
        bodyHasReese ||
        bodyHasChallengeMarker ||
        !!captchaKind;

    var surface;
    if (captchaKind) {
        surface = { kind: "Captcha", captcha: captchaKind };
    } else if (reese84Key || bodyHasReese) {
        surface = { kind: "Reese84" };
    } else if (hasLegacyCookies || bodyHasIncapsulaResource) {
        surface = { kind: "Legacy" };
    } else {
        surface = { kind: "None" };
    }

    var sessionCookies = [];
    for (var m = 0; m < cookieNames.length; m++) {
        var name = cookieNames[m];
        if (
            name === reese84Key ||
            name === "___utmvc" ||
            name.indexOf("incap_ses_") === 0 ||
            name.indexOf("visid_incap_") === 0 ||
            name === "nlbi"
        ) {
            sessionCookies.push({ name: name, value: cookies[name] });
        }
    }

    return {
        surface: surface,
        reese84: reese84,
        body_clean: bodyClean,
        sessions: sessionCookies,
        has_imperva_signal: hasImpervaSignal,
    };
})()
