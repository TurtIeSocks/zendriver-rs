// Defeats: bot.sannysoft.com `Permissions (New)` row.
// Real Chrome: Notification.permission === 'default' AND
//   navigator.permissions.query({name:'notifications'}).state === 'prompt'
// Headless: mismatch — Notification.permission is 'denied' but query says 'prompt'.
__zdReplace(navigator.permissions, 'query', (orig) => function(p) {
    if (p && p.name === 'notifications') {
        return Promise.resolve({ state: Notification.permission, onchange: null });
    }
    return orig.call(navigator.permissions, p);
});
