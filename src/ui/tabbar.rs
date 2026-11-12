use eframe::egui::{Button, PointerButton, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2};

use super::theme;
use crate::tab::Tab;

const MAX_LABEL_WIDTH: f32 = 180.0;

#[derive(Default)]
pub struct TabBarEvents {
    pub activate: Option<usize>,
    pub close: Option<usize>,
    /// Index of the tab the user began dragging.
    pub drag_started: Option<usize>,
    /// Screen-space rect of every tab (label and close button), in order.
    pub tab_rects: Vec<Rect>,
}

/// What an ongoing tab drag changes in this bar's drawing.
#[derive(Default, Clone, Copy)]
pub struct DragView {
    /// Tab being dragged, shown dimmed wherever it still is.
    pub dragged: Option<u64>,
    /// Insertion index to mark when the drag would drop into this bar.
    pub marker: Option<usize>,
}

pub fn title(tab: &Tab) -> String {
    match tab.unseen {
        0 => tab.label(),
        n => format!("{} ({n})", tab.label()),
    }
}

pub fn show(ui: &mut Ui, tabs: &[Tab], active: usize, drag: DragView) -> TabBarEvents {
    let mut events = TabBarEvents::default();
    ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal(|ui| {
            for (index, tab) in tabs.iter().enumerate() {
                let shown = ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.set_max_width(MAX_LABEL_WIDTH + 24.0);
                    let text = match drag.dragged == Some(tab.id) {
                        true => RichText::new(title(tab)).weak(),
                        false => RichText::new(title(tab)),
                    };
                    let label = ui.scope(|ui| {
                        ui.set_max_width(MAX_LABEL_WIDTH);
                        ui.add(Button::selectable(index == active, text).truncate().sense(Sense::click_and_drag()))
                    });
                    let label = label.inner.on_hover_text(tab.root.display().to_string());
                    let close = ui.add(Button::new(RichText::new("×").weak()).frame(false));
                    if label.drag_started_by(PointerButton::Primary) {
                        events.drag_started = Some(index);
                        events.activate = Some(index);
                    } else if label.clicked() {
                        events.activate = Some(index);
                    }
                    if close.clicked() || label.middle_clicked() || close.middle_clicked() {
                        events.close = Some(index);
                    }
                });
                events.tab_rects.push(shown.response.rect);
            }
        });
        if let Some(at) = drag.marker {
            paint_marker(ui, &events.tab_rects, at);
        }
    });
    events
}

fn paint_marker(ui: &Ui, tabs: &[Rect], at: usize) {
    let Some(last) = tabs.last() else { return };
    let (x, rect) = match tabs.get(at) {
        Some(next) => (next.min.x, next),
        None => (last.max.x, last),
    };
    let stroke = Stroke::new(2.0, theme::palette(ui.ctx()).accent);
    ui.painter().line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], stroke);
}
