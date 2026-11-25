//! The split-pane layout of one tab. Panes sit in groups that arrange their parts side by side
//! or stacked. A group never holds a group of its own axis (that group's parts join it instead),
//! so every row or column of panes the user sees is exactly one group. Pure data, no UI, so every
//! rule (split, close, focus moves, divider drags, scrolling rows) is unit-tested.
//!
//! Coordinates are logical pixels; the view snaps them when painting.

use std::collections::{HashMap, HashSet};

/// Identifies a pane within its tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(u64);

/// Identifies a group within its tree. Ids are never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(u64);

/// Identifies a row (see [`PaneTree`]). Ids are never reused, so state kept for a row that is
/// gone can never be applied to a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowId(u64);

/// Identifies a divider: the one between part `index` and part `index + 1` of `group`. Only
/// valid while the tree's [`PaneTree::revision`] is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DividerId {
    /// The group it divides.
    pub group: GroupId,
    /// The part before it.
    pub index: usize,
}

/// How a group arranges its parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Side by side; the dividers are vertical.
    Horizontal,
    /// Stacked; the dividers are horizontal.
    Vertical,
}

/// Where a new pane goes relative to the pane being split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Left of it.
    Left,
    /// Right of it.
    Right,
    /// Above it.
    Top,
    /// Below it.
    Bottom,
}

impl Side {
    fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Horizontal,
            Self::Top | Self::Bottom => Axis::Vertical,
        }
    }

    fn is_new_first(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }
}

/// A direction for moving the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Towards the left edge.
    Left,
    /// Towards the right edge.
    Right,
    /// Towards the top edge.
    Up,
    /// Towards the bottom edge.
    Down,
}

/// An axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl Rect {
    /// A rectangle from its position and size.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }

    fn right(&self) -> f32 {
        self.x + self.width
    }

    fn bottom(&self) -> f32 {
        self.y + self.height
    }

    fn start(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.x,
            Axis::Vertical => self.y,
        }
    }

    fn extent(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.width,
            Axis::Vertical => self.height,
        }
    }

    /// This rectangle with its span along `axis` replaced.
    fn with_span(self, axis: Axis, start: f32, extent: f32) -> Self {
        match axis {
            Axis::Horizontal => Self { x: start, width: extent, ..self },
            Axis::Vertical => Self { y: start, height: extent, ..self },
        }
    }
}

/// Geometry settings for layout.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutParams {
    /// Space between two parts, where the divider line is drawn.
    pub gap: f32,
    /// Smallest width and height of each pane (from its own font and zoom).
    pub min_sizes: HashMap<PaneId, (f32, f32)>,
    /// Smallest size of a pane missing from `min_sizes` (not measured yet).
    pub default_min: (f32, f32),
}

impl LayoutParams {
    fn min(&self, id: PaneId, axis: Axis) -> f32 {
        let (width, height) = self.min_sizes.get(&id).copied().unwrap_or(self.default_min);
        match axis {
            Axis::Horizontal => width,
            Axis::Vertical => height,
        }
    }
}

/// How much of its group a part asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Size {
    /// An even share of what the parts sized by hand leave.
    Auto,
    /// This share (0..1) of the group's content (its length without the gaps), set by hand.
    Share(f32),
}

/// The share of the group's content each part asks for, summing to 1: hand-set shares as they
/// are and the rest split evenly among the automatic parts. Hand-set shares that leave nothing
/// (or a group with no automatic part) are scaled to fill the group exactly.
fn fractions(sizes: &[Size]) -> Vec<f32> {
    let set: f32 = sizes.iter().map(|s| if let Size::Share(v) = s { v.max(0.) } else { 0. }).sum();
    let autos = sizes.iter().filter(|s| **s == Size::Auto).count();
    let auto = if autos > 0 { (1. - set).max(0.) / autos as f32 } else { 0. };
    let raw: Vec<f32> = sizes.iter().map(|s| if let Size::Share(v) = s { v.max(0.) } else { auto }).collect();
    let total: f32 = raw.iter().sum();
    if total > 0. { raw.iter().map(|f| f / total).collect() } else { vec![1. / sizes.len().max(1) as f32; sizes.len()] }
}

/// Lengths of parts asking for `fractions` of `content`, each at least its minimum when all
/// minimums fit; when they do not, the plain fractions (panes are clipped then).
fn distribute(content: f32, fractions: &[f32], mins: &[f32]) -> Vec<f32> {
    if mins.iter().sum::<f32>() > content {
        return fractions.iter().map(|f| f * content).collect();
    }
    let mut fixed = vec![false; fractions.len()];
    loop {
        let free_content = content - mins.iter().zip(&fixed).filter(|(_, f)| **f).map(|(m, _)| m).sum::<f32>();
        let free_fraction: f32 = fractions.iter().zip(&fixed).filter(|(_, f)| !**f).map(|(f, _)| f).sum();
        let free_count = fixed.iter().filter(|f| !**f).count().max(1) as f32;
        let share = |i: usize| {
            if free_fraction > 0. { fractions[i] / free_fraction * free_content } else { free_content / free_count }
        };
        let mut changed = false;
        for (i, is_fixed) in fixed.iter_mut().enumerate() {
            if !*is_fixed && share(i) < mins[i] {
                *is_fixed = true;
                changed = true;
            }
        }
        if !changed {
            return (0..fractions.len()).map(|i| if fixed[i] { mins[i] } else { share(i) }).collect();
        }
    }
}

/// One part of a group.
#[derive(Debug)]
struct Slot<T> {
    node: Node<T>,
    size: Size,
    /// The row this part is, for the parts of a stacked root group only.
    row: Option<RowId>,
}

impl<T> Slot<T> {
    fn new(node: Node<T>) -> Self {
        Self { node, size: Size::Auto, row: None }
    }
}

#[derive(Debug)]
struct Group<T> {
    id: GroupId,
    axis: Axis,
    /// Always two or more.
    slots: Vec<Slot<T>>,
}

impl<T> Group<T> {
    /// The rectangle of every part in `rect`: lengths from the parts' sizes and minimums,
    /// positions rounded as a running total so the parts and gaps always fill `rect` exactly.
    fn part_rects(&self, rect: Rect, params: &LayoutParams) -> (Vec<Rect>, f32) {
        let axis = self.axis;
        let gaps = self.slots.len().saturating_sub(1) as f32;
        let length = rect.extent(axis).max(0.);
        // Gaps never take more than the whole length, so no part gets a negative size.
        let gap = if gaps > 0. { params.gap.min(length / gaps).max(0.) } else { 0. };
        let content = (length - gap * gaps).max(0.);
        let sizes: Vec<Size> = self.slots.iter().map(|s| s.size).collect();
        let mins: Vec<f32> = self.slots.iter().map(|s| s.node.min_extent(axis, params)).collect();
        let lengths = distribute(content, &fractions(&sizes), &mins);
        let mut total: f32 = 0.;
        let mut rects = Vec::with_capacity(lengths.len());
        for (i, len) in lengths.iter().enumerate() {
            let from = total.round();
            total += len;
            let to = total.round().max(from);
            rects.push(rect.with_span(axis, rect.start(axis) + from + gap * i as f32, to - from));
        }
        (rects, gap)
    }
}

#[derive(Debug)]
enum Node<T> {
    Leaf { id: PaneId, item: T },
    Group(Group<T>),
}

impl<T> Node<T> {
    /// Stands in for a node while it is being rebuilt; never left in a tree.
    fn placeholder() -> Self {
        Self::Group(Group { id: GroupId(u64::MAX), axis: Axis::Horizontal, slots: Vec::new() })
    }

    fn first_leaf(&self) -> Option<PaneId> {
        match self {
            Self::Leaf { id, .. } => Some(*id),
            Self::Group(g) => g.slots.first()?.node.first_leaf(),
        }
    }

    fn leaves<'a>(&'a self, out: &mut Vec<(PaneId, &'a T)>) {
        match self {
            Self::Leaf { id, item } => out.push((*id, item)),
            Self::Group(g) => g.slots.iter().for_each(|s| s.node.leaves(out)),
        }
    }

    fn find_mut(&mut self, target: PaneId) -> Option<&mut T> {
        match self {
            Self::Leaf { id, item } => (*id == target).then_some(item),
            Self::Group(g) => g.slots.iter_mut().find_map(|s| s.node.find_mut(target)),
        }
    }

    /// The slot indices from this node down to the first node `is_target` accepts.
    fn path_to(&self, is_target: &impl Fn(&Self) -> bool, out: &mut Vec<usize>) -> bool {
        if is_target(self) {
            return true;
        }
        let Self::Group(g) = self else { return false };
        for (i, slot) in g.slots.iter().enumerate() {
            out.push(i);
            if slot.node.path_to(is_target, out) {
                return true;
            }
            out.pop();
        }
        false
    }

    fn is_pane(&self, target: PaneId) -> bool {
        matches!(self, Self::Leaf { id, .. } if *id == target)
    }

    fn is_group(&self, target: GroupId) -> bool {
        matches!(self, Self::Group(g) if g.id == target)
    }

    fn at(&self, path: &[usize]) -> Option<&Self> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                Self::Group(g) => g.slots.get(*i)?.node.at(rest),
                Self::Leaf { .. } => None,
            },
        }
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Self> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                Self::Group(g) => g.slots.get_mut(*i)?.node.at_mut(rest),
                Self::Leaf { .. } => None,
            },
        }
    }

    /// Smallest extent along `axis` this node can be squeezed to.
    fn min_extent(&self, axis: Axis, params: &LayoutParams) -> f32 {
        match self {
            Self::Leaf { id, .. } => params.min(*id, axis),
            Self::Group(g) => {
                let mins = g.slots.iter().map(|s| s.node.min_extent(axis, params));
                if g.axis == axis {
                    mins.sum::<f32>() + params.gap * g.slots.len().saturating_sub(1) as f32
                } else {
                    mins.fold(0., f32::max)
                }
            }
        }
    }

    fn place(&self, rect: Rect, params: &LayoutParams, out: &mut Layout) {
        match self {
            Self::Leaf { id, .. } => out.panes.push((*id, rect)),
            Self::Group(g) => {
                let parts = out.place_group(g, rect, params);
                for (slot, part) in g.slots.iter().zip(parts) {
                    slot.node.place(part, params, out);
                }
            }
        }
    }
}

/// The space between two parts of a group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Divider {
    /// Which divider it is.
    pub id: DividerId,
    /// The group's axis.
    pub axis: Axis,
    /// The gap itself.
    pub rect: Rect,
}

/// Width and scroll position of one row.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct RowState {
    /// Fixed width of the row's contents; `None` (or a width not above the visible width)
    /// follows the visible width.
    width: Option<f32>,
    /// How far the contents are scrolled to the right.
    scroll: f32,
}

/// One row of the tree as laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Which row it is.
    pub id: RowId,
    /// The visible part.
    pub rect: Rect,
    /// Width of the contents (at least the visible width).
    pub width: f32,
    /// Scroll position, kept within the contents.
    pub scroll: f32,
    /// The panes in it.
    pub panes: Vec<PaneId>,
}

/// Everything one layout pass places, for drawing, hit-testing, focus moves and scroll bars
/// alike. Pane and divider rectangles are in the coordinates of the laid-out bounds, with every
/// row shifted by its scroll, so panes of a scrolled row can reach past the bounds.
#[derive(Debug, Default)]
pub struct Layout {
    panes: Vec<(PaneId, Rect)>,
    dividers: Vec<Divider>,
    rows: Vec<Row>,
    /// Where each group's parts went.
    groups: HashMap<GroupId, Vec<Rect>>,
}

impl Layout {
    /// Every pane and its rectangle, in tree order (left/top first).
    pub fn panes(&self) -> &[(PaneId, Rect)] {
        &self.panes
    }

    /// The rectangle of pane `id`.
    pub fn pane(&self, id: PaneId) -> Option<Rect> {
        self.panes.iter().find(|(p, _)| *p == id).map(|(_, r)| *r)
    }

    /// Every divider, outer groups first.
    pub fn dividers(&self) -> &[Divider] {
        &self.dividers
    }

    /// The rows, top to bottom.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Row `id`.
    pub fn row(&self, id: RowId) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// The row pane `id` is in.
    pub fn row_of(&self, id: PaneId) -> Option<&Row> {
        self.rows.iter().find(|r| r.panes.contains(&id))
    }

    /// Lays out the parts of `group` in `rect`, records its dividers, and returns the parts.
    fn place_group<T>(&mut self, group: &Group<T>, rect: Rect, params: &LayoutParams) -> Vec<Rect> {
        let axis = group.axis;
        let (parts, gap) = group.part_rects(rect, params);
        for (index, pair) in parts.windows(2).enumerate() {
            let end = pair[0].start(axis) + pair[0].extent(axis);
            let line = rect.with_span(axis, end, gap.min(pair[1].start(axis) - end).max(0.));
            self.dividers.push(Divider { id: DividerId { group: group.id, index }, axis, rect: line });
        }
        self.groups.insert(group.id, parts.clone());
        parts
    }
}

/// Result of [`PaneTree::close`].
#[derive(Debug, PartialEq, Eq)]
pub enum Closed<T> {
    /// The pane was removed and `focus` is focused now.
    Removed {
        /// The removed pane's item.
        item: T,
        /// The pane that has the focus now.
        focus: PaneId,
    },
    /// It is the only pane; nothing was removed (closing it closes the tab).
    LastPane,
    /// No pane has this id.
    NotFound,
}

/// The panes of one tab and which of them has the focus. Never empty.
///
/// When the root is a stacked group, each of its parts is a row; otherwise the whole tree is one
/// row. Each row can be given a fixed width wider than the space it is shown in; its contents
/// then scroll sideways. Groups nested deeper do not scroll on their own.
///
/// Sizes are even by default: the parts of a group share it equally, except parts the user sized
/// by dragging a divider, which keep their share of the group.
#[derive(Debug)]
pub struct PaneTree<T> {
    root: Node<T>,
    focused: PaneId,
    next_id: u64,
    /// The row of a root that is not a stacked group.
    root_row: RowId,
    rows: HashMap<RowId, RowState>,
    revision: u64,
}

impl<T> PaneTree<T> {
    /// A tree with a single pane, which has the focus.
    pub fn new(item: T) -> Self {
        let id = PaneId(0);
        Self {
            root: Node::Leaf { id, item },
            focused: id,
            next_id: 2,
            root_row: RowId(1),
            rows: HashMap::new(),
            revision: 0,
        }
    }

    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Changes whenever panes are added or removed. Divider ids and pane positions from before
    /// a change must not be used after it.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The focused pane.
    pub fn focused(&self) -> PaneId {
        self.focused
    }

    /// Focuses `id`. Returns `false` (and changes nothing) if there is no such pane.
    pub fn focus(&mut self, id: PaneId) -> bool {
        let exists = self.root.path_to(&|node| node.is_pane(id), &mut Vec::new());
        if exists {
            self.focused = id;
        }
        exists
    }

    /// Number of panes.
    pub fn len(&self) -> usize {
        self.panes().len()
    }

    /// Always `false`: a tree has at least one pane.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The panes in tree order (left/top first).
    pub fn panes(&self) -> Vec<(PaneId, &T)> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }

    /// The item of pane `id`.
    pub fn get(&self, id: PaneId) -> Option<&T> {
        self.panes().into_iter().find(|(p, _)| *p == id).map(|(_, item)| item)
    }

    /// The item of pane `id`, mutably.
    pub fn get_mut(&mut self, id: PaneId) -> Option<&mut T> {
        self.root.find_mut(id)
    }

    fn group_mut(&mut self, path: &[usize]) -> Option<&mut Group<T>> {
        match self.root.at_mut(path)? {
            Node::Group(g) => Some(g),
            Node::Leaf { .. } => None,
        }
    }

    fn is_stacked_root(&self) -> bool {
        matches!(&self.root, Node::Group(g) if g.axis == Axis::Vertical)
    }

    /// Gives every part of a stacked root a row and forgets the state of rows that are gone.
    fn sync_rows(&mut self) {
        let mut live = HashSet::new();
        if let Node::Group(g) = &mut self.root
            && g.axis == Axis::Vertical
        {
            for slot in &mut g.slots {
                let row = *slot.row.get_or_insert_with(|| {
                    let id = RowId(self.next_id);
                    self.next_id += 1;
                    id
                });
                live.insert(row);
            }
        } else {
            live.insert(self.root_row);
        }
        self.rows.retain(|id, _| live.contains(id));
    }

    /// Splits pane `target`, putting `item` on `side` of it, and focuses the new pane. Along its
    /// group the new pane becomes another part and both are sized evenly; across it the two make
    /// a new group in the pane's place, which keeps the pane's size. Returns the new pane's id,
    /// or gives `item` back if there is no such pane.
    pub fn split(&mut self, target: PaneId, side: Side, item: T) -> Result<PaneId, T> {
        let mut path = Vec::new();
        if !self.root.path_to(&|node| node.is_pane(target), &mut path) {
            return Err(item);
        }
        let (axis, is_new_first) = (side.axis(), side.is_new_first());
        let new_id = PaneId(self.fresh_id());
        let group_id = GroupId(self.fresh_id());
        let new_slot = |item| Slot::new(Node::Leaf { id: new_id, item });
        let pair = |old: Slot<T>, new: Slot<T>| if is_new_first { vec![new, old] } else { vec![old, new] };
        match path.split_last() {
            None => {
                let old = std::mem::replace(&mut self.root, Node::placeholder());
                // A pane that was the whole tab, stacked: it keeps its row.
                let row = (axis == Axis::Vertical).then_some(self.root_row);
                let slots = pair(Slot { row, ..Slot::new(old) }, new_slot(item));
                self.root = Node::Group(Group { id: group_id, axis, slots });
            }
            Some((&index, parent)) => {
                let Some(group) = self.group_mut(parent) else { return Err(item) };
                let Some(slot) = group.slots.get_mut(index) else { return Err(item) };
                if group.axis == axis {
                    slot.size = Size::Auto;
                    group.slots.insert(if is_new_first { index } else { index + 1 }, new_slot(item));
                } else {
                    let old = std::mem::replace(&mut slot.node, Node::placeholder());
                    slot.node = Node::Group(Group { id: group_id, axis, slots: pair(Slot::new(old), new_slot(item)) });
                }
            }
        }
        self.sync_rows();
        self.revision += 1;
        self.focused = new_id;
        Ok(new_id)
    }

    /// Removes pane `id`, as laid out in `bounds`. Its group's other parts take the freed space.
    /// In a row with a fixed width, a pane that is a side-by-side part of the row itself takes
    /// its width and one gap off the row, so the other panes keep their widths. A group left
    /// with one part is replaced by that part; if that part is a group of the outer group's axis,
    /// its parts join the outer group, keeping their size. If `id` had the focus, the focus moves
    /// to the part after it (right or below), or the part before it when it was the last; when
    /// that part is a group, to its first pane.
    pub fn close(&mut self, id: PaneId, bounds: Rect, params: &LayoutParams) -> Closed<T> {
        if let Node::Leaf { id: only, .. } = &self.root {
            return if *only == id { Closed::LastPane } else { Closed::NotFound };
        }
        let mut path = Vec::new();
        if !self.root.path_to(&|node| node.is_pane(id), &mut path) {
            return Closed::NotFound;
        }
        let Some((&index, parent_path)) = path.split_last() else { return Closed::NotFound };
        let Some(Node::Group(parent)) = self.root.at(parent_path) else { return Closed::NotFound };
        let sibling = parent.slots.get(index + 1).or_else(|| index.checked_sub(1).and_then(|i| parent.slots.get(i)));
        let Some(next_focus) = sibling.and_then(|s| s.node.first_leaf()) else { return Closed::NotFound };
        let layout = self.layout(bounds, params);
        self.shrink_row_for(id, parent_path, &layout, params);
        let Some(parent) = self.group_mut(parent_path) else { return Closed::NotFound };
        let removed = parent.slots.remove(index);
        if parent.slots.len() == 1 {
            self.collapse(parent_path, &layout, params);
        }
        self.sync_rows();
        self.revision += 1;
        if self.focused == id {
            self.focused = next_focus;
        }
        match removed.node {
            Node::Leaf { item, .. } => Closed::Removed { item, focus: self.focused },
            Node::Group(_) => unreachable!("the path ends at a pane"),
        }
    }

    /// Before pane `id` (a part of the group at `parent_path`) is removed: if that group is the
    /// side-by-side content of a row with a fixed width, the row loses the pane's width and a
    /// gap, and the hand-set shares of the group are rescaled so they keep their pixels.
    fn shrink_row_for(&mut self, id: PaneId, parent_path: &[usize], layout: &Layout, params: &LayoutParams) {
        let is_row_group = if self.is_stacked_root() { parent_path.len() == 1 } else { parent_path.is_empty() };
        if !is_row_group {
            return;
        }
        let row_id = match (&self.root, parent_path.first()) {
            (Node::Group(g), Some(i)) => g.slots.get(*i).and_then(|s| s.row),
            _ => Some(self.root_row),
        };
        let Some(row) = row_id.and_then(|r| layout.row(r)) else { return };
        let (row_id, visible) = (row.id, row.rect.width);
        let Some(width) = self.row_state(row_id).width.filter(|w| *w > visible) else { return };
        let Some(pane) = layout.pane(id) else { return };
        let Some(group) = self.group_mut(parent_path) else { return };
        if group.axis != Axis::Horizontal {
            return;
        }
        let Some(parts) = layout.groups.get(&group.id) else { return };
        let narrower = width - pane.width - params.gap;
        let new_width = (narrower > visible + 0.5).then_some(narrower);
        let old_content: f32 = parts.iter().map(|r| r.width).sum();
        let gaps = group.slots.len().saturating_sub(2) as f32;
        let new_content = (new_width.unwrap_or(visible) - params.gap * gaps).max(1.);
        for slot in &mut group.slots {
            if let Size::Share(share) = &mut slot.size {
                *share *= old_content / new_content;
            }
        }
        self.set_row_width(row_id, new_width);
    }

    /// Replaces the group at `path`, which has one part left, by that part. A group part of the
    /// outer group's axis joins its parts to the outer group (`layout` is from before the
    /// removal, for converting sizes so they keep their pixels).
    fn collapse(&mut self, path: &[usize], layout: &Layout, params: &LayoutParams) {
        let Some((&index, outer_path)) = path.split_last() else {
            match std::mem::replace(&mut self.root, Node::placeholder()) {
                Node::Group(mut root) if root.slots.len() == 1 => {
                    let only = root.slots.remove(0);
                    if root.axis == Axis::Vertical {
                        self.root_row = only.row.unwrap_or(self.root_row);
                    }
                    // A side-by-side root left with a stack: its parts become rows (sync_rows).
                    self.root = only.node;
                }
                other => self.root = other,
            }
            return;
        };
        let Some(outer) = self.group_mut(outer_path) else { return };
        let Some(old) = outer.slots.get_mut(index) else { return };
        let only = match &mut old.node {
            Node::Group(group) if group.slots.len() == 1 => group.slots.remove(0),
            _ => return,
        };
        let old = outer.slots.remove(index);
        match only.node {
            Node::Group(inner) if inner.axis == outer.axis => {
                let content: Option<f32> =
                    layout.groups.get(&outer.id).map(|parts| parts.iter().map(|r| r.extent(outer.axis)).sum());
                let mut sizes: Vec<Size> = outer.slots.iter().map(|s| s.size).collect();
                sizes.insert(index, old.size);
                let own = fractions(&sizes).get(index).copied().unwrap_or(0.);
                let joined = inner.slots.len();
                let added_gaps = params.gap * joined.saturating_sub(1) as f32;
                let keeps_auto = old.size == Size::Auto && inner.slots.iter().all(|s| s.size == Size::Auto);
                let parts: Vec<Slot<T>> = match content {
                    Some(content) if !keeps_auto && content > added_gaps => {
                        let new_content = content - added_gaps;
                        for slot in &mut outer.slots {
                            if let Size::Share(share) = &mut slot.size {
                                *share *= content / new_content;
                            }
                        }
                        // Pixels the joined group had, without its own gaps, shared out as its
                        // parts asked for.
                        let inner_content = (own * content - added_gaps).max(0.);
                        let inner_sizes: Vec<Size> = inner.slots.iter().map(|s| s.size).collect();
                        let inner_fractions = fractions(&inner_sizes);
                        inner
                            .slots
                            .into_iter()
                            .zip(inner_fractions)
                            .map(|(s, f)| Slot { size: Size::Share(f * inner_content / new_content), row: None, ..s })
                            .collect()
                    }
                    _ => inner.slots.into_iter().map(|s| Slot { size: Size::Auto, row: None, ..s }).collect(),
                };
                outer.slots.splice(index..index, parts);
            }
            node => outer.slots.insert(index, Slot { node, size: old.size, row: old.row }),
        }
    }

    /// Lays the tree out in `bounds`: every pane, divider and row.
    pub fn layout(&self, bounds: Rect, params: &LayoutParams) -> Layout {
        let mut out = Layout::default();
        match &self.root {
            Node::Group(g) if g.axis == Axis::Vertical => {
                let parts = out.place_group(g, bounds, params);
                for (slot, rect) in g.slots.iter().zip(parts) {
                    self.place_row(slot.row.unwrap_or(self.root_row), &slot.node, rect, params, &mut out);
                }
            }
            node => self.place_row(self.root_row, node, bounds, params, &mut out),
        }
        out
    }

    fn place_row(&self, id: RowId, node: &Node<T>, rect: Rect, params: &LayoutParams, out: &mut Layout) {
        let state = self.row_state(id);
        let width = state.width.filter(|w| *w > rect.width).unwrap_or(rect.width);
        // Not `clamp`: it panics on a NaN bound, and the bounds come from the UI.
        let scroll = state.scroll.min(width - rect.width).max(0.);
        let first = out.panes.len();
        node.place(Rect { x: rect.x - scroll, width, ..rect }, params, out);
        let panes = out.panes[first..].iter().map(|(p, _)| *p).collect();
        out.rows.push(Row { id, rect, width, scroll, panes });
    }

    /// The stored state of row `id` (default for a row that is gone).
    fn row_state(&self, id: RowId) -> RowState {
        self.rows.get(&id).copied().unwrap_or_default()
    }

    /// Sets row `id`'s fixed width; `None` lets it follow the visible width again (and scrolls
    /// it back to the start).
    pub fn set_row_width(&mut self, id: RowId, width: Option<f32>) {
        let state = self.rows.entry(id).or_default();
        state.width = width;
        if width.is_none() {
            state.scroll = 0.;
        }
        self.sync_rows();
    }

    /// Sets row `id`'s scroll position (kept within its contents when laid out).
    pub fn set_row_scroll(&mut self, id: RowId, scroll: f32) {
        self.rows.entry(id).or_default().scroll = scroll;
        self.sync_rows();
    }

    /// Lets every row of `layout` whose fixed width the visible width has caught up with follow
    /// the visible width again.
    pub fn settle_rows(&mut self, layout: &Layout) {
        for row in &layout.rows {
            if self.row_state(row.id).width.is_some_and(|w| w <= row.rect.width) {
                self.set_row_width(row.id, None);
            }
        }
    }

    /// Moves divider `id` so it starts at `position` (an x for side-by-side parts, a y for
    /// stacked ones). Only the two parts touching it change; they keep their new share of the
    /// group from then on, and each stays at least its minimum size. Returns `false` if there is
    /// no such divider.
    pub fn drag_divider(&mut self, id: DividerId, position: f32, bounds: Rect, params: &LayoutParams) -> bool {
        let layout = self.layout(bounds, params);
        let Some(parts) = layout.groups.get(&id.group) else { return false };
        let mut path = Vec::new();
        if !self.root.path_to(&|node| node.is_group(id.group), &mut path) {
            return false;
        }
        let Some(group) = self.group_mut(&path) else { return false };
        let axis = group.axis;
        let (Some(a), Some(b)) = (parts.get(id.index), parts.get(id.index + 1)) else { return false };
        let (Some(slot_a), Some(slot_b)) = (group.slots.get(id.index), group.slots.get(id.index + 1)) else {
            return false;
        };
        let (min_a, min_b) = (slot_a.node.min_extent(axis, params), slot_b.node.min_extent(axis, params));
        let content: f32 = parts.iter().map(|r| r.extent(axis)).sum();
        let pair = a.extent(axis) + b.extent(axis);
        if content <= 0. || pair <= 0. {
            return true;
        }
        let first = position - a.start(axis);
        let first = if min_a + min_b > pair { pair / 2. } else { first.max(min_a).min(pair - min_b) };
        // The pair's total is unchanged, so the other parts keep their size.
        group.slots[id.index].size = Size::Share(first / content);
        group.slots[id.index + 1].size = Size::Share((pair - first) / content);
        true
    }

    /// Gives every part of the group of divider `id` the same size (every pane side by side in
    /// that row, or stacked in that column), dropping the sizes set there by hand. Groups inside
    /// it keep theirs. Returns `false` if there is no such divider.
    pub fn equalize(&mut self, id: DividerId) -> bool {
        let mut path = Vec::new();
        if !self.root.path_to(&|node| node.is_group(id.group), &mut path) {
            return false;
        }
        let Some(group) = self.group_mut(&path) else { return false };
        for slot in &mut group.slots {
            slot.size = Size::Auto;
        }
        true
    }

    /// The nearest pane in `direction` from pane `from` in `layout`: among panes beyond that edge
    /// that overlap it on the other axis, the closest one, then the one with the longest overlap.
    /// With none on that side it wraps around to the far edge, as Ghostty's `nearestWrapped`
    /// does (the pane is looked for as if `from` sat one whole layout further back).
    pub fn neighbor(&self, from: PaneId, direction: Direction, layout: &Layout) -> Option<PaneId> {
        let panes = layout.panes();
        let here = layout.pane(from)?;
        Self::nearest(panes, from, here, direction).or_else(|| {
            // Scrolled rows reach past the bounds, so wrap by the extent of all panes.
            let extent = |start: fn(&Rect) -> f32, end: fn(&Rect) -> f32| {
                let lo = panes.iter().map(|(_, r)| start(r)).fold(f32::INFINITY, f32::min);
                let hi = panes.iter().map(|(_, r)| end(r)).fold(f32::NEG_INFINITY, f32::max);
                hi - lo
            };
            let (width, height) = (extent(|r| r.x, Rect::right), extent(|r| r.y, Rect::bottom));
            let mut shifted = here;
            match direction {
                Direction::Right => shifted.x -= width,
                Direction::Left => shifted.x += width,
                Direction::Down => shifted.y -= height,
                Direction::Up => shifted.y += height,
            }
            Self::nearest(panes, from, shifted, direction)
        })
    }

    fn nearest(panes: &[(PaneId, Rect)], from: PaneId, here: Rect, direction: Direction) -> Option<PaneId> {
        const EPSILON: f32 = 0.5;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.);
        panes
            .iter()
            .filter(|(id, _)| *id != from)
            .filter_map(|(id, r)| {
                let (distance, shared) = match direction {
                    Direction::Right => (r.x - here.right(), overlap(here.y, here.bottom(), r.y, r.bottom())),
                    Direction::Left => (here.x - r.right(), overlap(here.y, here.bottom(), r.y, r.bottom())),
                    Direction::Down => (r.y - here.bottom(), overlap(here.x, here.right(), r.x, r.right())),
                    Direction::Up => (here.y - r.bottom(), overlap(here.x, here.right(), r.x, r.right())),
                };
                (distance >= -EPSILON && shared > EPSILON).then_some((*id, distance, shared))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|(id, _, _)| id)
    }

    /// The pane after `from` in tree order, wrapping around.
    pub fn next(&self, from: PaneId) -> PaneId {
        self.step(from, true)
    }

    /// The pane before `from` in tree order, wrapping around.
    pub fn previous(&self, from: PaneId) -> PaneId {
        self.step(from, false)
    }

    fn step(&self, from: PaneId, is_forward: bool) -> PaneId {
        let ids: Vec<PaneId> = self.panes().into_iter().map(|(id, _)| id).collect();
        let Some(index) = ids.iter().position(|id| *id == from) else { return from };
        let next = if is_forward { (index + 1) % ids.len() } else { (index + ids.len() - 1) % ids.len() };
        ids[next]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rect = Rect::new(0., 0., 801., 601.);

    fn params() -> LayoutParams {
        LayoutParams { gap: 1., min_sizes: HashMap::new(), default_min: (50., 30.) }
    }

    fn rect_in(tree: &PaneTree<&str>, bounds: Rect, id: PaneId) -> Rect {
        tree.layout(bounds, &params()).pane(id).expect("pane")
    }

    fn rect_of(tree: &PaneTree<&str>, id: PaneId) -> Rect {
        rect_in(tree, BOUNDS, id)
    }

    fn close(tree: &mut PaneTree<&'static str>, id: PaneId) -> Closed<&'static str> {
        tree.close(id, BOUNDS, &params())
    }

    /// The divider whose line contains point (x, y).
    fn divider_at(tree: &PaneTree<&str>, bounds: Rect, x: f32, y: f32) -> DividerId {
        let layout = tree.layout(bounds, &params());
        let hit = layout.dividers().iter().find(|d| {
            let r = d.rect;
            r.x - 1. <= x && x <= r.right() + 1. && r.y - 1. <= y && y <= r.bottom() + 1.
        });
        hit.expect("divider there").id
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1.
    }

    /// a | b | c | ... made by splitting the last pane to the right each time.
    fn row_of(names: &[&'static str]) -> (PaneTree<&'static str>, Vec<PaneId>) {
        let mut tree = PaneTree::new(names[0]);
        let mut ids = vec![tree.focused()];
        for name in &names[1..] {
            let last = *ids.last().expect("one pane");
            ids.push(tree.split(last, Side::Right, *name).expect("split"));
        }
        (tree, ids)
    }

    #[test]
    fn split_right_and_bottom_place_and_focus_the_new_pane() {
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        assert_eq!(tree.focused(), b);
        assert_eq!(rect_of(&tree, a), Rect::new(0., 0., 400., 601.));
        assert_eq!(rect_of(&tree, b), Rect::new(401., 0., 400., 601.));
        let c = tree.split(b, Side::Bottom, "c").expect("split");
        assert_eq!(rect_of(&tree, b), Rect::new(401., 0., 400., 300.));
        assert_eq!(rect_of(&tree, c), Rect::new(401., 301., 400., 300.));
        let names: Vec<&str> = tree.panes().into_iter().map(|(_, n)| *n).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn split_left_puts_the_new_pane_first() {
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Left, "b").expect("split");
        assert_eq!(rect_of(&tree, b).x, 0.);
        assert_eq!(rect_of(&tree, a).x, 401.);
    }

    #[test]
    fn splitting_along_a_row_adds_an_even_part() {
        // a | b, then b split right: three even parts, not a | (b | c) halves.
        let (tree, ids) = row_of(&["a", "b", "c"]);
        let widths: Vec<f32> = ids.iter().map(|&id| rect_of(&tree, id).width).collect();
        assert!(widths.iter().all(|w| near(*w, 266.)), "{widths:?}");
        let sum: f32 = widths.iter().sum();
        assert_eq!(sum + 2., 801., "parts and gaps fill the row exactly");
    }

    #[test]
    fn closing_collapses_without_gaps_and_moves_the_focus_to_the_next_part() {
        // a | (b / (c | d)); close b -> c | d takes its place and joins the row, c (the first
        // pane of the part after b) gets the focus, and a, c, d share the row evenly.
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        let c = tree.split(b, Side::Bottom, "c").expect("split");
        let d = tree.split(c, Side::Right, "d").expect("split");
        tree.focus(b);
        assert_eq!(close(&mut tree, b), Closed::Removed { item: "b", focus: c });
        let right = rect_of(&tree, c);
        assert_eq!((right.y, right.height), (0., 601.));
        for id in [a, c, d] {
            assert!(near(rect_of(&tree, id).width, 266.), "{:?}", rect_of(&tree, id));
        }
        // Closing a pane that is not focused keeps the focus.
        assert_eq!(close(&mut tree, d), Closed::Removed { item: "d", focus: c });
        assert_eq!(rect_of(&tree, c).width, 400.);
    }

    #[test]
    fn closing_the_last_part_focuses_the_one_before() {
        // a | b | c with c focused: closing c focuses b, as splitting right again and again and
        // closing the newest pane walks back.
        let (mut tree, ids) = row_of(&["a", "b", "c"]);
        assert_eq!(close(&mut tree, ids[2]), Closed::Removed { item: "c", focus: ids[1] });
        // a | b with a focused: closing a focuses b (the part after it).
        tree.focus(ids[0]);
        assert_eq!(close(&mut tree, ids[0]), Closed::Removed { item: "a", focus: ids[1] });
    }

    #[test]
    fn the_last_pane_cannot_be_removed() {
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        assert_eq!(close(&mut tree, a), Closed::LastPane);
        assert_eq!(tree.len(), 1);
        let b = tree.split(a, Side::Right, "b").expect("split");
        assert_eq!(close(&mut tree, a), Closed::Removed { item: "a", focus: b });
        assert_eq!(close(&mut tree, b), Closed::LastPane);
    }

    #[test]
    fn unknown_ids_change_nothing() {
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        close(&mut tree, b);
        let revision = tree.revision();
        assert_eq!(close(&mut tree, b), Closed::NotFound);
        assert!(!tree.focus(b));
        assert_eq!(tree.split(b, Side::Right, "x"), Err("x"));
        assert_eq!(tree.revision(), revision);
    }

    #[test]
    fn neighbors_in_a_grid() {
        // a b
        // c d
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        let c = tree.split(a, Side::Bottom, "c").expect("split");
        let d = tree.split(b, Side::Bottom, "d").expect("split");
        let layout = tree.layout(BOUNDS, &params());
        let n = |from, dir| tree.neighbor(from, dir, &layout);
        assert_eq!(n(a, Direction::Right), Some(b));
        assert_eq!(n(a, Direction::Down), Some(c));
        assert_eq!(n(d, Direction::Left), Some(c));
        assert_eq!(n(d, Direction::Up), Some(b));
        // Past the edge it wraps around.
        assert_eq!(n(a, Direction::Left), Some(b));
        assert_eq!(n(a, Direction::Up), Some(c));
        assert_eq!(n(d, Direction::Right), Some(c));
    }

    #[test]
    fn a_single_pane_has_no_neighbor() {
        let tree = PaneTree::new("a");
        let layout = tree.layout(BOUNDS, &params());
        assert_eq!(tree.neighbor(tree.focused(), Direction::Right, &layout), None);
    }

    #[test]
    fn neighbor_prefers_the_longest_overlap() {
        // a | (b / c) with the divider low: from a, Right picks b (overlaps a's height most).
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        tree.split(b, Side::Bottom, "c").expect("split");
        let stacked = divider_at(&tree, BOUNDS, 600., 300.);
        assert!(tree.drag_divider(stacked, 450., BOUNDS, &params()));
        let layout = tree.layout(BOUNDS, &params());
        assert_eq!(tree.neighbor(a, Direction::Right, &layout), Some(b));
    }

    #[test]
    fn next_and_previous_wrap_in_tree_order() {
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        let c = tree.split(b, Side::Bottom, "c").expect("split");
        assert_eq!(tree.next(a), b);
        assert_eq!(tree.next(c), a);
        assert_eq!(tree.previous(a), c);
    }

    #[test]
    fn dragging_a_divider_respects_minimum_sizes() {
        let (mut tree, ids) = row_of(&["a", "b"]);
        let divider = divider_at(&tree, BOUNDS, 400., 10.);
        tree.drag_divider(divider, 200., BOUNDS, &params());
        assert_eq!(rect_of(&tree, ids[0]).width, 200.);
        tree.drag_divider(divider, 5., BOUNDS, &params());
        assert_eq!(rect_of(&tree, ids[0]).width, 50.);
        tree.drag_divider(divider, 10_000., BOUNDS, &params());
        assert_eq!(rect_of(&tree, ids[1]).width, 50.);
    }

    #[test]
    fn nan_bounds_and_positions_do_not_panic() {
        let (mut tree, ids) = row_of(&["a", "b"]);
        let divider = divider_at(&tree, BOUNDS, 400., 10.);
        let nan = Rect::new(0., 0., f32::NAN, f32::NAN);
        tree.set_row_scroll(tree.layout(BOUNDS, &params()).rows[0].id, 30.);
        let _ = tree.layout(nan, &params());
        tree.drag_divider(divider, f32::NAN, BOUNDS, &params());
        tree.drag_divider(divider, 200., nan, &params());
        assert!(rect_of(&tree, ids[0]).width.is_finite());
    }

    #[test]
    fn layout_keeps_minimum_widths_when_a_share_would_squeeze_a_part() {
        // a | b sized by hand in a wide area (b gets 100 of 2000), then laid out in 801 with a
        // minimum of 250: b would get 40. The share itself is kept: back in the wide area b is
        // 100 again.
        let (mut tree, ids) = row_of(&["a", "b"]);
        let wide = Rect::new(0., 0., 2001., 601.);
        let divider = divider_at(&tree, wide, 1000., 10.);
        tree.drag_divider(divider, 1900., wide, &params());
        let big = LayoutParams { default_min: (250., 30.), ..params() };
        let layout = tree.layout(BOUNDS, &big);
        let width = |id| layout.pane(id).map(|r| r.width);
        assert_eq!((width(ids[0]), width(ids[1])), (Some(550.), Some(250.)));
        assert_eq!(rect_in(&tree, wide, ids[1]).width, 100.);
    }

    #[test]
    fn dragging_changes_only_the_two_neighbors_and_they_keep_that() {
        // a | b | c | d evenly; drag the divider between b and c.
        let (mut tree, ids) = row_of(&["a", "b", "c", "d"]);
        let [a, b, c, d] = ids[..] else { panic!("four panes") };
        let w = |tree: &PaneTree<&str>, id| rect_of(tree, id).width;
        let (wa, wd) = (w(&tree, a), w(&tree, d));
        let between = divider_at(&tree, BOUNDS, rect_of(&tree, c).x - 0.5, 10.);
        let pair = w(&tree, b) + w(&tree, c);
        tree.drag_divider(between, rect_of(&tree, b).x + 100., BOUNDS, &params());
        assert_eq!((w(&tree, a), w(&tree, b), w(&tree, d)), (wa, 100., wd));
        assert_eq!(w(&tree, c), pair - 100.);
        // Splitting a, the even parts share the rest; b and c keep their share.
        let (wb, wc) = (w(&tree, b), w(&tree, c));
        let e = tree.split(a, Side::Right, "e").expect("split");
        assert!(near(w(&tree, b), wb) && near(w(&tree, c), wc));
        assert!(near(w(&tree, a), w(&tree, e)) && near(w(&tree, a), w(&tree, d)));
        // A double-click on any divider of the row evens out all of them again.
        let any = divider_at(&tree, BOUNDS, rect_of(&tree, e).x - 0.5, 10.);
        assert!(tree.equalize(any));
        assert!(near(w(&tree, b), w(&tree, e)) && near(w(&tree, b), w(&tree, d)));
    }

    #[test]
    fn dragging_in_a_column_and_inside_a_nested_group() {
        // a / b / c stacked: dragging a|b leaves c alone.
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Bottom, "b").expect("split");
        let c = tree.split(b, Side::Bottom, "c").expect("split");
        let hc = rect_of(&tree, c).height;
        let top = divider_at(&tree, BOUNDS, 10., rect_of(&tree, b).y - 0.5);
        tree.drag_divider(top, 100., BOUNDS, &params());
        assert_eq!(rect_of(&tree, a).height, 100.);
        assert_eq!(rect_of(&tree, c).height, hc);
        // x | (y / z / w): dragging y|z leaves w and x alone.
        let mut tree = PaneTree::new("x");
        let x = tree.focused();
        let y = tree.split(x, Side::Right, "y").expect("split");
        let z = tree.split(y, Side::Bottom, "z").expect("split");
        let w = tree.split(z, Side::Bottom, "w").expect("split");
        let (hw, wx) = (rect_of(&tree, w).height, rect_of(&tree, x).width);
        let between = divider_at(&tree, BOUNDS, 600., rect_of(&tree, z).y - 0.5);
        tree.drag_divider(between, 50., BOUNDS, &params());
        assert_eq!(rect_of(&tree, y).height, 50.);
        assert_eq!((rect_of(&tree, w).height, rect_of(&tree, x).width), (hw, wx));
    }

    #[test]
    fn equalize_touches_only_its_own_group() {
        // a | (b / c): size b/c by hand, then a|b by hand; equalizing the row keeps b/c.
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        tree.split(b, Side::Bottom, "c").expect("split");
        let stacked = divider_at(&tree, BOUNDS, 600., 300.);
        tree.drag_divider(stacked, 100., BOUNDS, &params());
        let row = divider_at(&tree, BOUNDS, 400., 10.);
        tree.drag_divider(row, 200., BOUNDS, &params());
        assert!(tree.equalize(row));
        assert_eq!(rect_of(&tree, a).width, 400.);
        assert_eq!(rect_of(&tree, b).height, 100.);
    }

    #[test]
    fn hand_set_shares_scale_with_the_area() {
        let (mut tree, ids) = row_of(&["a", "b", "c"]);
        let divider = divider_at(&tree, BOUNDS, rect_of(&tree, ids[1]).x - 0.5, 10.);
        tree.drag_divider(divider, 100., BOUNDS, &params());
        let wide = Rect::new(0., 0., 1603., 601.);
        let share = |bounds: Rect, id| rect_in(&tree, bounds, id).width / (bounds.width - 2.);
        for id in &ids {
            assert!((share(BOUNDS, *id) - share(wide, *id)).abs() < 0.01);
        }
    }

    #[test]
    fn splitting_a_hand_sized_pane_across_keeps_its_size_along_drops_it() {
        let (mut tree, ids) = row_of(&["a", "b", "c"]);
        let divider = divider_at(&tree, BOUNDS, rect_of(&tree, ids[1]).x - 0.5, 10.);
        tree.drag_divider(divider, 100., BOUNDS, &params());
        // Across: a's 100 px go to the new stack.
        let d = tree.split(ids[0], Side::Bottom, "d").expect("split");
        assert_eq!((rect_of(&tree, ids[0]).width, rect_of(&tree, d).width), (100., 100.));
        // Along: b (sized by hand too) and the new pane become even parts like c.
        let e = tree.split(ids[1], Side::Right, "e").expect("split");
        let (wb, we, wc) = (rect_of(&tree, ids[1]).width, rect_of(&tree, e).width, rect_of(&tree, ids[2]).width);
        assert!(near(wb, we) && near(we, wc), "{wb} {we} {wc}");
        assert_eq!(rect_of(&tree, ids[0]).width, 100.);
    }

    #[test]
    fn hand_set_shares_over_the_whole_group_are_scaled_to_fit() {
        assert_eq!(fractions(&[Size::Share(0.8), Size::Share(0.8)]), [0.5, 0.5]);
        assert_eq!(fractions(&[Size::Share(0.75), Size::Share(0.75), Size::Auto]), [0.5, 0.5, 0.]);
        assert_eq!(fractions(&[Size::Share(0.25), Size::Auto, Size::Auto]), [0.25, 0.375, 0.375]);
        assert_eq!(fractions(&[Size::Share(0.), Size::Share(0.)]), [0.5, 0.5]);
        // The automatic part with nothing left still gets its minimum.
        assert_eq!(distribute(100., &[0.5, 0.5, 0.], &[10., 10., 10.]), [45., 45., 10.]);
        // Minimums that do not fit: plain fractions.
        assert_eq!(distribute(10., &[0.5, 0.5], &[10., 10.]), [5., 5.]);
    }

    #[test]
    fn closing_a_part_of_a_wide_row_takes_its_width_off_the_row() {
        // Four even panes in a row 4 x 200 + 3 gaps wide shown in 602 (three fit): closing one
        // leaves exactly the visible width, so the row follows it again.
        let (mut tree, ids) = row_of(&["a", "b", "c", "d"]);
        let shown = Rect::new(0., 0., 602., 601.);
        let row = tree.layout(shown, &params()).rows()[0].id;
        tree.set_row_width(row, Some(803.));
        assert_eq!(rect_in(&tree, shown, ids[1]).width, 200.);
        tree.close(ids[1], shown, &params());
        assert_eq!(tree.row_state(row).width, None);
        let layout = tree.layout(shown, &params());
        assert!(layout.panes().iter().all(|(_, r)| near(r.width, 200.)), "{:?}", layout.panes());
    }

    #[test]
    fn closing_in_a_wide_row_keeps_hand_set_pixels() {
        // a | b | c | d in 1603 shown in 801; b sized to 500 by hand; closing d keeps b at 500.
        let (mut tree, ids) = row_of(&["a", "b", "c", "d"]);
        let row = tree.layout(BOUNDS, &params()).rows()[0].id;
        tree.set_row_width(row, Some(1603.));
        let divider = divider_at(&tree, BOUNDS, rect_of(&tree, ids[2]).x - 0.5, 10.);
        tree.drag_divider(divider, rect_of(&tree, ids[1]).x + 500., BOUNDS, &params());
        let wd = rect_of(&tree, ids[3]).width;
        tree.close(ids[3], BOUNDS, &params());
        assert_eq!(tree.row_state(row).width, Some(1603. - wd - 1.));
        assert!(near(rect_of(&tree, ids[1]).width, 500.));
    }

    #[test]
    fn closing_inside_a_stack_of_a_wide_row_keeps_the_row_width() {
        // a | (b / c) in a row 1601 wide: closing c leaves the row as wide.
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        let c = tree.split(b, Side::Bottom, "c").expect("split");
        let row = tree.layout(BOUNDS, &params()).rows()[0].id;
        tree.set_row_width(row, Some(1601.));
        tree.close(c, BOUNDS, &params());
        assert_eq!(tree.row_state(row).width, Some(1601.));
    }

    #[test]
    fn joining_parts_keep_their_pixels_with_gaps_and_hand_set_sizes() {
        // a | (b / (c | d)) with a gap of 9, a at 300 by hand: closing b joins c and d to the
        // row; a keeps its 300 and c, d keep the width they had.
        let p = LayoutParams { gap: 9., ..params() };
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        let c = tree.split(b, Side::Bottom, "c").expect("split");
        let d = tree.split(c, Side::Right, "d").expect("split");
        let layout = tree.layout(BOUNDS, &p);
        let row = layout.dividers().iter().find(|div| div.axis == Axis::Horizontal && div.rect.height > 600.);
        tree.drag_divider(row.expect("row divider").id, 300., BOUNDS, &p);
        let before = tree.layout(BOUNDS, &p);
        let width = |l: &Layout, id| l.pane(id).expect("pane").width;
        tree.close(b, BOUNDS, &p);
        let after = tree.layout(BOUNDS, &p);
        assert_eq!(width(&after, a), 300.);
        assert!(near(width(&after, c), width(&before, c)) && near(width(&after, d), width(&before, d)));
        let total: f32 = [a, c, d].iter().map(|id| width(&after, *id)).sum();
        assert_eq!(total + 18., 801.);
    }

    #[test]
    fn even_parts_joining_an_even_row_stay_even() {
        // a | (b / (c | d)) all even: closing b gives a | c | d, even.
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        let c = tree.split(b, Side::Bottom, "c").expect("split");
        let d = tree.split(c, Side::Right, "d").expect("split");
        close(&mut tree, b);
        // Narrow it: three even parts, not a share frozen at the old pixels.
        let wide = Rect::new(0., 0., 1202., 601.);
        for id in [a, c, d] {
            assert_eq!(rect_in(&tree, wide, id).width, 400.);
        }
    }

    #[test]
    fn rows_are_the_parts_of_the_outer_stack_only() {
        // a / b / c: three rows.
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Bottom, "b").expect("split");
        tree.split(b, Side::Bottom, "c").expect("split");
        assert_eq!(tree.layout(BOUNDS, &params()).rows().len(), 3);
        // x | (y / (z | w)): one row; the stack inside does not make rows of its own.
        let mut tree = PaneTree::new("x");
        let x = tree.focused();
        let y = tree.split(x, Side::Right, "y").expect("split");
        let z = tree.split(y, Side::Bottom, "z").expect("split");
        let w = tree.split(z, Side::Right, "w").expect("split");
        let layout = tree.layout(BOUNDS, &params());
        assert_eq!(layout.rows().len(), 1);
        assert_eq!(layout.rows()[0].rect, BOUNDS);
        assert_eq!(layout.row_of(w).map(|r| r.id), Some(layout.rows()[0].id));
    }

    #[test]
    fn a_wide_row_lays_out_its_panes_wider_and_scrolled() {
        // a / (b | c): the bottom row gets twice the width and is scrolled by 300.
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Bottom, "b").expect("split");
        let c = tree.split(b, Side::Right, "c").expect("split");
        let bottom = tree.layout(BOUNDS, &params()).row_of(b).expect("row").id;
        tree.set_row_width(bottom, Some(1602.));
        tree.set_row_scroll(bottom, 300.);
        assert_eq!(rect_of(&tree, a), Rect::new(0., 0., 801., 300.));
        assert_eq!(rect_of(&tree, b), Rect::new(-300., 301., 801., 300.));
        assert_eq!(rect_of(&tree, c), Rect::new(502., 301., 800., 300.));
        // The scroll stays within the contents; a width not above the visible one follows it.
        tree.set_row_scroll(bottom, 5000.);
        assert_eq!(tree.layout(BOUNDS, &params()).row_of(c).map(|r| r.scroll), Some(801.));
        tree.set_row_width(bottom, Some(700.));
        assert_eq!(rect_of(&tree, c).right(), 801.);
    }

    #[test]
    fn a_row_keeps_its_identity_and_width_through_splits_and_closes() {
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Bottom, "b").expect("split");
        let row = tree.layout(BOUNDS, &params()).row_of(b).expect("row").id;
        tree.set_row_width(row, Some(3000.));
        let c = tree.split(b, Side::Right, "c").expect("split");
        let layout = tree.layout(BOUNDS, &params());
        assert_eq!(layout.row_of(c).map(|r| (r.id, r.width)), Some((row, 3000.)));
        let wc = layout.pane(c).expect("pane").width;
        tree.close(c, BOUNDS, &params());
        let layout = tree.layout(BOUNDS, &params());
        assert_eq!(layout.row_of(b).map(|r| (r.id, r.width)), Some((row, 3000. - wc - 1.)));
        assert_eq!(layout.row_of(a).map(|r| r.width), Some(801.));
        // Closing the top row leaves b's row as the whole tab, same id and width.
        tree.close(a, BOUNDS, &params());
        let layout = tree.layout(BOUNDS, &params());
        assert_eq!(layout.rows().len(), 1);
        assert_eq!(layout.rows()[0].id, row);
        assert_eq!(layout.rows()[0].width, 3000. - wc - 1.);
    }

    #[test]
    fn rows_keep_their_own_width_and_scroll_through_edits_elsewhere() {
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Bottom, "b").expect("split");
        let layout = tree.layout(BOUNDS, &params());
        let (top, bottom) = (layout.row_of(a).expect("row").id, layout.row_of(b).expect("row").id);
        tree.set_row_width(top, Some(2000.));
        tree.set_row_scroll(top, 200.);
        let c = tree.split(b, Side::Right, "c").expect("split");
        tree.split(c, Side::Bottom, "d").expect("split");
        tree.close(c, BOUNDS, &params());
        assert_eq!(tree.row_state(top), RowState { width: Some(2000.), scroll: 200. });
        assert_eq!(tree.row_state(bottom), RowState::default());
    }

    #[test]
    fn a_side_by_side_root_left_with_a_stack_makes_new_rows() {
        // a | (b / c) is one row; closing a leaves b / c: two rows with new ids, and the old
        // row's state is gone.
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        tree.split(b, Side::Bottom, "c").expect("split");
        let old = tree.layout(BOUNDS, &params()).rows()[0].id;
        tree.set_row_width(old, Some(2000.));
        tree.close(a, BOUNDS, &params());
        let layout = tree.layout(BOUNDS, &params());
        assert_eq!(layout.rows().len(), 2);
        assert!(layout.rows().iter().all(|r| r.id != old && r.width == 801.));
        assert_eq!(tree.row_state(old), RowState::default());
    }

    #[test]
    fn settle_rows_releases_widths_the_area_caught_up_with() {
        let (mut tree, _) = row_of(&["a", "b"]);
        let row = tree.layout(BOUNDS, &params()).rows()[0].id;
        tree.set_row_width(row, Some(1000.));
        tree.set_row_scroll(row, 150.);
        let layout = tree.layout(BOUNDS, &params());
        tree.settle_rows(&layout);
        assert_eq!(tree.row_state(row).width, Some(1000.));
        let wide = Rect::new(0., 0., 1200., 601.);
        let layout = tree.layout(wide, &params());
        assert_eq!(layout.rows()[0].scroll, 0.);
        tree.settle_rows(&layout);
        assert_eq!(tree.row_state(row), RowState::default());
    }

    #[test]
    fn divider_ids_are_tied_to_the_revision() {
        let (mut tree, ids) = row_of(&["a", "b"]);
        let revision = tree.revision();
        let divider = divider_at(&tree, BOUNDS, 400., 10.);
        tree.drag_divider(divider, 300., BOUNDS, &params());
        assert_eq!(tree.revision(), revision, "a drag does not change the structure");
        tree.split(ids[0], Side::Bottom, "c").expect("split");
        assert_ne!(tree.revision(), revision);
        assert!(!tree.drag_divider(DividerId { group: divider.group, index: 5 }, 1., BOUNDS, &params()));
    }

    #[test]
    fn each_pane_has_its_own_minimum_and_focus_does_not_change_the_layout() {
        // a | b with b zoomed in (minimum 300 wide): dragging toward b stops at 300; a keeps
        // the small minimum.
        let (mut tree, ids) = row_of(&["a", "b"]);
        let mut p = params();
        p.min_sizes.insert(ids[1], (300., 30.));
        let divider = divider_at(&tree, BOUNDS, 400., 10.);
        tree.drag_divider(divider, 10_000., BOUNDS, &p);
        assert_eq!(tree.layout(BOUNDS, &p).pane(ids[1]).map(|r| r.width), Some(300.));
        tree.drag_divider(divider, 0., BOUNDS, &p);
        assert_eq!(tree.layout(BOUNDS, &p).pane(ids[0]).map(|r| r.width), Some(50.));
        let before: Vec<_> = tree.layout(BOUNDS, &p).panes().to_vec();
        tree.focus(ids[0]);
        assert_eq!(tree.layout(BOUNDS, &p).panes(), before);
    }

    #[test]
    fn tiny_areas_never_give_negative_sizes() {
        let mut tree = PaneTree::new("a");
        let mut last = tree.focused();
        for (i, name) in ["b", "c", "d", "e", "f"].into_iter().enumerate() {
            let side = if i % 2 == 0 { Side::Right } else { Side::Bottom };
            last = tree.split(last, side, name).expect("split");
        }
        let p = LayoutParams { gap: 8., ..params() };
        for bounds in [Rect::new(0., 0., 5., 3.), Rect::new(0., 0., 0., 0.), Rect::new(0., 0., 30., 900.)] {
            let layout = tree.layout(bounds, &p);
            for (_, r) in layout.panes() {
                assert!(r.width >= 0. && r.height >= 0. && r.x.is_finite() && r.y.is_finite(), "{r:?}");
            }
            for d in layout.dividers() {
                assert!(d.rect.width >= 0. && d.rect.height >= 0., "{d:?}");
            }
            let row = &layout.rows()[0];
            assert!(row.scroll >= 0. && row.width >= row.rect.width);
        }
    }

    #[test]
    fn neighbor_wraps_around_a_scrolled_row() {
        // a | b | c in a row three times as wide, scrolled to the middle: Right from c wraps to a.
        let (mut tree, ids) = row_of(&["a", "b", "c"]);
        let row = tree.layout(BOUNDS, &params()).rows()[0].id;
        tree.set_row_width(row, Some(2403.));
        tree.set_row_scroll(row, 801.);
        let layout = tree.layout(BOUNDS, &params());
        assert_eq!(tree.neighbor(ids[2], Direction::Right, &layout), Some(ids[0]));
        assert_eq!(tree.neighbor(ids[0], Direction::Left, &layout), Some(ids[2]));
    }

    #[test]
    fn a_side_split_again_keeps_room_for_all_its_panes() {
        // a | (b / c): dragging toward the right stops at the stack's minimum width.
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        let c = tree.split(b, Side::Bottom, "c").expect("split");
        let outer = divider_at(&tree, BOUNDS, 400., 10.);
        tree.drag_divider(outer, 10_000., BOUNDS, &params());
        assert_eq!(rect_of(&tree, a).width, 750.);
        assert_eq!((rect_of(&tree, b).width, rect_of(&tree, c).width), (50., 50.));
    }
}
