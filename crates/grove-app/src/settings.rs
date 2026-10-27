//! Page Paramètres (specs §11.2).

use gpui::{Context, Window};
use grove_core::ProjectId;

use crate::grove_view::GroveView;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    General,
    Terminal,
    Profiles,
    Claude,
    Shortcuts,
}

impl GroveView {
    pub(crate) fn open_settings(&mut self, _: Option<Section>, _: Option<ProjectId>, _: &mut Window, _: &mut Context<Self>) {}
}
