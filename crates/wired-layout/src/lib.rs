//! Split-pane layout of a tab: panes in groups, rows, sizes, focus moves. Pure data with no UI
//! or terminal dependency, so every rule is unit-tested.
#![forbid(unsafe_code)]

pub mod pane_tree;
