// `var` + indexed loops, no `for...of`: the crate-wide convention for
// injected source, stated in full on `WALKER_JS` in `bypass.rs`. Short
// version: `for...of` over a `NodeList` calls
// `NodeList.prototype[Symbol.iterator]`, which the page can redefine to
// watch someone walk its DOM.
(function () {
    function walk(root) {
        var iframes = root.querySelectorAll
            ? root.querySelectorAll("iframe")
            : [];
        for (var i = 0; i < iframes.length; i++) {
            var f = iframes[i];
            if (f.src && f.src.includes("challenges.cloudflare.com")) {
                var r = f.getBoundingClientRect();
                return { x: r.left, y: r.top, width: r.width, height: r.height };
            }
        }
        var all = root.querySelectorAll ? root.querySelectorAll("*") : [];
        for (var j = 0; j < all.length; j++) {
            if (all[j].shadowRoot) {
                var sub = walk(all[j].shadowRoot);
                if (sub) return sub;
            }
        }
        return null;
    }
    return walk(document);
})()
