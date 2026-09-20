//! Session state: images, workspaces, filtering and ordering.
//! Pure data — no GTK, no filesystem access.

use std::path::PathBuf;

/// Index into `Session::images`. Stable for the lifetime of the session.
pub type ImageId = usize;

/// Workspaces 1..=10. The tenth sits on the `0` key and is shown as "0".
pub const WORKSPACES: u8 = 10;

/// What a workspace is called on screen: the key that fills it.
pub fn bin_label(workspace: u8) -> String {
    (workspace % 10).to_string()
}
/// View showing the images that are in no workspace yet.
pub const UNBINNED: u8 = WORKSPACES + 1;
/// View showing the marked images — the one non-exclusive set.
pub const MARKED: u8 = WORKSPACES + 2;

#[derive(Debug, Clone)]
pub struct ImageEntry {
    pub path: PathBuf,
    pub workspace: Option<u8>,
    /// Quarter turns clockwise, shown but not yet written to the file.
    pub rotation: u8,
    /// Marked: a flag independent of the workspace, for picking across them.
    pub mark: bool,
    /// The file is gone (trashed by a command); ids stay stable, the entry hides.
    pub removed: bool,
}

/// Sort state of one view. View 0 is "all images", 1..=10 are the
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
    marks: Vec<bool>,
    rotations: Vec<u8>,
    views: Vec<Workspace>,
    active: u8,
    selected: Option<ImageId>,
}

const UNDO_DEPTH: usize = 200;

#[derive(Debug)]
pub struct Session {
    images: Vec<ImageEntry>,
    /// The "natural" order: as given, unless re-sorted by some file property.
    natural: Vec<ImageId>,
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
        let images: Vec<ImageEntry> = paths
            .into_iter()
            .map(|path| ImageEntry { path, workspace: None, rotation: 0, mark: false, removed: false })
            .collect();
        Session {
            natural: (0..images.len()).collect(),
            images,
            views: vec![Workspace::default(); MARKED as usize + 1],
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
            marks: self.images.iter().map(|e| e.mark).collect(),
            rotations: self.images.iter().map(|e| e.rotation).collect(),
            views: self.views.clone(),
            active: self.active,
            selected: self.selected,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        for (entry, workspace) in self.images.iter_mut().zip(snapshot.workspaces) {
            entry.workspace = workspace;
        }
        for (entry, mark) in self.images.iter_mut().zip(snapshot.marks) {
            entry.mark = mark;
        }
        for (entry, rotation) in self.images.iter_mut().zip(snapshot.rotations) {
            entry.rotation = rotation;
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

    /// 0 = all images, 1..=10 = workspace filter, `UNBINNED`.
    pub fn active(&self) -> u8 {
        self.active
    }

    pub fn selected(&self) -> Option<ImageId> {
        self.selected
    }

    pub fn manual_sort(&self) -> bool {
        self.views[self.active as usize].manual_sort_enabled
    }

    /// Number of images still in the session.
    pub fn len(&self) -> usize {
        self.images.iter().filter(|e| !e.removed).count()
    }

    pub fn workspace_count(&self, ws: u8) -> usize {
        self.images.iter().filter(|e| !e.removed && e.workspace == Some(ws)).count()
    }

    pub fn mark_count(&self) -> usize {
        self.images.iter().filter(|e| !e.removed && e.mark).count()
    }

    pub fn unbinned_count(&self) -> usize {
        self.images.iter().filter(|e| !e.removed && e.workspace.is_none()).count()
    }

    fn in_view(&self, view: u8, id: ImageId) -> bool {
        if self.images[id].removed {
            return false;
        }
        match view {
            0 => true,
            UNBINNED => self.images[id].workspace.is_none(),
            MARKED => self.images[id].mark,
            _ => self.images[id].workspace == Some(view),
        }
    }

    /// Image ids shown in the active view, in display order.
    pub fn visible(&self) -> Vec<ImageId> {
        self.view_order(self.active)
    }

    pub fn view_order(&self, view: u8) -> Vec<ImageId> {
        let natural = self.natural.iter().copied().filter(|&id| self.in_view(view, id));
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

    /// Re-sort what unsorted views show. `order` must list every image once;
    /// manually arranged views keep their arrangement.
    pub fn set_natural_order(&mut self, order: Vec<ImageId>) {
        let mut seen = vec![false; self.images.len()];
        let complete = order.len() == seen.len()
            && order.iter().all(|&id| id < seen.len() && !std::mem::replace(&mut seen[id], true));
        if complete {
            self.natural = order;
        }
    }

    pub fn select(&mut self, id: Option<ImageId>) {
        self.selected = id.filter(|&id| id < self.images.len());
    }

    /// Switch the filter. Keeps the selection if still visible, else selects the first image.
    pub fn set_active(&mut self, view: u8) {
        if view > MARKED {
            return;
        }
        self.active = view;
        self.anchor = None;
        let visible = self.visible();
        if !self.selected.is_some_and(|id| visible.contains(&id)) {
            self.selected = visible.first().copied();
        }
    }

    /// Turn the given images by `quarters` clockwise (negative: counter-clockwise).
    /// Session-only, like everything else here; one undo step.
    pub fn rotate(&mut self, ids: &[ImageId], quarters: i8) {
        let turns = quarters.rem_euclid(4) as u8;
        let ids: Vec<ImageId> = ids.iter().copied().filter(|&id| id < self.images.len()).collect();
        if turns == 0 || ids.is_empty() {
            return;
        }
        self.checkpoint(self.snapshot());
        for id in ids {
            self.images[id].rotation = (self.images[id].rotation + turns) % 4;
        }
    }

    pub fn pending_rotations(&self) -> usize {
        self.images.iter().filter(|e| !e.removed && e.rotation != 0).count()
    }

    /// The pending rotation of `id` has been written to its file: the file
    /// is the new baseline, also for every state undo could bring back.
    pub fn rotation_saved(&mut self, id: ImageId) {
        let Some(entry) = self.images.get_mut(id) else { return };
        let saved = std::mem::take(&mut entry.rotation);
        for snapshot in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            // Snapshots from before the image joined the session don't know it.
            if let Some(rotation) = snapshot.rotations.get_mut(id) {
                *rotation = (*rotation + 4 - saved) % 4;
            }
        }
    }

    /// The next (or previous) binned image in the active view, wrapping around.
    pub fn next_binned(&self, forward: bool) -> Option<ImageId> {
        let visible = self.visible();
        let len = visible.len();
        let start = self.selected.and_then(|id| visible.iter().position(|&x| x == id));
        // Without a selection, start just outside so the first/last image counts.
        let start = start.unwrap_or(if forward { len.saturating_sub(1) } else { 0 });
        (1..=len)
            .map(|step| if forward { (start + step) % len } else { (start + len - step % len) % len })
            .map(|position| visible[position])
            .find(|&id| self.images[id].workspace.is_some() && Some(id) != self.selected)
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
    pub fn selection(&self) -> Vec<ImageId> {
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

    /// Bin keys toggle: what pressing the key of `ws` should do to the marked
    /// images — take them out if every one is in there already, else put them in.
    pub fn toggle_target(&self, ws: u8) -> Option<u8> {
        let marked = self.selection();
        let all_in = !marked.is_empty() && marked.iter().all(|&id| self.images[id].workspace == Some(ws));
        (!all_in).then_some(ws)
    }

    /// Assign everything marked, as a single undo step. Ends the range.
    pub fn assign_selection(&mut self, ws: Option<u8>) {
        let marked = self.selection();
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
        self.reselect(&before, &ids);
    }

    /// If the selected image just left the view (`before`: the view as it
    /// was, `changed`: what was touched), whatever followed slides into place.
    fn reselect(&mut self, before: &[ImageId], changed: &[ImageId]) {
        if self.selected.is_some_and(|id| !self.in_view(self.active, id)) {
            let after = self.visible();
            let first = before.iter().position(|id| changed.contains(id)).unwrap_or(0);
            self.selected = after.get(first).or(after.last()).copied();
        }
    }

    /// Toggle the mark of the selection: off if all of it is marked, else on.
    /// Marks don't touch workspaces. One undo step; ends a range.
    pub fn toggle_mark(&mut self) {
        let ids = self.selection();
        if ids.is_empty() {
            return;
        }
        let mark = !ids.iter().all(|&id| self.images[id].mark);
        self.checkpoint(self.snapshot());
        let before = self.visible();
        for &id in &ids {
            self.images[id].mark = mark;
            let order = &mut self.views[MARKED as usize].explicit_order;
            order.retain(|&x| x != id);
            if mark {
                order.push(id);
            }
        }
        self.anchor = None;
        self.reselect(&before, &ids);
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

    /// New files join the session: unbinned, at the end of the natural order.
    pub fn add(&mut self, paths: Vec<PathBuf>) -> Vec<ImageId> {
        let first = self.images.len();
        for path in paths {
            self.natural.push(self.images.len());
            self.images.push(ImageEntry { path, workspace: None, rotation: 0, mark: false, removed: false });
        }
        (first..self.images.len()).collect()
    }

    /// What would be lost by starting over, e.g. "12 binned, 3 marked".
    /// `None` if the session holds no organizing work.
    pub fn work_summary(&self) -> Option<String> {
        let present = || self.images.iter().filter(|e| !e.removed);
        let counts = [
            (present().filter(|e| e.workspace.is_some()).count(), "binned"),
            (present().filter(|e| e.mark).count(), "marked"),
            (present().filter(|e| e.rotation != 0).count(), "rotated"),
            (self.views.iter().filter(|v| v.manual_sort_enabled).count(), "arranged by hand"),
        ];
        let parts: Vec<String> =
            counts.iter().filter(|(n, _)| *n > 0).map(|(n, what)| format!("{n} {what}")).collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }

    /// Start over with other images: everything so far leaves, along with
    /// all bins, marks, orders and the undo history.
    pub fn replace(&mut self, paths: Vec<PathBuf>) -> Vec<ImageId> {
        for entry in &mut self.images {
            entry.removed = true;
        }
        self.views.iter_mut().for_each(|view| *view = Workspace::default());
        self.undo.clear();
        self.redo.clear();
        self.active = 0;
        self.anchor = None;
        let ids = self.add(paths);
        self.selected = ids.first().copied();
        ids
    }

    /// The files behind `ids` no longer exist. Not an undo step: undo never
    /// brings files back, and a removed image stays hidden in every state.
    pub fn remove(&mut self, ids: &[ImageId]) {
        let before = self.visible();
        for &id in ids {
            if let Some(entry) = self.images.get_mut(id) {
                entry.removed = true;
            }
        }
        self.anchor = None;
        if self.selected.is_some_and(|id| self.images[id].removed) {
            let after = self.visible();
            let first = before.iter().position(|id| ids.contains(id)).unwrap_or(0);
            self.selected = after.get(first).or(after.last()).copied();
        }
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
        s.assign(1, Some(11));
        assert_eq!(s.image(1).workspace, None);
        s.assign(1, Some(10)); // the tenth workspace, on the 0 key
        assert_eq!(s.view_order(10), vec![1]);
        assert_eq!((bin_label(10), bin_label(3)), ("0".to_string(), "3".to_string()));
    }

    #[test]
    fn marks_are_independent_of_workspaces() {
        let mut s = session(5);
        s.assign(1, Some(1));
        s.assign(3, Some(2));
        s.select_range(1, 3);
        s.toggle_mark(); // 1, 2, 3 — across two bins and an unbinned image
        assert_eq!(s.mark_count(), 3);
        assert_eq!((s.image(1).workspace, s.image(3).workspace), (Some(1), Some(2)));
        s.select_range(3, 4);
        s.toggle_mark(); // mixed: the rest gets marked too
        assert_eq!(s.view_order(MARKED), vec![1, 2, 3, 4]);

        s.set_active(MARKED);
        s.select(Some(2));
        s.toggle_mark(); // unmarked in its own view: gone, the next one is up
        assert_eq!(s.visible(), vec![1, 3, 4]);
        assert_eq!(s.selected(), Some(3));
        s.assign_selection(Some(7)); // binning here changes nothing about the mark
        assert_eq!(s.visible(), vec![1, 3, 4]);

        s.move_to(4, 0); // the marked view has its own order
        assert_eq!(s.visible(), vec![4, 1, 3]);
        assert!(s.undo() && s.undo() && s.undo());
        assert_eq!(s.view_order(MARKED), vec![1, 2, 3, 4]);
    }

    #[test]
    fn bin_keys_toggle() {
        let mut s = session(3);
        s.select(Some(0));
        assert_eq!(s.toggle_target(1), Some(1));
        s.assign_selection(s.toggle_target(1));
        assert_eq!(s.toggle_target(2), Some(2)); // another bin: move there
        assert_eq!(s.toggle_target(1), None); // its own bin: out
        s.assign_selection(s.toggle_target(1));
        assert_eq!(s.image(0).workspace, None);

        // A range goes out only if all of it is in; otherwise the rest joins.
        s.assign(1, Some(4));
        s.select_range(0, 1);
        assert_eq!(s.toggle_target(4), Some(4));
        s.assign_selection(Some(4));
        s.select_range(0, 1);
        assert_eq!(s.toggle_target(4), None);
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
        s.assign_selection(Some(1)); // image 0 goes, the next one is up
        assert_eq!(s.visible(), vec![2, 3]);
        assert_eq!(s.selected(), Some(2));
        s.set_active(2);
        s.assign_selection(None); // and back onto the pile
        assert_eq!(s.unbinned_count(), 3);
    }

    #[test]
    fn replacing_the_session_starts_clean() {
        let mut s = session(3);
        assert_eq!(s.work_summary(), None);
        s.assign(0, Some(2));
        s.select(Some(1));
        s.toggle_mark();
        s.rotate(&[2], 1);
        s.move_to(2, 0);
        assert_eq!(s.work_summary().as_deref(), Some("1 binned, 1 marked, 1 rotated, 1 arranged by hand"));
        s.set_active(2);

        let ids = s.replace(vec![PathBuf::from("/q/a.jpg"), PathBuf::from("/q/b.jpg")]);
        assert_eq!(ids, vec![3, 4]);
        assert_eq!((s.active(), s.visible(), s.selected()), (0, vec![3, 4], Some(3)));
        assert_eq!((s.len(), s.work_summary()), (2, None));
        assert!(!s.undo() && !s.manual_sort());
        assert!(s.view_order(2).is_empty());
    }

    #[test]
    fn images_can_join_later() {
        let mut s = session(2);
        s.assign(0, Some(1));
        s.rotate(&[1], 1);
        let ids = s.add(vec![PathBuf::from("/p/new.jpg")]);
        assert_eq!(ids, vec![2]);
        assert_eq!(s.visible(), vec![0, 1, 2]);
        assert_eq!(s.unbinned_count(), 2);
        s.assign(2, Some(1));
        s.rotate(&[2], 1);
        s.rotation_saved(2); // older snapshots have no slot for image 2
        while s.undo() {} // through states from before it existed, too
        // Unbinned again; its saved quarter turn is undone by three more, pending.
        assert_eq!((s.image(2).workspace, s.image(2).rotation), (None, 3));
        assert_eq!(s.visible().len(), 3); // undo never makes an image disappear
        s.set_natural_order(vec![2, 1, 0]);
        assert_eq!(s.visible(), vec![2, 1, 0]);
    }

    #[test]
    fn natural_order_can_be_resorted() {
        let mut s = session(4);
        s.assign(3, Some(1));
        s.assign(0, Some(1));
        s.set_natural_order(vec![3, 2, 1, 0]);
        assert_eq!(s.visible(), vec![3, 2, 1, 0]);
        assert_eq!(s.view_order(1), vec![3, 0]);
        s.set_natural_order(vec![0, 1]); // incomplete: ignored
        s.set_natural_order(vec![0, 0, 1, 2]); // not a permutation: ignored
        assert_eq!(s.visible(), vec![3, 2, 1, 0]);

        s.set_active(1);
        s.move_to(0, 0); // manual: [0, 3]
        s.set_natural_order(vec![0, 1, 2, 3]);
        assert_eq!(s.visible(), vec![0, 3]);
        s.toggle_manual_sort();
        assert_eq!(s.visible(), vec![0, 3]);
    }

    #[test]
    fn removed_images_vanish_for_good() {
        let mut s = session(4);
        s.assign(1, Some(1));
        s.assign(2, Some(1));
        s.set_active(1);
        s.select(Some(1));
        s.remove(&[1]);
        assert_eq!(s.visible(), vec![2]);
        assert_eq!(s.selected(), Some(2));
        assert_eq!((s.len(), s.workspace_count(1)), (3, 1));
        assert!(s.undo()); // back before image 2 was binned; 1 stays gone
        s.set_active(0);
        assert_eq!(s.visible(), vec![0, 2, 3]);
        s.remove(&[0, 2, 3]);
        assert_eq!(s.selected(), None);
    }

    #[test]
    fn rotation_is_pending_and_undoable() {
        let mut s = session(3);
        s.rotate(&[0, 1], 1);
        s.rotate(&[1], -2);
        assert_eq!((s.image(0).rotation, s.image(1).rotation), (1, 3));
        assert_eq!(s.pending_rotations(), 2);
        s.rotate(&[2], 4); // a full turn is nothing
        assert!(s.undo());
        assert_eq!(s.image(1).rotation, 1);

        // Image 1 is written to disk with one quarter turn. Undoing to the
        // state before any rotation now means turning it back by one.
        s.rotation_saved(1);
        assert_eq!(s.image(1).rotation, 0);
        assert!(s.undo());
        assert_eq!((s.image(0).rotation, s.image(1).rotation), (0, 3));
        assert!(s.redo());
        assert_eq!((s.image(0).rotation, s.image(1).rotation), (1, 0));
    }

    #[test]
    fn jumping_between_binned_images() {
        let mut s = session(6);
        s.select(Some(0));
        assert_eq!(s.next_binned(true), None);
        s.assign(1, Some(1));
        s.assign(4, Some(2));
        assert_eq!(s.next_binned(true), Some(1));
        assert_eq!(s.next_binned(false), Some(4)); // wraps
        s.select(Some(4));
        assert_eq!(s.next_binned(true), Some(1));
        assert_eq!(s.next_binned(false), Some(1));
        s.assign(1, None);
        assert_eq!(s.next_binned(true), None); // only the selected one is left
        s.select(None);
        assert_eq!(s.next_binned(true), Some(4));
        assert_eq!(Session::new(Vec::new()).next_binned(true), None);
    }

    #[test]
    fn range_assignment() {
        let mut s = session(6);
        s.select(Some(4));
        assert_eq!(s.selection(), vec![4]);
        s.toggle_range();
        s.select(Some(2)); // ranges work backwards too
        assert_eq!(s.selection(), vec![2, 3, 4]);
        s.assign_selection(Some(1));
        assert!(!s.has_range());
        assert_eq!(s.view_order(1), vec![2, 3, 4]);

        s.set_active(1);
        s.select_range(2, 3);
        s.assign_selection(Some(2));
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
