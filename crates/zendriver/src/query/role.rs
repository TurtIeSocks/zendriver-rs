//! ARIA role enum + role-to-CSS-selector compilation.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AriaRole {
    Button,
    Link,
    Textbox,
    Combobox,
    Checkbox,
    Radio,
    Tab,
    Menu,
    Menuitem,
    Dialog,
    Heading,
    Banner,
    Navigation,
    Main,
    Article,
    List,
    Listitem,
    Row,
    Cell,
    Columnheader,
    Rowheader,
    /// Escape hatch for ARIA roles not in the enum above.
    Other(&'static str),
}

impl AriaRole {
    /// Compile to a CSS attribute selector. Tag-implicit roles (e.g. `<button>`
    /// implies role=button) are NOT auto-included; users querying by role get
    /// only explicit `[role="..."]` matches. This avoids surprising matches
    /// against tag-implicit elements that may have different accessibility
    /// behavior.
    #[must_use]
    pub fn to_css(self) -> String {
        let name = match self {
            AriaRole::Button => "button",
            AriaRole::Link => "link",
            AriaRole::Textbox => "textbox",
            AriaRole::Combobox => "combobox",
            AriaRole::Checkbox => "checkbox",
            AriaRole::Radio => "radio",
            AriaRole::Tab => "tab",
            AriaRole::Menu => "menu",
            AriaRole::Menuitem => "menuitem",
            AriaRole::Dialog => "dialog",
            AriaRole::Heading => "heading",
            AriaRole::Banner => "banner",
            AriaRole::Navigation => "navigation",
            AriaRole::Main => "main",
            AriaRole::Article => "article",
            AriaRole::List => "list",
            AriaRole::Listitem => "listitem",
            AriaRole::Row => "row",
            AriaRole::Cell => "cell",
            AriaRole::Columnheader => "columnheader",
            AriaRole::Rowheader => "rowheader",
            AriaRole::Other(s) => s,
        };
        format!("[role=\"{name}\"]")
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn button_compiles_to_attribute_selector() {
        assert_eq!(AriaRole::Button.to_css(), r#"[role="button"]"#);
    }

    #[test]
    fn other_role_uses_passed_name() {
        assert_eq!(AriaRole::Other("tooltip").to_css(), r#"[role="tooltip"]"#);
    }

    #[test]
    fn all_roles_compile_snapshot() {
        let all = [
            AriaRole::Button,
            AriaRole::Link,
            AriaRole::Textbox,
            AriaRole::Combobox,
            AriaRole::Checkbox,
            AriaRole::Radio,
            AriaRole::Tab,
            AriaRole::Menu,
            AriaRole::Menuitem,
            AriaRole::Dialog,
            AriaRole::Heading,
            AriaRole::Banner,
            AriaRole::Navigation,
            AriaRole::Main,
            AriaRole::Article,
            AriaRole::List,
            AriaRole::Listitem,
            AriaRole::Row,
            AriaRole::Cell,
            AriaRole::Columnheader,
            AriaRole::Rowheader,
        ];
        let css: Vec<String> = all.iter().map(|r| r.to_css()).collect();
        insta::assert_yaml_snapshot!("aria_role_css_compilation", css);
    }
}
