//! `FormController` — a composed controller for a single `Form` section
//! inside a `SidebarSystem`.
//!
//! Parallel to `TreeController` but much thinner: holds the current
//! `Form` value that the app sets per frame via
//! [`FormController::set_form`]. The `Form` primitive owns its own
//! `focused_field` and `has_focus` — the controller is primarily a
//! storage slot that `SidebarSystem::build_view` reads from.

use crate::primitives::form::Form;
use crate::WidgetId;

pub struct FormController {
    id: String,
    form: Option<Form>,
}

impl FormController {
    pub fn new(id: String) -> Self {
        Self { id, form: None }
    }

    pub fn set_form(&mut self, form: Form) {
        self.form = Some(form);
    }

    pub fn form(&self) -> Option<&Form> {
        self.form.as_ref()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn default_form_id(&self) -> WidgetId {
        WidgetId::new(format!("{}-form", self.id))
    }
}
