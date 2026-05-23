// Patch platform, hardwareConcurrency, deviceMemory, languages on Navigator.prototype.
// `fp` is the serialized Fingerprint object passed by the bundle factory.
Object.defineProperty(Navigator.prototype, 'platform', {
    get: () => fp.platformJs,
    configurable: true, enumerable: true,
});
Object.defineProperty(Navigator.prototype, 'hardwareConcurrency', {
    get: () => fp.cpuCount,
    configurable: true, enumerable: true,
});
Object.defineProperty(Navigator.prototype, 'deviceMemory', {
    get: () => fp.memoryGb,
    configurable: true, enumerable: true,
});
Object.defineProperty(Navigator.prototype, 'languages', {
    get: () => fp.languages,
    configurable: true, enumerable: true,
});
