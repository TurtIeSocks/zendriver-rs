// Headless Chromium lacks proprietary codecs. Stub canPlayType so
// media-feature detection sees 'probably' for common containers.
const origCanPlay = HTMLMediaElement.prototype.canPlayType;
HTMLMediaElement.prototype.canPlayType = function(type) {
    if (typeof type === 'string') {
        const t = type.toLowerCase();
        if (t.includes('avc1') || t.includes('mp4a.40') || t.includes('video/mp4') || t.includes('audio/mp4')) {
            return 'probably';
        }
    }
    return origCanPlay.call(this, type);
};
