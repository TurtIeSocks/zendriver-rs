//! Where the catalogue's two inputs come from, and at which commit.
//!
//! Both are fetched rather than vendored, because both are reachable at an
//! immutable commit SHA. That keeps regeneration reproducible without copying
//! 1.5 MB of `pci.ids` into the tree or redistributing a dual BSD-3/GPLv2 file.
//!
//! Pinning against `pci-ids.ucw.cz` directly is not an option: it serves a
//! moving file with no content-addressed URL, so the catalogue could change
//! with no input in this repo changing. `pciutils/pciids` is a read-only git
//! mirror of that same database, regenerated automatically, which is what makes
//! a pin possible at all.

/// Read-only git mirror of the PCI ID Database.
pub const PCI_IDS_COMMIT: &str = "e91752832f366923b29d518f4d5e58abd0ccb917";

/// The fingerprint corpus whose `videoCard` node carries driver-reported
/// renderer strings.
///
/// The runtime cache in `zendriver-fingerprints` tracks `master`, which is
/// right for a cache and wrong for a generator: the committed catalogue must
/// not change unless a constant in this file changes.
pub const CORPUS_COMMIT: &str = "4d97621b824fceac5a1e6ebbbdf3d616f6fabca4";

/// Raw URL for the pinned `pci.ids`.
#[must_use]
pub fn pci_ids_url() -> String {
    format!("https://raw.githubusercontent.com/pciutils/pciids/{PCI_IDS_COMMIT}/pci.ids")
}

/// Raw URL for the pinned fingerprint network definition.
#[must_use]
pub fn corpus_url() -> String {
    format!(
        "https://raw.githubusercontent.com/apify/fingerprint-suite/{CORPUS_COMMIT}\
         /packages/fingerprint-generator/src/data_files/fingerprint-network-definition.zip"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sources_are_pinned_to_a_full_commit_sha() {
        // A branch name here would let the committed catalogue change without
        // any input in this repo changing, which is the failure mode hardest
        // to notice: nothing in a diff would show it.
        for (label, pin) in [("pci.ids", PCI_IDS_COMMIT), ("corpus", CORPUS_COMMIT)] {
            assert_eq!(pin.len(), 40, "{label} pin must be a full SHA, got {pin:?}");
            assert!(
                pin.chars().all(|c| c.is_ascii_hexdigit()),
                "{label} pin must be hex, got {pin:?}"
            );
        }
    }

    #[test]
    fn both_urls_embed_their_pin() {
        assert!(pci_ids_url().contains(PCI_IDS_COMMIT));
        assert!(corpus_url().contains(CORPUS_COMMIT));
        // Line continuations in the format string must not leave whitespace in
        // the path.
        assert!(
            !corpus_url().contains(' '),
            "corpus URL has a space in it: {}",
            corpus_url()
        );
    }
}
