// Patch platform, hardwareConcurrency, deviceMemory, languages on Navigator.prototype.
// `fp` is the serialized Fingerprint object passed by the bundle factory.
__zdGetter(Navigator.prototype, 'platform', () => fp.platformJs, { enumerable: true });
__zdGetter(Navigator.prototype, 'hardwareConcurrency', () => fp.cpuCount, { enumerable: true });
__zdGetter(Navigator.prototype, 'deviceMemory', () => fp.memoryGb, { enumerable: true });
__zdGetter(Navigator.prototype, 'languages', () => fp.languages, { enumerable: true });
