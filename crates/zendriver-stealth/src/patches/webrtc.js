(function (policy, fakeIp) {
  // policy: "block" | "value" | "native"
  if (policy === 'native') return;
  const RTC = window.RTCPeerConnection || window.webkitRTCPeerConnection;
  if (!RTC) return;
  window.RTCPeerConnection = __zdMark(function RTCPeerConnection(cfg, ...rest) {
    const pc = new RTC(cfg, ...rest);
    const origAdd = pc.addEventListener.bind(pc);
    pc.addEventListener = __zdMark(function addEventListener(type, cb, ...a) {
      if (type === 'icecandidate') {
        const wrapped = function (e) {
          if (policy === 'block' && e && e.candidate) return; // drop local IPs
          if (policy === 'value' && fakeIp && e && e.candidate) {
            try {
              Object.defineProperty(e.candidate, 'address', { value: fakeIp });
            } catch (x) {}
          }
          return cb.apply(this, arguments);
        };
        return origAdd(type, wrapped, ...a);
      }
      return origAdd(type, cb, ...a);
    }, 'addEventListener', 2);
    return pc;
  }, 'RTCPeerConnection', 0);
  window.RTCPeerConnection.prototype = RTC.prototype;
})(WEBRTC_POLICY, WEBRTC_FAKE_IP);
