use eframe::egui::{Button, RichText, ScrollArea, Ui};

use crate::tab::Tab;

const MAX_LABEL_WIDTH: f32 = 180.0;

#[derive(Default)]
pub struct TabBarEvents {
    pub activate: Option<usize>,
    pub close: Option<usize>,
}

fn title(tab: &Tab) -> String {
    match tab.unseen {
        0 => tab.label(),
        n => format!("{} ({n})", tab.label()),
    }
}

pub fn show(ui: &mut Ui, tabs: &[Tab], active: usize) -> TabBarEvents {
    let mut events = TabBarEvents::default();
    ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal(|ui| {
            for (index, tab) in tabs.iter().enumerate() {
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.set_max_width(MAX_LABEL_WIDTH + 24.0);
                    let label = ui.scope(|ui| {
                        ui.set_max_width(MAX_LABEL_WIDTH);
                        ui.add(Button::selectable(index == active, title(tab)).truncate())
                    });
                    let label = label.inner.on_hover_text(tab.root.display().to_string());
                    let close = ui.add(Button::new(RichText::new("×").weak()).frame(false));
                    if label.clicked() {
                        events.activate = Some(index);
                    }
                    if close.clicked() || label.middle_clicked() || close.middle_clicked() {
                        events.close = Some(index);
                    }
                });
            }
        });
    });
    events
}
