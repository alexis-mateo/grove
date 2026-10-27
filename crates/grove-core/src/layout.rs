//! Volets d'un onglet (splits) : plusieurs terminaux côte à côte ou empilés.
//!
//! Le volet principal porte l'identifiant de l'onglet et son type (terminal,
//! session Claude) ; les autres sont des terminaux, chacun avec son profil.
//! Leurs identifiants sont des `TabId` : processus, hooks et état SSH se
//! retrouvent par le même identifiant, qu'il s'agisse d'un onglet ou d'un volet.

use serde::{Deserialize, Serialize};

use crate::ids::TabId;
use crate::model::{Tab, TabKind};

/// Disposition des deux moitiés d'un split.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Côte à côte.
    Row,
    /// L'un au-dessus de l'autre.
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Layout {
    Pane { id: TabId },
    Split {
        axis: Axis,
        /// Part de la première moitié, de 0 à 1.
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

/// Volet ajouté à un onglet (autre que le principal).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub id: TabId,
    pub kind: TabKind,
}

/// Ce que la fermeture d'un volet a fait de l'onglet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosedPane {
    Removed,
    /// Le volet principal est parti : `pane` le remplace et prend l'identifiant de l'onglet.
    Promoted { pane: TabId },
}

/// Rectangle d'un volet dans le carré unité.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub const MIN_RATIO: f32 = 0.1;

impl Layout {
    fn pane(id: TabId) -> Self {
        Self::Pane { id }
    }

    /// Volets dans l'ordre de lecture (gauche à droite, haut en bas).
    pub fn panes(&self) -> Vec<TabId> {
        match self {
            Self::Pane { id } => vec![*id],
            Self::Split { first, second, .. } => {
                let mut panes = first.panes();
                panes.extend(second.panes());
                panes
            }
        }
    }

    fn contains(&self, target: TabId) -> bool {
        match self {
            Self::Pane { id } => *id == target,
            Self::Split { first, second, .. } => first.contains(target) || second.contains(target),
        }
    }

    fn replace(&mut self, target: TabId, with: Layout) -> bool {
        match self {
            Self::Pane { id } if *id == target => {
                *self = with;
                true
            }
            Self::Pane { .. } => false,
            Self::Split { first, second, .. } => first.replace(target, with.clone()) || second.replace(target, with),
        }
    }

    /// Retire le volet `target` : l'autre moitié de son split prend toute la place.
    fn remove(&mut self, target: TabId) -> bool {
        let Self::Split { first, second, .. } = self else { return false };
        let keep = if **first == Self::pane(target) {
            second
        } else if **second == Self::pane(target) {
            first
        } else {
            return first.remove(target) || second.remove(target);
        };
        let keep = std::mem::replace(keep.as_mut(), Self::pane(target));
        *self = keep;
        true
    }

    fn rename(&mut self, from: TabId, to: TabId) {
        match self {
            Self::Pane { id } if *id == from => *id = to,
            Self::Pane { .. } => {}
            Self::Split { first, second, .. } => {
                first.rename(from, to);
                second.rename(from, to);
            }
        }
    }

    /// Proportion du split désigné par `path` (0 : première moitié, 1 : seconde, à chaque niveau).
    pub fn ratio_mut(&mut self, path: &[usize]) -> Option<&mut f32> {
        let Self::Split { ratio, first, second, .. } = self else { return None };
        match path.split_first() {
            None => Some(ratio),
            Some((0, rest)) => first.ratio_mut(rest),
            Some((_, rest)) => second.ratio_mut(rest),
        }
    }

    pub fn rects(&self) -> Vec<(TabId, Rect)> {
        let mut rects = Vec::new();
        self.collect_rects(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }, &mut rects);
        rects
    }

    fn collect_rects(&self, area: Rect, rects: &mut Vec<(TabId, Rect)>) {
        match self {
            Self::Pane { id } => rects.push((*id, area)),
            Self::Split { axis, ratio, first, second } => {
                let (a, b) = match axis {
                    Axis::Row => {
                        let w = area.w * ratio;
                        (Rect { w, ..area }, Rect { x: area.x + w, w: area.w - w, ..area })
                    }
                    Axis::Column => {
                        let h = area.h * ratio;
                        (Rect { h, ..area }, Rect { y: area.y + h, h: area.h - h, ..area })
                    }
                };
                first.collect_rects(a, rects);
                second.collect_rects(b, rects);
            }
        }
    }
}

impl Tab {
    pub fn is_split(&self) -> bool {
        self.layout.is_some()
    }

    /// Volets de l'onglet, le principal compris, dans l'ordre de lecture.
    pub fn pane_ids(&self) -> Vec<TabId> {
        self.layout.as_ref().map_or_else(|| vec![self.id], Layout::panes)
    }

    pub fn has_pane(&self, id: TabId) -> bool {
        id == self.id || self.panes.iter().any(|p| p.id == id)
    }

    pub fn pane_kind(&self, id: TabId) -> Option<&TabKind> {
        if id == self.id {
            return Some(&self.kind);
        }
        self.panes.iter().find(|p| p.id == id).map(|p| &p.kind)
    }

    /// Partage le volet `target` en deux ; le nouveau volet (`kind`) va à droite ou dessous.
    pub fn split(&mut self, target: TabId, axis: Axis, kind: TabKind) -> Option<TabId> {
        if !self.pane_ids().contains(&target) {
            return None;
        }
        let id = TabId::new();
        let split = Layout::Split { axis, ratio: 0.5, first: Box::new(Layout::pane(target)), second: Box::new(Layout::pane(id)) };
        match &mut self.layout {
            Some(layout) => {
                layout.replace(target, split);
            }
            None => self.layout = Some(split),
        }
        self.panes.push(Pane { id, kind });
        Some(id)
    }

    /// Ferme un volet. `None` si l'onglet n'a pas d'autre volet : c'est l'onglet qu'il faut fermer.
    pub fn close_pane(&mut self, id: TabId) -> Option<ClosedPane> {
        let layout = self.layout.as_mut()?;
        if !layout.contains(id) || !layout.remove(id) {
            return None;
        }
        let closed = if id == self.id {
            // Le premier volet restant devient le principal.
            let promoted = layout.panes()[0];
            layout.rename(promoted, self.id);
            let index = self.panes.iter().position(|p| p.id == promoted)?;
            self.kind = self.panes.remove(index).kind;
            ClosedPane::Promoted { pane: promoted }
        } else {
            self.panes.retain(|p| p.id != id);
            ClosedPane::Removed
        };
        if matches!(self.layout, Some(Layout::Pane { .. })) {
            self.layout = None;
            self.panes.clear();
        }
        Some(closed)
    }

    /// Volet voisin de `from` dans la direction donnée : le plus proche parmi ceux
    /// qui le bordent de ce côté, en préférant celui qui lui fait le plus face.
    pub fn neighbor(&self, from: TabId, direction: Direction) -> Option<TabId> {
        const EPS: f32 = 1e-4;
        let rects = self.layout.as_ref()?.rects();
        let (_, r) = *rects.iter().find(|(id, _)| *id == from)?;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.0);
        rects
            .iter()
            .filter(|(id, _)| *id != from)
            .filter_map(|&(id, c)| {
                let (gap, facing) = match direction {
                    Direction::Right => (c.x - (r.x + r.w), overlap(r.y, r.y + r.h, c.y, c.y + c.h)),
                    Direction::Left => (r.x - (c.x + c.w), overlap(r.y, r.y + r.h, c.y, c.y + c.h)),
                    Direction::Down => (c.y - (r.y + r.h), overlap(r.x, r.x + r.w, c.x, c.x + c.w)),
                    Direction::Up => (r.y - (c.y + c.h), overlap(r.x, r.x + r.w, c.x, c.x + c.w)),
                };
                (gap > -EPS && facing > EPS).then_some((id, gap, facing))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|(id, _, _)| id)
    }

    /// Fixe la proportion d'un split, bornée pour qu'aucun volet ne disparaisse.
    pub fn set_ratio(&mut self, path: &[usize], ratio: f32) -> bool {
        match self.layout.as_mut().and_then(|l| l.ratio_mut(path)) {
            Some(current) => {
                *current = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Workspace;

    fn terminal() -> TabKind {
        TabKind::Terminal { profile: None }
    }

    fn tab() -> Tab {
        let mut workspace = Workspace::direct("/tmp");
        workspace.add_terminal_tab();
        workspace.tabs.remove(0)
    }

    #[test]
    fn split_and_close_back_to_a_single_pane() {
        let mut tab = tab();
        let main = tab.id;
        assert!(!tab.is_split());
        let right = tab.split(main, Axis::Row, terminal()).unwrap();
        let below = tab.split(right, Axis::Column, TabKind::Terminal { profile: Some("db".into()) }).unwrap();
        assert_eq!(tab.pane_ids(), [main, right, below]);
        assert_eq!(tab.pane_kind(below), Some(&TabKind::Terminal { profile: Some("db".into()) }));

        assert_eq!(tab.close_pane(right), Some(ClosedPane::Removed));
        assert_eq!(tab.pane_ids(), [main, below]);
        assert_eq!(tab.close_pane(below), Some(ClosedPane::Removed));
        assert!(!tab.is_split());
        assert!(tab.panes.is_empty());
        assert_eq!(tab.close_pane(main), None, "dernier volet : c'est l'onglet qu'on ferme");
    }

    #[test]
    fn closing_the_main_pane_promotes_the_next_one() {
        let mut tab = tab();
        let main = tab.id;
        tab.kind = TabKind::Claude { session_id: None, view: Default::default() };
        let right = tab.split(main, Axis::Row, TabKind::Terminal { profile: Some("staging".into()) }).unwrap();
        let below = tab.split(right, Axis::Column, terminal()).unwrap();

        assert_eq!(tab.close_pane(main), Some(ClosedPane::Promoted { pane: right }));
        assert_eq!(tab.kind, TabKind::Terminal { profile: Some("staging".into()) });
        assert_eq!(tab.pane_ids(), [main, below], "le volet promu garde sa place, sous l'id de l'onglet");
        assert!(!tab.has_pane(right));
    }

    #[test]
    fn neighbors_follow_the_geometry() {
        // ┌──────┬──────┐
        // │ main │ b    │
        // │      ├──────┤
        // │      │ c    │
        // └──────┴──────┘
        let mut tab = tab();
        let main = tab.id;
        let b = tab.split(main, Axis::Row, terminal()).unwrap();
        let c = tab.split(b, Axis::Column, terminal()).unwrap();
        assert_eq!(tab.neighbor(main, Direction::Right), Some(b));
        assert_eq!(tab.neighbor(b, Direction::Down), Some(c));
        assert_eq!(tab.neighbor(c, Direction::Up), Some(b));
        assert_eq!(tab.neighbor(c, Direction::Left), Some(main));
        assert_eq!(tab.neighbor(main, Direction::Left), None);
        assert_eq!(tab.neighbor(main, Direction::Up), None);
    }

    #[test]
    fn ratios_are_clamped_and_persisted() {
        let mut tab = tab();
        let main = tab.id;
        let b = tab.split(main, Axis::Row, terminal()).unwrap();
        tab.split(b, Axis::Column, terminal()).unwrap();
        assert!(tab.set_ratio(&[], 0.99));
        assert!(tab.set_ratio(&[1], 0.3));
        assert!(!tab.set_ratio(&[0], 0.3), "la première moitié est un volet, pas un split");
        let json = serde_json::to_string(&tab).unwrap();
        let restored: Tab = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, tab);
        let Some(Layout::Split { ratio, .. }) = &restored.layout else { panic!("split") };
        assert_eq!(*ratio, 1.0 - MIN_RATIO);
    }

    #[test]
    fn tabs_saved_before_splits_have_a_single_pane() {
        let tab: Tab = serde_json::from_str(
            r#"{"id": "3c1b8f9e-0b9a-4a55-9a47-4a1f0f7c2d11", "title": null, "kind": {"type": "terminal"}}"#,
        )
        .unwrap();
        assert!(!tab.is_split());
        assert_eq!(tab.pane_ids(), [tab.id]);
    }
}
