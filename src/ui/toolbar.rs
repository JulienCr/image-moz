use eframe::egui::{ComboBox, Slider, Ui};

use crate::config::{Config, MAX_SIZE, MIN_SIZE};
use crate::layout::{BentoRule, LayoutKind};
use crate::library::SortOrder;

#[derive(Default)]
pub struct ToolbarEvents {
    pub open: bool,
    /// The recursive toggle flipped: the folder must be re-read and re-watched.
    pub recursive_changed: bool,
    /// Any persisted setting changed.
    pub config_changed: bool,
}

fn sort_label(sort: SortOrder) -> &'static str {
    match sort {
        SortOrder::DateDesc => "Date ↓",
        SortOrder::DateAsc => "Date ↑",
        SortOrder::NameAsc => "Nom A→Z",
        SortOrder::NameDesc => "Nom Z→A",
    }
}

fn rule_label(rule: BentoRule) -> &'static str {
    match rule {
        BentoRule::Recent => "Récentes",
        BentoRule::Pinned => "Épinglées",
        BentoRule::Resolution => "Plus grandes",
    }
}

pub fn show(ui: &mut Ui, config: &mut Config) -> ToolbarEvents {
    let mut events = ToolbarEvents::default();
    ui.horizontal_wrapped(|ui| {
        if ui.button("Ouvrir").clicked() {
            events.open = true;
        }
        if ui.checkbox(&mut config.recursive, "Récursif").changed() {
            events.recursive_changed = true;
            events.config_changed = true;
        }
        ui.separator();
        for (kind, label, hint) in [
            (LayoutKind::Grid, "Grille", "Grille uniforme (1)"),
            (LayoutKind::Justified, "Justifié", "Rangées justifiées (2)"),
            (LayoutKind::Bento, "Bento", "Mosaïque (3)"),
        ] {
            let response = ui.selectable_value(&mut config.layout, kind, label).on_hover_text(hint);
            events.config_changed |= response.changed();
        }
        ui.separator();
        ComboBox::from_id_salt("sort_order")
            .selected_text(sort_label(config.sort))
            .width(90.0)
            .show_ui(ui, |ui| {
                for sort in [SortOrder::DateDesc, SortOrder::DateAsc, SortOrder::NameAsc, SortOrder::NameDesc] {
                    events.config_changed |=
                        ui.selectable_value(&mut config.sort, sort, sort_label(sort)).changed();
                }
            });
        if config.layout == LayoutKind::Bento {
            ComboBox::from_id_salt("bento_rule")
                .selected_text(rule_label(config.bento_rule))
                .width(100.0)
                .show_ui(ui, |ui| {
                    for rule in [BentoRule::Recent, BentoRule::Pinned, BentoRule::Resolution] {
                        events.config_changed |=
                            ui.selectable_value(&mut config.bento_rule, rule, rule_label(rule)).changed();
                    }
                });
        }
        ui.separator();
        ui.spacing_mut().slider_width = 140.0;
        let slider = Slider::new(&mut config.target_size, MIN_SIZE..=MAX_SIZE)
            .step_by(1.0)
            .suffix(" px");
        events.config_changed |= ui.add(slider).changed();
    });
    events
}
