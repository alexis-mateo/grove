//! Historique des onglets visités, parcouru avec les boutons précédent / suivant
//! de la souris, comme dans un navigateur.

/// Au-delà, les visites les plus anciennes sont oubliées.
const MAX_ENTRIES: usize = 100;

pub struct NavHistory<T> {
    back: Vec<T>,
    forward: Vec<T>,
    current: Option<T>,
}

impl<T> Default for NavHistory<T> {
    fn default() -> Self {
        Self { back: Vec::new(), forward: Vec::new(), current: None }
    }
}

impl<T: Copy + PartialEq> NavHistory<T> {
    /// Note l'emplacement affiché. S'il a changé, l'ancien rejoint « précédent »
    /// et « suivant » est oublié.
    pub fn visit(&mut self, location: Option<T>) {
        if location == self.current {
            return;
        }
        if let Some(previous) = self.current.take() {
            if self.back.last() != Some(&previous) {
                self.back.push(previous);
            }
            if self.back.len() > MAX_ENTRIES {
                self.back.remove(0);
            }
            self.forward.clear();
        }
        self.current = location;
    }

    /// Emplacement précédent encore valide (`exists`), qui devient le courant.
    pub fn back(&mut self, exists: impl Fn(T) -> bool) -> Option<T> {
        Self::step(&mut self.back, &mut self.forward, &mut self.current, exists)
    }

    /// Emplacement suivant encore valide (`exists`), qui devient le courant.
    pub fn forward(&mut self, exists: impl Fn(T) -> bool) -> Option<T> {
        Self::step(&mut self.forward, &mut self.back, &mut self.current, exists)
    }

    /// Dépile `from` jusqu'à un emplacement valide et différent du courant ;
    /// les autres (onglets fermés) sont oubliés. Le courant passe sur `to`.
    fn step(from: &mut Vec<T>, to: &mut Vec<T>, current: &mut Option<T>, exists: impl Fn(T) -> bool) -> Option<T> {
        while let Some(target) = from.pop() {
            if Some(target) == *current || !exists(target) {
                continue;
            }
            if let Some(previous) = current.replace(target) {
                to.push(previous);
            }
            return Some(target);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::NavHistory;

    fn history(visits: &[u32]) -> NavHistory<u32> {
        let mut history = NavHistory::default();
        for &v in visits {
            history.visit(Some(v));
        }
        history
    }

    #[test]
    fn back_then_forward() {
        let mut h = history(&[1, 2, 3]);
        let all = |_| true;
        assert_eq!(h.back(all), Some(2));
        // La vue se redessine sur 2 : pas une nouvelle visite.
        h.visit(Some(2));
        assert_eq!(h.back(all), Some(1));
        assert_eq!(h.back(all), None);
        assert_eq!(h.forward(all), Some(2));
        assert_eq!(h.forward(all), Some(3));
        assert_eq!(h.forward(all), None);
    }

    #[test]
    fn new_visit_clears_forward() {
        let mut h = history(&[1, 2, 3]);
        h.back(|_| true);
        h.visit(Some(4));
        assert_eq!(h.forward(|_| true), None);
        assert_eq!(h.back(|_| true), Some(2));
    }

    #[test]
    fn skips_closed_locations() {
        let mut h = history(&[1, 2, 3, 4]);
        assert_eq!(h.back(|v| v != 3 && v != 2), Some(1));
        assert_eq!(h.forward(|v| v != 3 && v != 2), Some(4));
    }

    #[test]
    fn no_consecutive_duplicates() {
        // 1 → 2 → 1 → 2 : précédent donne 1, puis 2, sans répéter.
        let mut h = history(&[1, 2, 1, 2]);
        assert_eq!(h.back(|_| true), Some(1));
        assert_eq!(h.back(|_| true), Some(2));
        assert_eq!(h.back(|_| true), Some(1));
        assert_eq!(h.back(|_| true), None);
    }

    #[test]
    fn nothing_displayed_keeps_previous() {
        let mut h = history(&[1]);
        h.visit(None);
        assert_eq!(h.back(|_| true), Some(1));
    }

    #[test]
    fn bounded() {
        let visits: Vec<u32> = (0..250).collect();
        let mut h = history(&visits);
        let mut count = 0;
        while h.back(|_| true).is_some() {
            count += 1;
        }
        assert_eq!(count, super::MAX_ENTRIES);
    }
}
