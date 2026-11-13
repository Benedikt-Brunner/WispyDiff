//! Back / forward through the places a jump left, like a browser's history. A jump records
//! where the reader was; going back records where they are for going forward again. Places
//! equal to the current one, or no longer usable, are skipped.

pub struct NavHistory<T> {
    back: Vec<T>,
    forward: Vec<T>,
    same: Box<dyn Fn(&T, &T) -> bool>,
    limit: usize,
}
impl<T> NavHistory<T> {
    pub fn new(same: impl Fn(&T, &T) -> bool + 'static) -> Self {
        Self::with_limit(same, 100)
    }
    pub fn with_limit(same: impl Fn(&T, &T) -> bool + 'static, limit: usize) -> Self {
        Self {
            back: vec![],
            forward: vec![],
            same: Box::new(same),
            limit,
        }
    }
    /// Before a jump: remember `here`; a new jump drops the places gone back from.
    pub fn push(&mut self, here: T) {
        if self
            .back
            .last()
            .is_some_and(|last| (self.same)(last, &here))
        {
            self.back.pop();
        }
        self.back.push(here);
        if self.back.len() > self.limit {
            self.back.remove(0);
        }
        self.forward.clear();
    }
    /// The place to go back to from `here` (which forward then returns to), if any.
    pub fn go_back(&mut self, here: T, usable: impl Fn(&T) -> bool) -> Option<T> {
        Self::step(&mut self.back, &mut self.forward, here, &*self.same, usable)
    }
    pub fn go_forward(&mut self, here: T, usable: impl Fn(&T) -> bool) -> Option<T> {
        Self::step(&mut self.forward, &mut self.back, here, &*self.same, usable)
    }
    fn step(
        from: &mut Vec<T>,
        to: &mut Vec<T>,
        here: T,
        same: &dyn Fn(&T, &T) -> bool,
        usable: impl Fn(&T) -> bool,
    ) -> Option<T> {
        while let Some(place) = from.pop() {
            if !same(&place, &here) && usable(&place) {
                to.push(here);
                return Some(place);
            }
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn back_forward_and_new_jump() {
        let mut h = NavHistory::new(|a: &i32, b| a == b);
        h.push(1);
        h.push(2);
        assert_eq!(h.go_back(3, |_| true), Some(2));
        assert_eq!(h.go_back(2, |_| true), Some(1));
        assert_eq!(h.go_forward(1, |_| true), Some(2));
        h.push(2);
        assert_eq!(h.go_forward(9, |_| true), None);
    }
    #[test]
    fn dedup_updates_place_skips_and_limit() {
        let mut h = NavHistory::with_limit(|a: &i32, b| a / 10 == b / 10, 2);
        h.push(10);
        h.push(11);
        h.push(20);
        h.push(30);
        assert_eq!(h.go_back(31, |p| *p != 20), None);
        assert_eq!(h.go_forward(50, |_| true), None);
        let mut h = NavHistory::with_limit(|a: &i32, b| a / 10 == b / 10, 2);
        h.push(10);
        h.push(11);
        assert_eq!(h.go_back(20, |_| true), Some(11));
        let mut h = NavHistory::with_limit(|a: &i32, b| a == b, 0);
        h.push(1);
        assert_eq!(h.go_back(2, |_| true), None);
    }
}
