// Defeats: bot.sannysoft.com `Chrome (New)` row.
if (!window.chrome) {
    window.chrome = { runtime: {} };
} else if (!window.chrome.runtime) {
    window.chrome.runtime = {};
}
