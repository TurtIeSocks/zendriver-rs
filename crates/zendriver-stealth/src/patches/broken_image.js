// Defeats: bot.sannysoft.com `Broken Image Dimensions` row.
// Real Chrome reports naturalWidth=16 for an unloaded broken-icon <img>.
// Headless reports 0. Patch the getter to return 16 when the img has no src.
const origNaturalWidth  = Object.getOwnPropertyDescriptor(HTMLImageElement.prototype, 'naturalWidth')?.get;
const origNaturalHeight = Object.getOwnPropertyDescriptor(HTMLImageElement.prototype, 'naturalHeight')?.get;
if (origNaturalWidth && origNaturalHeight) {
    __zdGetter(HTMLImageElement.prototype, 'naturalWidth', function() {
            const v = origNaturalWidth.call(this);
            if (v === 0 && this.complete && !this.src) return 16;
            return v;
        }, { enumerable: true });
    __zdGetter(HTMLImageElement.prototype, 'naturalHeight', function() {
            const v = origNaturalHeight.call(this);
            if (v === 0 && this.complete && !this.src) return 16;
            return v;
        }, { enumerable: true });
}
