// navigator.userAgentData stub — many headless detectors check this.
// Mirrors what Emulation.setUserAgentOverride sends, but JS-readable.
__zdGetter(Navigator.prototype, 'userAgentData', () => ({
        brands: fp.brands,
        mobile: false,
        platform: fp.chPlatform,
        getHighEntropyValues: __zdMark(function getHighEntropyValues(hints) {
            return Promise.resolve({
                architecture: fp.architecture,
                bitness: fp.bitness,
                brands: fp.brands,
                fullVersionList: fp.fullVersionList,
                mobile: false,
                model: '',
                platform: fp.chPlatform,
                platformVersion: fp.platformVersion,
                wow64: false,
            });
        }, 'getHighEntropyValues', 1),
        toJSON: __zdMark(function toJSON() {
            return { brands: fp.brands, mobile: false, platform: fp.chPlatform };
        }, 'toJSON', 0),
    }), { enumerable: true });
