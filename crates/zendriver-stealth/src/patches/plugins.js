// Defeats: bot.sannysoft.com `Plugins Length (Old)` row.
// Fakes 3 plugins matching Chrome's modern stub layout.
Object.defineProperty(Navigator.prototype, 'plugins', {
    get: function() {
        const make = (name, filename, description) => {
            const p = Object.create(Plugin.prototype);
            Object.defineProperties(p, {
                name:        { value: name },
                filename:    { value: filename },
                description: { value: description },
                length:      { value: 1 },
            });
            return p;
        };
        const arr = [
            make('PDF Viewer',         'internal-pdf-viewer', 'Portable Document Format'),
            make('Chrome PDF Viewer',  'internal-pdf-viewer', 'Portable Document Format'),
            make('Chromium PDF Viewer','internal-pdf-viewer', 'Portable Document Format'),
        ];
        Object.setPrototypeOf(arr, PluginArray.prototype);
        return arr;
    },
    configurable: true,
    enumerable: true,
});
