(function (battery, mediaDevices, voices) {
  if (typeof battery === 'number' && navigator.getBattery) {
    __zdReplace(navigator, 'getBattery', () => function () {
      return Promise.resolve({
        level: battery, charging: true, chargingTime: 0,
        dischargingTime: Infinity,
        addEventListener() {}, removeEventListener() {},
      });
    });
  }
  if (typeof mediaDevices === 'number' && navigator.mediaDevices &&
      navigator.mediaDevices.enumerateDevices) {
    __zdReplace(navigator.mediaDevices, 'enumerateDevices', () => function () {
      const out = [];
      for (let i = 0; i < mediaDevices; i++) {
        out.push({ deviceId: 'dev' + i, kind: 'audioinput', label: '', groupId: 'g' + i });
      }
      return Promise.resolve(out);
    });
  }
  if (Array.isArray(voices) && window.speechSynthesis) {
    __zdReplace(speechSynthesis, 'getVoices', () => function () {
      return voices.map((n) => ({ name: n, lang: 'en-US', default: false,
        localService: true, voiceURI: n }));
    });
  }
})(HW_BATTERY, HW_MEDIA_DEVICES, HW_VOICES);
