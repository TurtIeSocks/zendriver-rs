// Defeats: bot.sannysoft.com `Chrome (New)` row.
//
// Do NOT synthesize window.chrome.runtime. On a normal web page real Chrome
// exposes NO chrome.runtime (it is an extension / privileged-context API);
// injecting an empty `{}` is itself a stealth artifact advanced sensors flag.
// Only create window.chrome (the benign object) when it is missing.
if (!window.chrome) {
    window.chrome = {};
}
