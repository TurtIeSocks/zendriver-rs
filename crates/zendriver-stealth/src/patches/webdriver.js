// Defeats: bot.sannysoft.com `WebDriver (New)` + `WebDriver Advanced` rows.
// Patches Navigator.prototype (not navigator directly) so
// Object.getOwnPropertyNames(navigator) doesn't reveal the hack. Routed through
// __zdGetter so the getter reports `function get webdriver() { [native code] }`.
__zdGetter(Navigator.prototype, 'webdriver', () => false, { enumerable: true });
