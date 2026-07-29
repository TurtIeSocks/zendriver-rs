//! Print the WebGL profile JSON the bootstrap substitutes into `webgl.js`.
//!
//! Feeds the Node harness in `.superpowers/sdd/webgl_harness.js`, which
//! exercises the shipped `webgl.js` against a model of Blink. Reads through
//! the public `bootstrap_script` so the JSON is exactly what a page receives,
//! rather than a re-derivation that could drift from it.
//!
//! Run: `cargo run -p zendriver-stealth --example dump_webgl_profile`

use zendriver_stealth::patches::bootstrap_script;
use zendriver_stealth::{Fingerprint, Persona, Platform, UserAgentMetadata};

fn main() {
    let identity = Fingerprint {
        platform: Platform::MacIntel,
        chrome_major: 120,
        chrome_full: "120.0.6099.234".into(),
        cpu_count: 10,
        memory_gb: 8,
        ua_string: String::new(),
        ua_metadata: UserAgentMetadata::realistic(Platform::MacIntel, 120, "120.0.6099.234"),
        timezone: None,
        locale: Some("en-US".into()),
        languages: None,
        screen: None,
    };
    let script = bootstrap_script(&Persona::default(), &identity);
    // webgl.js's last line is `})(<profile json>);`, and `profile_to_js`
    // emits compact single-line JSON, so the whole object is on that one line.
    let line = script
        .lines()
        .find(|l| l.starts_with("})({") && l.contains("\"params1\":"))
        .expect("no substituted WebGL profile found in the bootstrap");
    let json = line
        .trim_start_matches("})(")
        .trim_end_matches(';')
        .trim_end_matches(')');
    println!("{json}");
}
