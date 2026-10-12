//! Toastrack stick maker: pick a USB stick, click Write.
//!
//! A thin front end over `toastrack-installer`; everything it does the command line can do too.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod model;

use eframe::egui::{self, Color32, RichText};
use model::{Mode, Model, Phase, Source};
use toastrack_installer::{bundle, install::DataFs};

const ACCENT: Color32 = Color32::from_rgb(235, 170, 60);
const GOOD: Color32 = Color32::from_rgb(46, 160, 67);
const BAD: Color32 = Color32::from_rgb(218, 54, 51);

struct App {
    m: Model,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.m.poll();
        if matches!(self.m.phase, Phase::Working) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.ui(ui));
        });
        self.confirm_window(ctx);
    }
}

impl App {
    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("TOASTRACK").size(28.0).strong().color(ACCENT));
        ui.label("Make a multiboot USB stick: copy ISO files onto it and pick one at boot. Works with Secure Boot.");
        ui.add_space(8.0);

        if !self.m.elevated {
            ui.group(|ui| {
                ui.colored_label(BAD, "Not running as administrator, so writing to a disk will fail.");
                #[cfg(target_os = "linux")]
                if ui.button("Restart with administrator rights").clicked() {
                    model::relaunch_elevated();
                }
            });
            ui.add_space(8.0);
        }

        // 1. the stick
        ui.heading("1. Choose the USB stick");
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                self.m.refresh();
            }
            if ui.checkbox(&mut self.m.show_internal, "Show internal disks too (careful)").changed() {
                self.m.refresh();
            }
        });
        if self.m.visible_disks().is_empty() {
            ui.label(RichText::new("No USB stick found. Plug one in and press Refresh.").italics());
        }
        let mut chosen = None;
        for (i, d) in self.m.visible_disks() {
            let selected = self.m.selected == Some(i);
            let label = format!("{}   {:.1} GB   {}{}", d.path.display(), d.size as f64 / 1e9, if d.model.is_empty() { "(unknown)" } else { &d.model }, model::letters(&d.letters));
            if ui.selectable_label(selected, label).clicked() {
                chosen = Some(i);
            }
        }
        if chosen.is_some() {
            self.m.selected = chosen;
        }
        ui.collapsing("Use an image file instead (for virtual machines)", |ui| {
            ui.horizontal(|ui| {
                ui.label("File:");
                ui.text_edit_singleline(&mut self.m.image_path);
            });
            ui.horizontal(|ui| {
                ui.label("Size if new (e.g. 8G):");
                ui.add(egui::TextEdit::singleline(&mut self.m.image_size).desired_width(60.0));
            });
            if !self.m.image_path.trim().is_empty() {
                ui.label("The image file is used instead of the selection above.");
            }
        });
        ui.add_space(8.0);

        // 2. options
        ui.heading("2. Options");
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.m.mode, Mode::Install, "Erase and set up a new stick");
            ui.radio_value(&mut self.m.mode, Mode::Update, "Update boot files only (keep my ISOs)");
        });
        if self.m.mode == Mode::Install {
            ui.horizontal(|ui| {
                ui.label("ISO partition:");
                ui.radio_value(&mut self.m.data_fs, DataFs::Exfat, "exFAT (files over 4 GB)");
                ui.radio_value(&mut self.m.data_fs, DataFs::Fat32, "FAT32 (works everywhere, 4 GB limit)");
            });
        }
        ui.add_space(4.0);
        ui.collapsing("Boot files (advanced)", |ui| {
            let mut changed = false;
            changed |= ui.radio_value(&mut self.m.source, Source::Bundle, "Secure Boot bundle (recommended)").changed();
            changed |= ui.radio_value(&mut self.m.source, Source::Unsigned, "Unsigned loader, PCs with Secure Boot off only").changed();
            ui.horizontal(|ui| {
                ui.label(if self.m.source == Source::Bundle { "Bundle folder:" } else { "Loader file:" });
                changed |= ui.text_edit_singleline(&mut self.m.source_path).changed();
            });
            if self.m.source == Source::Unsigned {
                ui.horizontal(|ui| {
                    ui.label("Windows helper (optional):");
                    changed |= ui.text_edit_singleline(&mut self.m.attach_path).changed();
                });
            }
            if changed {
                self.m.reload_bundle();
            }
        });
        match &self.m.bundle {
            Ok(b) => {
                let what = if b.secure_boot { "Secure Boot bundle" } else { "Unsigned loader (Secure Boot must be off)" };
                let win = if b.attach_exe.is_some() { ", with Windows helper" } else { ", without Windows helper" };
                ui.colored_label(GOOD, format!("Boot files ready: {what}{win}."));
            }
            Err(e) => {
                ui.colored_label(BAD, format!("Boot files: {e}"));
            }
        }
        ui.add_space(8.0);

        // 3. write
        ui.heading("3. Write");
        let ready = self.m.can_write();
        let label = match self.m.mode {
            Mode::Install => "Write stick…",
            Mode::Update => "Update stick…",
        };
        ui.add_enabled_ui(ready && !matches!(self.m.phase, Phase::Working), |ui| {
            if ui.add(egui::Button::new(RichText::new(label).size(18.0)).min_size(egui::vec2(200.0, 36.0))).clicked() {
                self.m.phase = Phase::Confirm { understood: false };
            }
        });
        if let Some(why) = self.m.why_not() {
            ui.label(RichText::new(why).italics());
        }
        ui.add_space(8.0);

        match &self.m.phase {
            Phase::Working => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Writing… do not remove the stick.");
                });
            }
            Phase::Done(Ok(steps)) => {
                ui.group(|ui| {
                    ui.colored_label(GOOD, RichText::new("Done.").strong());
                    for s in steps {
                        ui.label(s);
                    }
                });
            }
            Phase::Done(Err(e)) => {
                ui.group(|ui| {
                    ui.colored_label(BAD, RichText::new("Failed").strong());
                    ui.label(e);
                });
            }
            _ => {}
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Phase::Confirm { understood } = self.m.phase else { return };
        let mut understood = understood;
        let mut go = false;
        let mut cancel = false;
        egui::Window::new("Are you sure?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(self.m.confirm_text());
            if self.m.mode == Mode::Install {
                ui.add_space(6.0);
                ui.checkbox(&mut understood, "I understand that everything on it will be erased");
            } else {
                understood = true;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(understood, |ui| {
                    if ui.button(RichText::new("Continue").color(BAD)).clicked() {
                        go = true;
                    }
                });
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if go {
            self.m.start();
        } else if cancel {
            self.m.phase = Phase::Idle;
        } else {
            self.m.phase = Phase::Confirm { understood };
        }
    }
}

fn main() -> eframe::Result {
    let _ = bundle::CERT; // keep the dependency's purpose obvious: the bundle format lives in the installer crate
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([660.0, 760.0]).with_min_inner_size([520.0, 520.0]).with_title("Toastrack"),
        ..Default::default()
    };
    eframe::run_native("Toastrack", opts, Box::new(|cc| {
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        Ok(Box::new(App { m: Model::new() }))
    }))
}
