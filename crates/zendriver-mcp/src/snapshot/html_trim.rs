//! Tiny HTML trimmer used by tools that bundle a "see the page" snapshot.
//!
//! - Strips `<script>...</script>` and `<style>...</style>` blocks (they
//!   bloat snapshot payloads without helping the LLM reason about page
//!   structure).
//! - Collapses runs of whitespace into single spaces and trims the result.
//!
//! Implementation is intentionally crude — not a full HTML parser. A
//! malformed or pathological input (e.g. `<script>` without a closing tag)
//! cuts off the trailing content; we accept that for v0.

/// Trim the rendered HTML for inclusion in an MCP tool response.
pub fn trim(html: &str) -> String {
    let no_scripts = strip_block(html, "script");
    let no_styles = strip_block(&no_scripts, "style");
    collapse_ws(&no_styles)
}

/// Remove every `<tag ...>...</tag>` block (case-sensitive) from `html`.
fn strip_block(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        match rest[start..].find(&close) {
            Some(end_off) => {
                rest = &rest[start + end_off + close.len()..];
            }
            None => {
                // Unbalanced tag — drop everything from `start` onward.
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Collapse runs of whitespace into single spaces and trim the result.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts() {
        let html = "<p>hi</p><script>alert(1)</script><p>bye</p>";
        assert_eq!(trim(html), "<p>hi</p><p>bye</p>");
    }

    #[test]
    fn strips_styles() {
        let html = "<style>p{color:red}</style><p>hi</p>";
        assert_eq!(trim(html), "<p>hi</p>");
    }

    #[test]
    fn collapses_whitespace() {
        let html = "<p>hi\n\n  there</p>";
        assert_eq!(trim(html), "<p>hi there</p>");
    }

    #[test]
    fn unbalanced_script_drops_trailing() {
        let html = "<p>before</p><script>oops";
        assert_eq!(trim(html), "<p>before</p>");
    }
}
