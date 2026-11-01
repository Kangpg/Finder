//! Volatile editor for shared NTFS settings. Loading preferences never applies these values.
use crate::{
    byte_size::{self, Unit},
    journal::{self, Limits, Snapshot},
    services::Job,
};
use eframe::egui::{self, RichText};
struct SizeInput {
    text: String,
    unit: Unit,
}
impl SizeInput {
    fn new(bytes: u64) -> Self {
        let unit = Unit::best(bytes);
        Self {
            text: unit.exact(bytes),
            unit,
        }
    }
    fn bytes(&self) -> anyhow::Result<u64> {
        self.unit.parse(&self.text)
    }
    fn show(&mut self, ui: &mut egui::Ui, id: &str, label: &str) {
        ui.horizontal(|ui| {
            ui.add_sized(
                [110., 24.],
                egui::Label::new(label).halign(egui::Align::Min),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .id_salt(id)
                    .char_limit(64)
                    .desired_width(130.),
            );
            let old = self.unit;
            egui::ComboBox::from_id_salt((id, "unit"))
                .width(60.)
                .selected_text(self.unit.label())
                .show_ui(ui, |ui| {
                    for unit in Unit::ALL {
                        ui.selectable_value(&mut self.unit, unit, unit.label());
                    }
                });
            if old != self.unit {
                match old.parse(&self.text) {
                    Ok(bytes) => self.text = self.unit.exact(bytes),
                    Err(_) => self.unit = old,
                }
            }
        });
    }
}
pub struct Editor {
    drive: String,
    snapshot: Option<Snapshot>,
    maximum: SizeInput,
    delta: SizeInput,
    pub busy: bool,
    status: crate::i18n::Message,
}
impl Default for Editor {
    fn default() -> Self {
        Self {
            drive: "D:".into(),
            snapshot: None,
            maximum: SizeInput::new(128 << 20),
            delta: SizeInput::new(8 << 20),
            busy: false,
            status: crate::i18n::Message::default(),
        }
    }
}
impl Editor {
    fn drive(&self) -> Option<char> {
        let text = self.drive.trim().trim_end_matches(':');
        let mut chars = text.chars();
        let c = chars.next()?;
        (chars.next().is_none() && c.is_ascii_alphabetic()).then(|| c.to_ascii_uppercase())
    }
    fn limits(&self) -> anyhow::Result<Limits> {
        let limits = Limits {
            maximum: self.maximum.bytes()?,
            delta: self.delta.bytes()?,
        };
        if let Some(s) = &self.snapshot {
            limits.validate(s)?;
        }
        Ok(limits)
    }
    pub fn query_done(&mut self, result: journal::Result<Snapshot>) {
        self.busy = false;
        match result {
            Ok(snapshot) => {
                self.maximum = SizeInput::new(snapshot.limits.maximum);
                self.delta = SizeInput::new(snapshot.limits.delta);
                self.status = crate::i18n::Message::new(
                    "현재 설정을 조회했습니다",
                    "Current settings loaded",
                );
                self.snapshot = Some(snapshot);
            }
            Err(e) => {
                self.snapshot = None;
                self.status = crate::i18n::Message::error(e);
            }
        }
    }
    pub fn apply_done(&mut self, result: journal::Result<journal::Report>) {
        self.busy = false;
        match result {
            Ok(report) => {
                let mut status = match &report.observed {
                    Ok(s) if s.limits == report.requested => crate::i18n::Message::new(
                        "USN 저널 크기 설정을 적용했습니다",
                        "USN journal size settings applied",
                    ),
                    Ok(_) => crate::msgf!(
                        "변경 요청 완료 · {} / {} · NTFS가 크기를 반영할 때까지 기존 값이 보일 수 있습니다",
                        "Change requested · {} / {} · Previous values may remain visible until NTFS applies the sizes",
                        byte_size::format(report.requested.maximum),
                        byte_size::format(report.requested.delta)
                    ),
                    Err(e) => crate::msgf!(
                        "변경 요청 완료 · 재조회 실패: {e}",
                        "Change requested · Could not query updated settings: {e}"
                    ),
                };
                if let Some(path) = report.backup {
                    status.append(crate::msgf!(
                        " · 이전 설정: {}",
                        " · Previous settings: {}",
                        path.display()
                    ));
                }
                self.snapshot = report.observed.ok();
                self.status = status;
            }
            Err(e) => {
                if matches!(
                    e,
                    journal::Error::Changed | journal::Error::Unavailable { .. }
                ) {
                    self.snapshot = None;
                }
                self.status = crate::i18n::Message::error(e);
            }
        }
    }
    pub fn submit_failed(&mut self, message: crate::i18n::Message) {
        self.busy = false;
        self.status = message;
    }
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Job> {
        let mut job = None;
        egui::CollapsingHeader::new(crate::i18n::text("고급 설정", "Advanced settings")).id_salt("advanced-settings").show(ui,|ui|{
            ui.label(RichText::new(crate::i18n::text("NTFS USN 저널", "NTFS USN journal")).strong());
            ui.label(crate::i18n::text("파일 변경 기록을 보관하는 크기입니다. 이 볼륨을 사용하는 다른 프로그램에도 적용됩니다.", "Controls the size of the file change log. This affects other applications using this volume."));
            ui.label(RichText::new(crate::i18n::text("조회·변경에는 관리자 권한이 필요합니다. 1 KB = 1,024 B", "Administrator permission is required to query or change settings. 1 KB = 1,024 B")).small().weak());
            ui.add_enabled_ui(!self.busy,|ui|{
                ui.horizontal(|ui|{
                    ui.label(crate::i18n::text("드라이브", "Drive"));
                    if ui.add(egui::TextEdit::singleline(&mut self.drive).char_limit(2).desired_width(42.)).changed(){self.snapshot=None;self.status.clear();}
                    if ui.add_enabled(self.drive().is_some(),egui::Button::new(crate::i18n::text("현재 설정 조회", "Query current settings"))).clicked(){job=Some(Job::JournalQuery(self.drive().unwrap()));}
                });
            });
            if let Some(snapshot)=&self.snapshot{
                ui.label(crate::trf!("조회값: 최대 {} · 할당 변화폭 {}", "Current: maximum {} · Allocation delta {}",byte_size::format(snapshot.limits.maximum),byte_size::format(snapshot.limits.delta)));
                ui.label(RichText::new(crate::trf!("저널 {:X} · 보존 USN {}–{} · 클러스터 {}", "Journal {:X} · Retained USN {}–{} · Cluster {}",snapshot.journal_id,snapshot.first_usn.max(snapshot.lowest_valid_usn),snapshot.next_usn,byte_size::format(u64::from(snapshot.cluster_bytes)))).small().weak());
            }
            ui.add_enabled_ui(!self.busy&&self.snapshot.is_some(),|ui|{
                self.maximum.show(ui,"journal-maximum",crate::i18n::text("최대 크기", "Maximum size"));
                self.delta.show(ui,"journal-delta",crate::i18n::text("할당 변화폭", "Allocation delta"));
                ui.horizontal(|ui|{
                    if ui.button("128 MB / 8 MB").clicked(){self.maximum=SizeInput::new(128<<20);self.delta=SizeInput::new(8<<20);}
                    let limits=self.limits();
                    let changed=self.snapshot.as_ref().is_some_and(|s|limits.as_ref().is_ok_and(|l|*l!=s.limits));
                    if ui.add_enabled(changed,egui::Button::new(crate::i18n::text("저널 크기 적용", "Apply journal sizes"))).clicked(){job=Some(Job::JournalApply{expected:self.snapshot.clone().unwrap(),limits:limits.unwrap()});}
                });
                if let Err(e)=self.limits(){ui.colored_label(egui::Color32::from_rgb(240,170,110),e.to_string());}
            });
            if self.busy{ui.horizontal(|ui|{ui.spinner();ui.label(crate::i18n::text("USN 설정 처리 중…", "Processing USN settings…"));});}
            if !self.status.is_empty(){ui.label(self.status.as_str());}
            ui.label(RichText::new(crate::i18n::text("변경 전 값은 data/usn-backups에 저장합니다. 저널 크기는 NTFS 정리 시점에 반영될 수 있습니다.", "Previous values are saved in data/usn-backups. NTFS may apply sizes during journal cleanup.")).small().weak());
        });
        if job.is_some() {
            self.busy = true;
        }
        job
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn raw() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900., 700.),
            )),
            ..Default::default()
        }
    }
    fn draw(
        ctx: &egui::Context,
        editor: &mut Editor,
        raw: egui::RawInput,
    ) -> (egui::FullOutput, Option<Job>) {
        let mut job = None;
        let mut out = ctx.run_ui(raw, |ui| {
            job = editor.show(ui);
        });
        out.textures_delta.clear();
        (out, job)
    }
    fn rect(out: &egui::FullOutput, text: &str) -> egui::Rect {
        out.shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == text => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing: {text}"))
    }
    fn click(ctx: &egui::Context, editor: &mut Editor, pos: egui::Pos2) -> Option<Job> {
        let mut result = None;
        for pressed in [true, false] {
            let mut input = raw();
            input.events.push(egui::Event::PointerMoved(pos));
            input.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            let (_, job) = draw(ctx, editor, input);
            if job.is_some() {
                result = job;
            }
        }
        result
    }
    #[test]
    fn advanced_settings_only_produces_an_apply_job_after_explicit_selection() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        let (out, job) = draw(&ctx, &mut editor, raw());
        assert!(job.is_none());
        assert!(click(&ctx, &mut editor, rect(&out, "고급 설정").center()).is_none());
        for _ in 0..20 {
            assert!(draw(&ctx, &mut editor, raw()).1.is_none());
        }
        let snapshot = Snapshot {
            drive: 'D',
            volume: "test volume".into(),
            serial: 3,
            cluster_bytes: 4096,
            volume_bytes: 1 << 40,
            journal_id: 7,
            first_usn: 0,
            next_usn: 100,
            lowest_valid_usn: 0,
            limits: Limits {
                maximum: 32 << 20,
                delta: 8 << 20,
            },
        };
        editor.query_done(Ok(snapshot.clone()));
        let (out, job) = draw(&ctx, &mut editor, raw());
        assert!(job.is_none());
        assert_eq!(editor.limits().unwrap(), snapshot.limits);
        assert!(click(&ctx, &mut editor, rect(&out, "128 MB / 8 MB").center()).is_none());
        let (out, job) = draw(&ctx, &mut editor, raw());
        assert!(job.is_none());
        let job = click(&ctx, &mut editor, rect(&out, "저널 크기 적용").center()).unwrap();
        match job {
            Job::JournalApply { expected, limits } => {
                assert_eq!(expected, snapshot);
                assert_eq!(
                    limits,
                    Limits {
                        maximum: 128 << 20,
                        delta: 8 << 20
                    }
                );
            }
            _ => panic!("unexpected action"),
        }
        assert!(editor.busy);
        assert!(draw(&ctx, &mut editor, raw()).1.is_none());
        editor.apply_done(Err(journal::Error::Changed));
        assert!(editor.snapshot.is_none());
        assert!(!editor.busy);
        assert!(draw(&ctx, &mut editor, raw()).1.is_none());
    }
    #[test]
    #[ignore = "renders the advanced editor meshes with Korean fonts; no native window"]
    fn advanced_editor_preview() {
        let ctx = egui::Context::default();
        crate::appearance::configure(&ctx);
        let mut editor = Editor::default();
        editor.query_done(Ok(Snapshot {
            drive: 'D',
            volume: "preview".into(),
            serial: 1,
            cluster_bytes: 4096,
            volume_bytes: 1 << 40,
            journal_id: 0x1234,
            first_usn: 100,
            next_usn: 200,
            lowest_valid_usn: 0,
            limits: Limits {
                maximum: 32 << 20,
                delta: 8 << 20,
            },
        }));
        let dir = std::path::Path::new("artifacts/ui");
        std::fs::create_dir_all(dir).unwrap();
        let mut renderer = crate::ui_preview::Renderer::default();
        let mut point = egui::Pos2::ZERO;
        for frame in 0..6 {
            let mut input = raw();
            input.time = Some(frame as f64 * 0.25);
            if frame == 1 || frame == 2 {
                input.events.push(egui::Event::PointerMoved(point));
                input.events.push(egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: frame == 1,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let out = ctx.run_ui(input, |ui| {
                ui.set_max_width(760.);
                assert!(editor.show(ui).is_none());
            });
            if frame == 0 {
                point = rect(&out, "고급 설정").center();
            }
            renderer.save(&ctx, out, [900, 700], &dir.join("advanced-settings.rgba"));
        }
    }
}
