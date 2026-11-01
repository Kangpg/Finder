use crate::fileops::{Rename, RenameRule, RenameSource, rename_plan};
use eframe::egui;
pub struct RenameDialog {
    pub open: bool,
    items: Vec<RenameSource>,
    name: String,
    find: String,
    replace: String,
    replace_mode: bool,
    keep_extension: bool,
    start: u32,
}
impl RenameDialog {
    pub fn new(items: Vec<RenameSource>) -> Self {
        let name = if items.len() == 1 {
            items[0]
                .path
                .file_name()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
        } else {
            String::new()
        };
        Self {
            open: true,
            items,
            name,
            find: String::new(),
            replace: String::new(),
            replace_mode: false,
            keep_extension: true,
            start: 1,
        }
    }
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Vec<Rename>> {
        let mut open = self.open;
        let mut accepted = None;
        egui::Window::new(crate::i18n::text("이름 변경", "Rename"))
            .id(egui::Id::new("rename-dialog"))
            .open(&mut open)
            .default_width(620.)
            .show(ctx, |ui| {
                ui.label(crate::trf!(
                    "이 창을 열 때 선택한 {}개 항목",
                    "{} items selected when this window opened",
                    self.items.len()
                ));
                if self.items.len() > 1 {
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.replace_mode,
                            false,
                            crate::i18n::text("기본 이름 + 번호", "Base name + number"),
                        );
                        ui.selectable_value(
                            &mut self.replace_mode,
                            true,
                            crate::i18n::text("찾아 바꾸기", "Find and replace"),
                        );
                    });
                }
                if self.replace_mode {
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::text("찾기", "Find"));
                        ui.text_edit_singleline(&mut self.find);
                    });
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::text("바꾸기", "Replace"));
                        ui.text_edit_singleline(&mut self.replace);
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label(if self.items.len() == 1 {
                            crate::i18n::text("새 이름", "New name")
                        } else {
                            crate::i18n::text("기본 이름", "Base name")
                        });
                        ui.text_edit_singleline(&mut self.name);
                    });
                    if self.items.len() > 1 {
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::text("시작 번호", "Start number"));
                            ui.add(egui::DragValue::new(&mut self.start).range(0..=u32::MAX));
                            ui.checkbox(
                                &mut self.keep_extension,
                                crate::i18n::text("확장자 유지", "Keep extensions"),
                            );
                        });
                    }
                }
                let rule = if self.replace_mode {
                    RenameRule::Replace {
                        from: &self.find,
                        to: &self.replace,
                    }
                } else if self.items.len() == 1 {
                    RenameRule::Single(&self.name)
                } else {
                    RenameRule::Numbered {
                        base: &self.name,
                        start: self.start,
                        keep_extension: self.keep_extension,
                    }
                };
                let plan = rename_plan(&self.items, rule);
                ui.separator();
                ui.label(crate::i18n::text("변경 미리보기", "Preview changes"));
                if let Ok(plan) = &plan {
                    egui::ScrollArea::vertical().max_height(280.).show_rows(
                        ui,
                        22.,
                        plan.len(),
                        |ui, range| {
                            for n in range {
                                let e = &plan[n];
                                ui.horizontal(|ui| {
                                    ui.label(
                                        e.source.file_name().unwrap_or_default().to_string_lossy(),
                                    );
                                    ui.label("→");
                                    ui.label(&e.name);
                                });
                            }
                        },
                    );
                } else if let Err(e) = &plan {
                    ui.label(format!("{e:#}"));
                }
                if ui
                    .add_enabled(
                        plan.is_ok(),
                        egui::Button::new(crate::i18n::text("이름 변경 적용", "Apply rename")),
                    )
                    .clicked()
                {
                    accepted = plan.ok();
                }
            });
        self.open = open && accepted.is_none();
        accepted
    }
}
