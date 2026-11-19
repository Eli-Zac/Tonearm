//! A slot for one listener, so background work can say what happened
//! without knowing who shows it. The app fills each slot once at start.

use std::sync::OnceLock;

pub struct Hook<E>(OnceLock<Box<dyn Fn(E) + Send + Sync>>);

impl<E> Hook<E> {
    pub const fn new() -> Self {
        Hook(OnceLock::new())
    }

    /// Sets the listener. Later calls are ignored.
    pub fn set(&self, listener: impl Fn(E) + Send + Sync + 'static) {
        let _ = self.0.set(Box::new(listener));
    }

    /// Tells the listener, if there is one.
    pub fn emit(&self, event: E) {
        if let Some(listener) = self.0.get() {
            listener(event);
        }
    }
}
