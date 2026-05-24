(function () {
    function walk(root) {
        const iframes = root.querySelectorAll
            ? root.querySelectorAll("iframe")
            : [];
        for (const f of iframes) {
            if (f.src && f.src.includes("challenges.cloudflare.com")) {
                const r = f.getBoundingClientRect();
                return { x: r.left, y: r.top, width: r.width, height: r.height };
            }
        }
        const all = root.querySelectorAll ? root.querySelectorAll("*") : [];
        for (const el of all) {
            if (el.shadowRoot) {
                const sub = walk(el.shadowRoot);
                if (sub) return sub;
            }
        }
        return null;
    }
    return walk(document);
})()
