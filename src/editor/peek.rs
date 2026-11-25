//! The definition peek: where a name the editor text uses is defined, when
//! that is outside the editor (the hidden `package::shadertoy` module or a
//! LYGIA module), read-only with the definition tinted.

use bevy::prelude::Resource;
use bevy_egui::egui::Ui;

use super::{HighlightCache, Mark, show_marked_code_view};
use crate::analyzer::Definition;

/// The definition shown, if any.
#[derive(Resource, Default)]
pub struct Peek {
    shown: Option<Shown>,
    /// Its own, so it does not take turns with the other code views.
    highlight: HighlightCache,
}

struct Shown {
    title: String,
    lygia: Option<String>,
    source: &'static str,
    mark: Mark,
}

/// What the peek asks the app to do.
#[derive(Debug, PartialEq)]
pub enum PeekAction {
    /// Open the LYGIA module (`generative::snoise`) in the LYGIA window.
    OpenInLygia(String),
}

impl Peek {
    /// Show `definition`, if it is one outside the editor text.
    pub fn open(&mut self, definition: Definition) {
        if let Definition::Module {
            title,
            lygia,
            source,
            range,
        } = definition
        {
            self.shown = Some(Shown {
                title,
                lygia,
                source,
                mark: Mark {
                    range,
                    scroll: true,
                },
            });
        }
    }

    /// The window title while something is shown.
    pub fn title(&self) -> Option<String> {
        self.shown
            .as_ref()
            .map(|s| format!("Definition — {}", s.title))
    }

    pub fn close(&mut self) {
        self.shown = None;
    }

    /// The window's contents.
    pub fn show(&mut self, ui: &mut Ui) -> Option<PeekAction> {
        let shown = self.shown.as_mut()?;
        let mut action = None;
        ui.horizontal(|ui| {
            ui.monospace(&shown.title);
            ui.weak("(read-only)");
            if let Some(module) = &shown.lygia
                && ui
                    .button("open in LYGIA")
                    .on_hover_text("Show this module in the LYGIA window")
                    .clicked()
            {
                action = Some(PeekAction::OpenInLygia(module.clone()));
            }
        });
        show_marked_code_view(ui, shown.source, &mut self.highlight, Some(&mut shown.mark));
        action
    }
}
