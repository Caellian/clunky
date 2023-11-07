use std::{sync::RwLock, cell::RefCell};

use crate::component::Component;

pub struct DirtMark(RwLock<bool>);

impl Default for DirtMark {
    fn default() -> Self {
        DirtMark(RwLock::new(true))
    }
}

impl DirtMark {
    pub fn make_dirty(&mut self) {
        self.0.get_mut().map(|it| *it = true);
    }

    pub fn make_clean(&mut self) {
        self.0.get_mut().map(|it| *it = false);
    }

    pub fn is_dirty(&mut self) -> bool {
        if let Ok(value) = self.0.read().map(|it| *it) {
            return value
        }
        
        self.0 = RwLock::new(true);
        true
    }
}

#[derive(Default)]
pub struct Layout {
    pub components: Vec<Box<dyn Component>>,
    pub dirty: RefCell<DirtMark>,
}

impl Layout {
    pub fn new() -> Layout {
        Self::default()
    }

    pub fn push(&mut self, component: Box<dyn Component>) {
        self.components.push(component);
        self.dirty.borrow_mut().make_dirty();
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.borrow_mut().is_dirty()
    }
}

impl rlua::UserData for &mut Layout {

}