// navigator.userAgentData stub — many headless detectors check this.
// Mirrors what Emulation.setUserAgentOverride sends, but JS-readable.
Object.defineProperty(Navigator.prototype, 'userAgentData', {
    get: () => ({
        brands: fp.brands,
        mobile: false,
        platform: fp.chPlatform,
        getHighEntropyValues: function(hints) {
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
        },
        toJSON: function() {
            return { brands: fp.brands, mobile: false, platform: fp.chPlatform };
        }
    }),
    configurable: true, enumerable: true,
});
