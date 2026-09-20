//! Session state: images, workspaces, filtering and ordering.
//! Pure data — no GTK, no filesystem access.

use std::path::PathBuf;

/// Index into `Session::images`. Stable for the lifetime of the session.
pub type ImageId = usize;

pub const WORKSPACES: u8 = 9;
/// View showing the images that are in no workspace yet.
pub const UNBINNED: u8 = WORKSPACES + 1;

#[derive(Debug, Clone)]
pub struct ImageEntry {
    pub path: PathBuf,
    pub workspace: Option<u8>,
}

/// Sort state of one view. View 0 is "all images", 1..=9 are the
/// workspaces, `UNBINNED` the rest.
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    pub manual_sort_enabled: bool,
    /// Remembered even while manual sorting is off, so toggling back restores it.
    pub explicit_order: Vec<ImageId>,
}

/// Everything undo brings back. File paths are not part of it: what
/// commands did on disk stays done.
#[derive(Debug, Clone)]
struct Snapshot {
    workspaces: Vec<Option<u8>>,
    views: Vec<Workspace>,
    active: u8,
    selected: Option<ImageId>,
}

const UNDO_DEPTH: usize = 200;

#[derive(Debug)]
pub struct Session {
    images: Vec<ImageEntry>,
    views: Vec<Workspace>,
    active: u8,
    selected: Option<ImageId>,
    /// Other end of a range selection that extends to `selected`.
    anchor: Option<ImageId>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl Session {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        let images = paths
            .into_iter()
            .map(|path| ImageEntry { path, workspace: None })
            .collect();
        Session {
            images,
            views: vec![Workspace::default(); UNBINNED as usize + 1],
            active: 0,
            selected: None,
            anchor: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            workspaces: self.images.iter().map(|e| e.workspace).collect(),
            views: self.views.clone(),
            active: self.active,
            selected: self.selected,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        for (entry, workspace) in self.images.iter_mut().zip(snapshot.workspaces) {
            entry.workspace = workspace;
        }
        self.views = snapshot.views;
        self.active = snapshot.active;
        self.selected = snapshot.selected;
        self.anchor = None;
    }

    /// Remember the state an organizing action is about to change.
    fn checkpoint(&mut self, before: Snapshot) {
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Take back the last assignment, reordering or sort toggle. Returns to
    /// the view and selection it happened in. Browsing is not undone.
    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo.pop() else { return false };
        self.redo.push(self.snapshot());
        self.restore(snapshot);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo.pop() else { return false };
        self.undo.push(self.snapshot());
        self.restore(snapshot);
        true
    }

    pub fn images(&self) -> &[ImageEntry] {
        &self.images
    }

    pub fn image(&self, id: ImageId) -> &ImageEntry {
        &self.images[id]
    }

    /// 0 = all images, 1..=9 = workspace filter, `UNBINNED`.
    pub fn active(&self) -> u8 {
        self.active
    }

    pub fn selected(&self) -> Option<ImageId> {
        self.selected
    }

    pub fn manual_sort(&self) -> bool {
        self.views[self.active as usize].manual_sort_enabled
    }

    pub fn workspace_count(&self, ws: u8) -> usize {
        self.images.iter().filter(|e| e.workspace == Some(ws)).count()
    }

    pub fn unbinned_count(&self) -> usize {
        self.images.iter().filter(|e| e.workspace.is_none()).count()
    }

    fn in_view(&self, view: u8, id: ImageId) -> bool {
        match view {
            0 => true,
            UNBINNED => self.images[id].workspace.is_none(),
            _ => self.images[id].workspace == Some(view),
        }
    }

    /// Image ids shown in the active view, in display order.
    pub fn visible(&self) -> Vec<ImageId> {
        self.view_order(self.active)
    }

    pub fn view_order(&self, view: u8) -> Vec<ImageId> {
        let natural = (0..self.images.len()).filter(|&id| self.in_view(view, id));
        let ws = &self.views[view as usize];
        if !ws.manual_sort_enabled {
            return natural.collect();
        }
        // Explicit order first; anything it doesn't know yet follows in natural order.
        let mut seen = vec![false; self.images.len()];
        let mut out = Vec::new();
        for &id in &ws.explicit_order {
            if self.in_view(view, id) && !seen[id] {
                seen[id] = true;
                out.push(id);
            }
        }
        out.extend(natural.filter(|&id| !seen[id]));
        out
    }

    pub fn select(&mut self, id: Option<ImageId>) {
        self.selected = id.filter(|&id| id < self.images.len());
    }

    /// Switch the filter. Keeps the selection if still visible, else selects the first image.
    pub fn set_active(&mut self, view: u8) {
        if view > UNBINNED {
            return;
        }
        self.active = view;
        self.anchor = None;
        let visible = self.visible();
        if !self.selected.is_some_and(|id| visible.contains(&id)) {
            self.selected = visible.first().copied();
        }
    }

    /// Start a range at the selected image, or drop the current one.
    pub fn toggle_range(&mut self) {
        self.anchor = if self.anchor.is_some() { None } else { self.selected };
    }

    pub fn clear_range(&mut self) {
        self.anchor = None;
    }

    pub fn has_range(&self) -> bool {
        self.anchor.is_some()
    }

    /// Select from `anchor` to `cursor`, e.g. after a shift-click.
    pub fn select_range(&mut self, anchor: ImageId, cursor: ImageId) {
        self.select(Some(cursor));
        self.anchor = Some(anchor).filter(|&a| a != cursor && a < self.images.len());
    }

    /// The images actions apply to, in display order: the range between
    /// anchor and selection, or just the selected image.
    pub fn marked(&self) -> Vec<ImageId> {
        let Some(selected) = self.selected else { return Vec::new() };
        let visible = self.visible();
        let position = |id| visible.iter().position(|&x| x == id);
        match (self.anchor.and_then(position), position(selected)) {
            (Some(a), Some(b)) => visible[a.min(b)..=a.max(b)].to_vec(),
            _ => vec![selected],
        }
    }

    /// Assign `id` to a workspace (exclusive) or clear it with `None`.
    /// If the image leaves the active view, a neighbouring image becomes selected.
    #[cfg(test)]
    pub fn assign(&mut self, id: ImageId, ws: Option<u8>) {
        self.assign_many(&[id], ws);
    }

    /// Assign everything marked, as a single undo step. Ends the range.
    pub fn assign_marked(&mut self, ws: Option<u8>) {
        let marked = self.marked();
        self.assign_many(&marked, ws);
        self.anchor = None;
    }

    fn assign_many(&mut self, ids: &[ImageId], ws: Option<u8>) {
        if ws.is_some_and(|w| w == 0 || w > WORKSPACES) {
            return;
        }
        let ids: Vec<ImageId> =
            ids.iter().copied().filter(|&id| id < self.images.len() && self.images[id].workspace != ws).collect();
        if ids.is_empty() {
            return;
        }
        self.checkpoint(self.snapshot());
        let before = self.visible();
        for &id in &ids {
            if let Some(old) = std::mem::replace(&mut self.images[id].workspace, ws) {
                self.views[old as usize].explicit_order.retain(|&x| x != id);
            }
            if let Some(new) = ws {
                let order = &mut self.views[new as usize].explicit_order;
                if !order.contains(&id) {
                    order.push(id);
                }
            }
        }
        if self.selected.is_some_and(|id| !self.in_view(self.active, id)) {
            // Whatever followed the departed images slides into their place.
            let after = self.visible();
            let first = before.iter().position(|id| ids.contains(id)).unwrap_or(0);
            self.selected = after.get(first).or(after.last()).copied();
        }
    }

    pub fn toggle_manual_sort(&mut self) -> bool {
        self.checkpoint(self.snapshot());
        let enabled = !self.manual_sort();
        if enabled {
            self.enable_manual_sort();
        } else {
            self.views[self.active as usize].manual_sort_enabled = false;
        }
        enabled
    }

    /// Turn manual sorting on. A remembered arrangement comes back; images it
    /// doesn't cover follow in natural order.
    fn enable_manual_sort(&mut self) {
        let view = self.active;
        if self.views[view as usize].manual_sort_enabled {
            return;
        }
        self.views[view as usize].manual_sort_enabled = true;
        let order = self.view_order(view);
        self.views[view as usize].explicit_order = order;
    }

    /// Move `id` to position `to` within the active view. Enables manual sorting.
    /// Returns false if nothing changed.
    pub fn move_to(&mut self, id: ImageId, to: usize) -> bool {
        let before = self.snapshot();
        self.enable_manual_sort();
        let mut order = self.visible();
        let from = order.iter().position(|&x| x == id);
        let to = to.min(order.len().saturating_sub(1));
        let Some(from) = from.filter(|&from| from != to) else {
            self.restore(before); // nothing moved: don't leave sorting switched on
            return false;
        };
        self.checkpoint(before);
        order.remove(from);
        order.insert(to, id);
        self.views[self.active as usize].explicit_order = order;
        true
    }

    /// Shift the selected image backward (-1) or forward (+1).
    pub fn move_selected(&mut self, delta: isize) -> bool {
        let Some(id) = self.selected else { return false };
        let order = self.visible();
        let Some(pos) = order.iter().position(|&x| x == id) else {
            return false;
        };
        let to = pos.saturating_add_signed(delta).min(order.len() - 1);
        to != pos && self.move_to(id, to)
    }

    /// Record that a file now lives elsewhere (after an explicit move/rename).
    pub fn set_path(&mut self, id: ImageId, path: PathBuf) {
        if let Some(e) = self.images.get_mut(id) {
            e.path = path;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(n: usize) -> Session {
        Session::new((0..n).map(|i| PathBuf::from(format!("/p/{i}.jpg"))).collect())
    }

    #[test]
    fn assignment_is_exclusive() {
        let mut s = session(3);
        s.assign(1, Some(1));
        s.assign(1, Some(2));
        assert_eq!(s.image(1).workspace, Some(2));
        assert_eq!(s.workspace_count(1), 0);
        assert_eq!(s.view_order(2), vec![1]);
        s.assign(1, None);
        assert_eq!(s.image(1).workspace, None);
        s.assign(1, Some(0));
        s.assign(1, Some(10));
        assert_eq!(s.image(1).workspace, None);
    }

    #[test]
    fn filter_shows_only_workspace() {
        let mut s = session(5);
        s.assign(4, Some(3));
        s.assign(1, Some(3));
        s.set_active(3);
        assert_eq!(s.visible(), vec![1, 4]); // natural order, not assignment order
        assert_eq!(s.selected(), Some(1));
        s.set_active(0);
        assert_eq!(s.visible().len(), 5);
        assert_eq!(s.selected(), Some(1)); // selection survives when still visible
    }

    #[test]
    fn reassigning_out_of_view_selects_neighbour() {
        let mut s = session(5);
        for id in [0, 2, 4] {
            s.assign(id, Some(1));
        }
        s.set_active(1);
        s.select(Some(2));
        s.assign(2, Some(2));
        assert_eq!(s.visible(), vec![0, 4]);
        assert_eq!(s.selected(), Some(4)); // next one slides into place
        s.assign(4, Some(2));
        assert_eq!(s.selected(), Some(0)); // was last: fall back to new last
        s.assign(0, None);
        assert_eq!(s.selected(), None);
        assert!(s.visible().is_empty());
    }

    #[test]
    fn assigning_in_all_view_keeps_selection() {
        let mut s = session(3);
        s.select(Some(1));
        s.assign(1, Some(5));
        assert_eq!(s.selected(), Some(1));
    }

    #[test]
    fn manual_order_is_per_view_and_remembered() {
        let mut s = session(4);
        for id in 0..4 {
            s.assign(id, Some(1));
        }
        s.set_active(1);
        s.select(Some(0));
        assert!(s.move_selected(1));
        assert!(s.manual_sort());
        assert_eq!(s.visible(), vec![1, 0, 2, 3]);
        assert!(s.move_to(3, 0));
        assert_eq!(s.visible(), vec![3, 1, 0, 2]);

        s.set_active(0);
        assert!(!s.manual_sort());
        assert_eq!(s.visible(), vec![0, 1, 2, 3]);

        s.set_active(1);
        assert!(!s.toggle_manual_sort());
        assert_eq!(s.visible(), vec![0, 1, 2, 3]);
        assert!(s.toggle_manual_sort()); // the arrangement was remembered
        assert_eq!(s.visible(), vec![3, 1, 0, 2]);
    }

    #[test]
    fn unbinned_view_is_a_shrinking_worklist() {
        let mut s = session(4);
        s.assign(1, Some(2));
        s.set_active(UNBINNED);
        assert_eq!(s.visible(), vec![0, 2, 3]);
        assert_eq!(s.unbinned_count(), 3);
        s.assign_marked(Some(1)); // image 0 goes, the next one is up
        assert_eq!(s.visible(), vec![2, 3]);
        assert_eq!(s.selected(), Some(2));
        s.set_active(2);
        s.assign_marked(None); // and back onto the pile
        assert_eq!(s.unbinned_count(), 3);
    }

    #[test]
    fn range_assignment() {
        let mut s = session(6);
        s.select(Some(4));
        assert_eq!(s.marked(), vec![4]);
        s.toggle_range();
        s.select(Some(2)); // ranges work backwards too
        assert_eq!(s.marked(), vec![2, 3, 4]);
        s.assign_marked(Some(1));
        assert!(!s.has_range());
        assert_eq!(s.view_order(1), vec![2, 3, 4]);

        s.set_active(1);
        s.select_range(2, 3);
        s.assign_marked(Some(2));
        assert_eq!(s.visible(), vec![4]);
        assert_eq!(s.selected(), Some(4));

        assert!(s.undo()); // the whole range is one step
        assert_eq!(s.visible(), vec![2, 3, 4]);
        s.toggle_range();
        s.set_active(0);
        assert!(!s.has_range());
    }

    #[test]
    fn undo_and_redo() {
        let mut s = session(4);
        s.select(Some(1));
        s.assign(1, Some(1));
        s.assign(2, Some(1));
        s.set_active(1);
        s.select(Some(2));
        s.assign(2, Some(3)); // leaves the view
        assert_eq!(s.visible(), vec![1]);

        assert!(s.undo());
        assert_eq!(s.visible(), vec![1, 2]);
        assert_eq!(s.selected(), Some(2));
        assert!(s.redo());
        assert_eq!(s.image(2).workspace, Some(3));
        assert!(s.undo());

        s.move_to(2, 0);
        assert_eq!(s.visible(), vec![2, 1]);
        s.set_active(0); // browsing is not an undo step
        assert!(s.undo());
        assert_eq!(s.active(), 1);
        assert!(!s.manual_sort());
        assert_eq!(s.visible(), vec![1, 2]);

        assert!(s.undo() && s.undo());
        assert!(!s.undo());
        assert_eq!(s.workspace_count(1), 0);
        s.assign(0, Some(9)); // a new action drops the redo history
        assert!(!s.redo());
    }

    #[test]
    fn a_move_that_moves_nothing_changes_nothing() {
        let mut s = session(3);
        assert!(!s.move_to(0, 0));
        assert!(!s.manual_sort());
        assert!(!s.undo());
    }

    #[test]
    fn moves_clamp_at_edges() {
        let mut s = session(3);
        s.select(Some(0));
        assert!(!s.move_selected(-1));
        s.select(Some(2));
        assert!(!s.move_selected(1));
        assert!(s.move_to(0, 99));
        assert_eq!(s.visible(), vec![1, 2, 0]);
    }

    #[test]
    fn newly_assigned_images_append_to_manual_order() {
        let mut s = session(4);
        s.assign(2, Some(1));
        s.assign(3, Some(1));
        s.set_active(1);
        s.move_to(3, 0);
        assert_eq!(s.visible(), vec![3, 2]);
        s.assign(0, Some(1));
        assert_eq!(s.visible(), vec![3, 2, 0]);
        s.assign(2, Some(4));
        assert_eq!(s.visible(), vec![3, 0]);
    }
}
