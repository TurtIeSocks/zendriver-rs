//! Snapshot formatters consumed by snapshot tools.
//!
//! Currently exports [`html_trim`] (drops `<script>` / `<style>` blocks
//! and collapses whitespace). The accessibility-tree-formatter approach
//! described in the original plan was dropped for v0 — see "API Reality"
//! in the plan for the rationale.

pub mod html_trim;
