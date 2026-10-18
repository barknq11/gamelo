//! First-run setup: check the tools games need, then bring games in.
//! Shown once for a new, empty library; Settings can run it again.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, tr};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;

#[derive(Default)]
pub struct Setup {
    pub step: u8,
}

/// A big clickable option: a title and one line under it.
fn option_card(ui: &mut egui::Ui, title: &str, sub: &str, w: f32) -> egui::Response {
    let id = ui.scope_id().with(("setup-option", title));
    let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 14))
        .corner_radius(rad(12))
        .fill(if hovered { pal().hover } else { pal().card })
        .stroke(egui::Stroke::new(1.0, if hovered { pal().accent } else { pal().border }));
    let inner = frame.show(ui, |ui| {
        ui.set_width(w - 34.0);
        vcol(ui, |ui| {
            ui.label(egui::RichText::new(tr(title)).strong().color(pal().text));
            para(ui, sub, pal().subtext, true);
        });
    });
    ui.interact(inner.response.rect, id, egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl App {
    pub fn setup_modal(&mut self, ctx: &egui::Context, acts: &mut Acts) {
        let Some(mut s) = self.setup.take() else { return };
        let mut finish = false;
        let frame = egui::Frame::new().fill(pal().panel).stroke(egui::Stroke::new(1.0, pal().border)).inner_margin(egui::Margin::same(28)).corner_radius(rad(18));
        egui::Modal::new(egui::Id::unique("first-run")).frame(frame).show(ctx, |ui| {
            dir_ui(ui, |ui| {
                ui.set_width(640.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover());
                    brand_mark(ui, r);
                    ui.add_space(10.0);
                    vcol(ui, |ui| {
                        ui.label(egui::RichText::new(tr("Welcome to Gamelo")).size(24.0).strong().color(pal().text));
                        ui.label(egui::RichText::new(tr("Two quick steps and you're ready to play.")).color(pal().subtext));
                    });
                });
                ui.add_space(18.0);
                let step_name = if s.step == 0 { lg("Tools") } else { lg("Your games") };
                ui.label(egui::RichText::new(disp(&lfmt("Step {} of 2 · {}", &[&(s.step + 1), &step_name]))).small().strong().color(pal().accent));
                ui.add_space(8.0);
                if s.step == 0 {
                    para(ui, "Gamelo runs Windows games with umu-run and Proton. Both install into your home folder; no root password needed.", pal().subtext, true);
                    ui.add_space(10.0);
                    let busy = self.download.is_some();
                    let rows: Vec<&DepRow> = self.deps.iter().filter(|r| r.name.contains("Proton") || r.name.contains("umu")).collect();
                    for row in rows {
                        card_frame().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                status_dot(ui, if row.found { pal().ok } else { pal().warn }, row.found);
                                vcol(ui, |ui| {
                                    ui.label(egui::RichText::new(tr(row.name)).strong().color(pal().text));
                                    ui.add(egui::Label::new(egui::RichText::new(tr(&row.detail)).small().color(pal().subtext)).truncate());
                                });
                                ui.with_layout(far_end(), |ui| {
                                    if row.found {
                                        pill(ui, "Ready", pal().ok);
                                    } else if let Some((action, label)) = row.action {
                                        ui.add_enabled_ui(!busy, |ui| {
                                            if primary_button(ui, label).clicked() {
                                                acts.dep = Some(action);
                                            }
                                        });
                                    }
                                });
                            });
                        });
                        ui.add_space(8.0);
                    }
                    if let Some(d) = &self.download {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new(tr(&d.label)).color(pal().subtext));
                        });
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, "Next").clicked() {
                            s.step = 1;
                        }
                        if ui.button(tr("Skip setup")).clicked() {
                            finish = true;
                        }
                    });
                } else {
                    para(ui, "Bring your games in. You can always add more later from the Library.", pal().subtext, true);
                    ui.add_space(12.0);
                    let w = (ui.available_width() - 12.0) / 2.0;
                    let options: [(&str, &str); 5] = [
                        ("Scan a folder", "Finds every game in a folder and adds them together."),
                        ("From Steam", "Non-Steam games you added to Steam, with their prefixes and artwork."),
                        ("From Lutris", "Games you installed with Lutris, keeping their prefixes."),
                        ("Epic Games or GOG", "Sign in and install the games you own there."),
                        ("Add one game", "Pick a single game's executable."),
                    ];
                    let mut clicked = None;
                    for pair in options.chunks(2) {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            for (title, sub) in pair {
                                if option_card(ui, title, sub, w).clicked() {
                                    clicked = Some(*title);
                                }
                            }
                        });
                        ui.add_space(12.0);
                    }
                    match clicked {
                        Some("Scan a folder") => acts.scan = true,
                        Some("From Steam") => acts.steam_import = true,
                        Some("From Lutris") => acts.lutris_import = true,
                        Some("Add one game") => acts.add = true,
                        Some(_) => {
                            self.page = Page::Stores;
                            finish = true;
                        }
                        None => {}
                    }
                    let n = self.cfg.custom_games.len();
                    let count = if n == 1 { lg("1 game in your library").to_string() } else { lfmt("{} games in your library", &[&n]) };
                    ui.label(egui::RichText::new(disp(&count)).color(if n > 0 { pal().ok } else { pal().subtext }));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, "Finish").clicked() {
                            finish = true;
                        }
                        if ui.button(tr("Back")).clicked() {
                            s.step = 0;
                        }
                    });
                }
            })
        });
        if finish {
            self.cfg.settings.setup_done = true;
            save_config(&self.cfg);
        } else {
            self.setup = Some(s);
        }
    }
}
