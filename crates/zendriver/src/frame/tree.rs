//! One depth-first walk over a `Page.getFrameTree` reply — internal.
//!
//! Chrome answers `Page.getFrameTree` with a recursive
//! `{ frame: {...}, childFrames: [ ... ] }` shape. Two callers traverse it
//! for different reasons — [`crate::frame::lifecycle`] collects every live
//! frame id to confirm a provisional sibling is gone, and
//! `Frame::discover_current_frame_id` collects the children of one parent
//! to re-bind a rejected frame id — and both encode the same protocol
//! fact. It lives here once so a change to Chrome's shape has one place to
//! land, and so the walk has one obvious place to hang a real-Chrome test.
//!
//! Depth is not bounded here. It does not need to be: `serde_json` rejects
//! nesting past its own recursion limit while parsing the reply, so a
//! hostile tree never reaches [`walk`].

use serde_json::Value;

/// The `frame` object at one node of the tree, projected to the fields
/// this crate reads.
///
/// Borrowed from the reply rather than owned — a walk over a large tree
/// allocates nothing, and callers copy only the fields they keep.
pub(crate) struct FrameNode<'a> {
    /// Distance below the walked root; `0` is the root itself. Lets a
    /// caller exclude the tree's own frame, which is not a child of
    /// anything in the tree.
    pub(crate) depth: usize,
    /// CDP `frameId`.
    pub(crate) id: &'a str,
    /// The `frameId` of this node's parent. Absent on the main frame.
    pub(crate) parent_id: Option<&'a str>,
    /// `<frame name>` / `<iframe name>` when Chrome reports one.
    pub(crate) name: Option<&'a str>,
    /// Committed document URL. Chrome reports `""` for a frame that has
    /// been attached but has not navigated yet.
    pub(crate) url: Option<&'a str>,
}

/// Visit every node of `root` depth-first, the root included.
///
/// `root` is the `frameTree` value of a `Page.getFrameTree` reply (i.e.
/// `reply["frameTree"]`), or any `childFrames` entry, which has the same
/// shape.
///
/// A node whose `frame.id` is missing or not a string is skipped — it
/// carries nothing any caller can key on — but its `childFrames` are still
/// descended into, so one malformed node cannot hide a live subtree.
pub(crate) fn walk<'a>(root: &'a Value, visit: &mut impl FnMut(FrameNode<'a>)) {
    walk_at(root, 0, visit);
}

fn walk_at<'a>(node: &'a Value, depth: usize, visit: &mut impl FnMut(FrameNode<'a>)) {
    let frame = &node["frame"];
    if let Some(id) = frame["id"].as_str() {
        visit(FrameNode {
            depth,
            id,
            parent_id: frame["parentId"].as_str(),
            name: frame["name"].as_str(),
            url: frame["url"].as_str(),
        });
    }
    if let Some(children) = node["childFrames"].as_array() {
        for child in children {
            walk_at(child, depth + 1, visit);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tree() -> Value {
        json!({
            "frame": { "id": "ROOT", "url": "https://host.test/" },
            "childFrames": [
                {
                    "frame": { "id": "A", "parentId": "ROOT", "url": "", "name": "sidebar" },
                    "childFrames": [
                        { "frame": { "id": "A1", "parentId": "A", "url": "https://host.test/a1" } }
                    ],
                },
                { "frame": { "id": "B", "parentId": "ROOT", "url": "https://host.test/b" } },
            ],
        })
    }

    #[test]
    fn walks_every_node_depth_first_including_the_root() {
        let t = tree();
        let mut seen = Vec::new();
        walk(&t, &mut |n| seen.push((n.depth, n.id.to_string())));
        assert_eq!(
            seen,
            vec![
                (0, "ROOT".into()),
                (1, "A".into()),
                (2, "A1".into()),
                (1, "B".into()),
            ],
        );
    }

    #[test]
    fn projects_the_fields_callers_read() {
        let t = tree();
        let mut a = None;
        walk(&t, &mut |n| {
            if n.id == "A" {
                a = Some((n.parent_id, n.name, n.url));
            }
        });
        assert_eq!(a.unwrap(), (Some("ROOT"), Some("sidebar"), Some("")));
    }

    /// A node Chrome sent without a usable `frame.id` must not swallow its
    /// children — the subtree below it is still real.
    #[test]
    fn a_node_without_an_id_is_skipped_but_still_descended_into() {
        let t = json!({
            "frame": { "id": "ROOT" },
            "childFrames": [{
                "frame": { "parentId": "ROOT" },
                "childFrames": [{ "frame": { "id": "DEEP", "parentId": "?" } }],
            }],
        });
        let mut seen = Vec::new();
        walk(&t, &mut |n| seen.push(n.id.to_string()));
        assert_eq!(seen, vec!["ROOT".to_string(), "DEEP".to_string()]);
    }
}
