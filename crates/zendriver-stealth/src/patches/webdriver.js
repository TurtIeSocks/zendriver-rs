// Defeats: bot.sannysoft.com `WebDriver (New)` + `WebDriver Advanced` rows.
// Patches Navigator.prototype (not navigator directly) so
// Object.getOwnPropertyNames(navigator) doesn't reveal the hack.
Object.defineProperty(Navigator.prototype, 'webdriver', {
    get: () => false,
    configurable: true,
    enumerable: true,
});
