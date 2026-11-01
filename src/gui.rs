use crate::background::{IndexCommand, IndexOutcome, Pane, Row, Runtime};
use crate::{
    details::Details,
    preferences::{Bookmark, Preferences, SearchSettings, Store},
    services::{Job, Kind, Outcome, Pending, Services},
};
use crate::{
    rename_dialog::RenameDialog,
    search_pane::{Action, PaneContext, SearchPane},
};
use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) const RESULT_ROW_HEIGHT: f32 = 28.;
const ICON_GUTTER: f32 = 34.;
struct ResultColumns {
    name: egui::Rect,
    path: egui::Rect,
    size: egui::Rect,
    extension: Option<egui::Rect>,
    modified: Option<egui::Rect>,
}
impl ResultColumns {
    fn width(available: f32, details: bool) -> f32 {
        available.max(if details { 820. } else { 460. })
    }
    fn new(rect: egui::Rect, details: bool) -> Self {
        let cell = |left, right| {
            egui::Rect::from_min_max(
                egui::pos2(left, rect.top()),
                egui::pos2(right, rect.bottom()),
            )
        };
        let mut right = rect.right() - 10.;
        let modified = details.then(|| {
            let c = cell(right - 154., right);
            right -= 166.;
            c
        });
        let extension = details.then(|| {
            let c = cell(right - 62., right);
            right -= 74.;
            c
        });
        let size = cell(right - 86., right);
        right -= 100.;
        let name_end = rect.left() + (rect.width() * 0.39).clamp(170., 420.);
        Self {
            name: cell(rect.left() + ICON_GUTTER, name_end - 12.),
            path: cell(name_end, right),
            size,
            extension,
            modified,
        }
    }
}
fn cell_text(ui: &egui::Ui, rect: egui::Rect, text: impl Into<String>, color: Color32) {
    let mut job = egui::text::LayoutJob::simple(
        text.into(),
        egui::TextStyle::Body.resolve(ui.style()),
        color,
        rect.width(),
    );
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    let x = rect.left();
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(rect))
        .galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.),
            galley,
            color,
        );
}
fn entry_icon(painter: &egui::Painter, rect: egui::Rect, directory: bool) {
    let p = |x: f32, y: f32| rect.min + egui::vec2(x, y);
    if directory {
        let back = Color32::from_rgb(188, 139, 45);
        painter.rect_filled(egui::Rect::from_min_max(p(1., 3.), p(8., 8.)), 2., back);
        painter.rect_filled(egui::Rect::from_min_max(p(1., 5.), p(17., 15.)), 2., back);
        painter.rect_filled(
            egui::Rect::from_min_max(p(1., 7.), p(17., 16.)),
            2.,
            Color32::from_rgb(237, 188, 76),
        );
    } else {
        painter.add(egui::Shape::convex_polygon(
            vec![p(3., 1.), p(10., 1.), p(15., 6.), p(15., 17.), p(3., 17.)],
            Color32::from_rgb(174, 205, 231),
            egui::Stroke::new(1., Color32::from_rgb(91, 129, 161)),
        ));
        painter.add(egui::Shape::convex_polygon(
            vec![p(10., 1.), p(10., 6.), p(15., 6.)],
            Color32::from_rgb(110, 158, 195),
            egui::Stroke::NONE,
        ));
        for (y, end) in [(10., 13.), (13., 11.)] {
            painter.line_segment(
                [p(5., y), p(end, y)],
                egui::Stroke::new(1., Color32::from_rgb(91, 129, 161)),
            );
        }
    }
}

pub(super) fn result_row(
    ui: &mut egui::Ui,
    row: &Row,
    selected: bool,
    details: bool,
    decoration: Option<&crate::row_metadata::Decoration>,
    native_icon: Option<&egui::TextureHandle>,
) -> (egui::Response, egui::Rect) {
    let width = ResultColumns::width(ui.available_width(), details);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, RESULT_ROW_HEIGHT),
        egui::Sense::click_and_drag(),
    );
    let columns = ResultColumns::new(rect, details);
    let name_width = ui
        .painter()
        .layout_no_wrap(
            row.name.clone(),
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().text_color(),
        )
        .size()
        .x;
    let name_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(
            (columns.name.left() + name_width + 6.).min(columns.name.right()),
            rect.bottom(),
        ),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            format!(
                "{}: {}",
                if row.directory {
                    crate::i18n::text("폴더", "Folders")
                } else {
                    crate::i18n::text("파일", "Files")
                },
                row.name
            ),
        )
    });
    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter()
                .rect_filled(rect, 3., ui.visuals().selection.bg_fill);
            ui.painter().rect_filled(
                egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 3., rect.bottom())),
                2.,
                crate::appearance::ACCENT,
            );
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, 3., Color32::from_rgb(31, 39, 52));
        }
        let icon = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 17., rect.center().y),
            egui::vec2(22., 22.),
        );
        if let Some(texture) = native_icon {
            ui.painter().image(
                texture.id(),
                icon,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                Color32::WHITE,
            );
        } else {
            entry_icon(ui.painter(), icon, row.directory);
        }
        let foreground = if selected {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().text_color()
        };
        cell_text(ui, columns.name, &row.name, foreground);
        let path = row.path.as_ref().and_then(|p| p.parent()).map_or_else(
            || crate::i18n::text("경로 미확인", "Path unavailable").into(),
            |p| dunce::simplified(p).to_string_lossy(),
        );
        cell_text(
            ui,
            columns.path,
            path,
            if selected {
                foreground
            } else {
                crate::appearance::MUTED
            },
        );
        let size = decoration.map_or_else(
            || {
                row.details
                    .as_ref()
                    .filter(|_| !row.directory)
                    .map_or_else(|| "…".into(), Details::size_text)
            },
            |d| {
                d.size.as_ref().map_or_else(
                    || {
                        if d.size_note.is_some() {
                            "—".into()
                        } else {
                            "…".into()
                        }
                    },
                    crate::row_metadata::Size::text,
                )
            },
        );
        cell_text(ui, columns.size, size, foreground);
        if let Some(cell) = columns.extension {
            let ext = if row.directory {
                crate::i18n::text("폴더", "Folders").into()
            } else {
                row.name
                    .rsplit_once('.')
                    .map_or_else(|| "—".into(), |(_, e)| e.to_uppercase())
            };
            cell_text(ui, cell, ext, crate::appearance::MUTED);
        }
        if let Some(cell) = columns.modified {
            cell_text(
                ui,
                cell,
                decoration
                    .map(|d| &d.details)
                    .or(row.details.as_ref())
                    .map_or_else(|| "…".into(), Details::date_text),
                crate::appearance::MUTED,
            );
        }
    }
    let response = response.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(&row.name).strong());
        if let Some(path) = &row.path {
            ui.label(dunce::simplified(path).to_string_lossy());
        }
        if let Some(d) = decoration {
            if let Some(note) = &d.icon_note {
                ui.label(crate::trf!(
                    "파일 아이콘 조회 실패: {note}",
                    "Could not load file icon: {note}"
                ));
            }
            ui.label(
                egui::RichText::new(crate::trf!(
                    "{}초 전 조회 · F5로 다시 확인",
                    "Checked {} s ago · Press F5 to check again",
                    d.measured.elapsed().as_secs()
                ))
                .small()
                .weak(),
            );
            if let Some(size) = &d.size {
                ui.label(format!(
                    "{} B · {}",
                    size.bytes,
                    if size.complete {
                        crate::i18n::text("계산 완료", "Calculated")
                    } else {
                        crate::i18n::text("일부 합계", "Partial total")
                    }
                ));
                if let Some(note) = &size.note {
                    ui.label(note.as_str());
                }
            }
            if let Some(note) = &d.size_note {
                ui.label(note.as_str());
            }
            if row.directory {
                ui.label(crate::i18n::text(
                    "하위 일반 파일 합계 · 링크·junction 제외",
                    "Total regular file size · Excludes links and junctions",
                ));
            }
        }
        if let Some(error) = decoration
            .map(|d| &d.details)
            .or(row.details.as_ref())
            .and_then(|d| d.error.as_ref())
        {
            ui.label(crate::trf!(
                "상세정보 읽기 실패: {error}",
                "Could not read details: {error}"
            ));
        }
    });
    (response, name_rect)
}
pub(super) fn result_header(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    viewport_width: f32,
    offset_x: f32,
    details: bool,
    order: crate::index::SearchOrder,
) -> Option<crate::index::SortColumn> {
    use crate::index::SortColumn;
    let content = egui::Rect::from_min_size(
        rect.min - egui::vec2(offset_x, 0.),
        egui::vec2(ResultColumns::width(viewport_width, details), rect.height()),
    );
    let columns = ResultColumns::new(content, details);
    ui.painter()
        .rect_filled(rect, 0., crate::appearance::SURFACE);
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        egui::Stroke::new(1., crate::appearance::LINE),
    );
    let mut clicked = None;
    for (column, cell) in [
        (SortColumn::Name, Some(columns.name)),
        (SortColumn::Path, Some(columns.path)),
        (SortColumn::Size, Some(columns.size)),
        (SortColumn::Type, columns.extension),
        (SortColumn::Modified, columns.modified),
    ] {
        let Some(cell) = cell else {
            continue;
        };
        let target = cell.intersect(rect);
        let active = order.column == column;
        let text = if active {
            order.label()
        } else {
            column.label().into()
        };
        if target.is_positive() {
            let response = ui.interact(
                target,
                ui.id().with(("sort-column", column as u8)),
                egui::Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &text)
            });
            if response.hovered() {
                ui.painter()
                    .rect_filled(target, 3., Color32::from_rgb(37, 45, 59));
            }
            let tip = if column == SortColumn::Size {
                crate::i18n::text(
                    "파일 크기 기준으로 전체 검색 결과 정렬 · 폴더 및 조회 불가 항목은 마지막",
                    "Sort all results by file size · Folders and unavailable sizes appear last",
                )
            } else {
                crate::i18n::text(
                    "전체 검색 결과 정렬 · 다시 누르면 방향 전환",
                    "Sort all results · Click again to reverse",
                )
            };
            if response.on_hover_text(tip).clicked() {
                clicked = Some(column);
            }
        }
        cell_text(
            ui,
            cell,
            text,
            if active {
                crate::appearance::ACCENT
            } else {
                crate::appearance::MUTED
            },
        );
    }
    clicked
}

fn startup_source(
    folder: Option<PathBuf>,
    drive: Option<char>,
    cache: Option<PathBuf>,
    prefs: &Preferences,
) -> Result<IndexCommand> {
    if let Some(path) = cache {
        return Ok(IndexCommand::Load(path));
    }
    if let Some(drive) = drive {
        return Ok(IndexCommand::Drive(drive));
    }
    if let Some(root) = folder.or_else(|| prefs.last_folder.clone()) {
        return Ok(IndexCommand::Folder(root));
    }
    Ok(IndexCommand::Folder(std::env::current_dir()?))
}
struct FileDrag {
    id: u64,
    origin: Pane,
    paths: Arc<[PathBuf]>,
    #[cfg(windows)]
    prepared: Option<crate::shell_path::Selection>,
}
#[cfg(windows)]
struct ShellRequest {
    id: u64,
    paths: Arc<[PathBuf]>,
    items: Vec<crate::fileops::RenameSource>,
    position: (i32, i32),
    extended: bool,
    properties: bool,
    prepared: Option<crate::shell_path::Selection>,
}
struct IndexSession {
    active_source: Option<crate::index::Source>,
    runtime: Runtime,
    folder: String,
    drive: String,
    cache: String,
    index_status: crate::i18n::Message,
    index_busy: bool,
    ready: bool,
    index_id: u64,
    revision: u64,
    refresh_after_index: bool,
    live: bool,
    last_refresh: Instant,
}
impl IndexSession {
    fn new(runtime: Runtime) -> Self {
        Self {
            runtime,
            active_source: None,
            folder: String::new(),
            drive: "D".into(),
            cache: "index.reaidx".into(),
            index_status: crate::i18n::Message::new(
                "폴더 또는 NTFS 드라이브를 선택하세요",
                "Select a folder or NTFS drive",
            ),
            index_busy: false,
            ready: false,
            index_id: 0,
            revision: 0,
            refresh_after_index: false,
            live: false,
            last_refresh: Instant::now(),
        }
    }
    fn location(&self) -> String {
        match &self.active_source {
            Some(crate::index::Source::Folder { root }) => {
                dunce::simplified(root).to_string_lossy().into_owned()
            }
            Some(crate::index::Source::Ntfs { drive, .. }) => format!("{drive}:\\"),
            None => self.folder.clone(),
        }
    }
}

struct App {
    indexes: [IndexSession; 2],
    index_target: Pane,
    metadata_epoch: [u64; 2],
    drag: Option<FileDrag>,
    drag_epoch: Arc<std::sync::atomic::AtomicU64>,
    window: Option<isize>,
    reset_pointer: bool,
    settings_tab: bool,
    journal_settings: crate::journal_settings::Editor,
    quitting: bool,
    hotkey_draft: crate::hotkey::Hotkey,
    hotkey_enabled: bool,
    applying_hotkey: bool,
    #[cfg(windows)]
    resident: Option<crate::resident::Controller>,
    index_dialog: bool,
    metadata: crate::row_metadata::Runtime,
    panes: [SearchPane; 2],
    focused: Pane,
    prefs: Preferences,
    services: Services,
    pending: Vec<Pending>,
    settings_enabled: bool,
    prefs_dirty: Option<Instant>,
    action_status: crate::i18n::Message,
    bookmark_dialog: bool,
    bookmark_name: String,
    bookmark_source: Option<(String, SearchSettings)>,
    help: bool,
    #[cfg(windows)]
    shell: Option<ShellRequest>,
    #[cfg(windows)]
    shell_epoch: Arc<std::sync::atomic::AtomicU64>,
    rename_dialog: Option<RenameDialog>,
}
impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        folder: Option<PathBuf>,
        drive: Option<char>,
        cache: Option<PathBuf>,
        workers: usize,
    ) -> Result<Self> {
        let ctx = &cc.egui_ctx;
        crate::appearance::configure(ctx);
        let notify = ctx.clone();
        let settings_path = crate::app_paths::data_dir()?.join("preferences.json");
        let (store, prefs, settings_status) = match Store::load(settings_path) {
            Ok((store, prefs)) => (Some(store), prefs, crate::i18n::Message::default()),
            Err(e) => (
                None,
                Preferences::default(),
                crate::msgf!(
                    "설정 읽기 실패: {e:#} · 저장 비활성화",
                    "Could not read settings: {e:#} · Saving disabled"
                ),
            ),
        };
        crate::i18n::set_language(prefs.language);
        let startup = startup_source(folder, drive, cache, &prefs)?;
        let initial_folder = match &startup {
            IndexCommand::Folder(root) => root.to_string_lossy().into_owned(),
            _ => prefs
                .last_folder
                .as_ref()
                .map_or_else(String::new, |p| p.to_string_lossy().into_owned()),
        };
        let initial_drive = match &startup {
            IndexCommand::Drive(drive) => *drive,
            _ => 'D',
        };
        let initial_cache = match &startup {
            IndexCommand::Load(path) => path.to_string_lossy().into_owned(),
            _ => "index.reaidx".into(),
        };
        let settings_enabled = store.is_some();
        let service_notify = ctx.clone();
        let metadata_notify = ctx.clone();

        let panes = [
            SearchPane::new(prefs.primary_settings()),
            SearchPane::new(prefs.secondary),
        ];
        let left = Runtime::new(Arc::new(move || notify.request_repaint()), workers)?;
        let right = left.independent()?;
        let mut indexes = [IndexSession::new(left), IndexSession::new(right)];
        indexes[0].folder = initial_folder;
        indexes[0].drive = initial_drive.to_string();
        indexes[0].cache = initial_cache;
        indexes[1].folder = prefs
            .right_folder
            .as_ref()
            .or(prefs.last_folder.as_ref())
            .map_or_else(
                || indexes[0].folder.clone(),
                |p| p.to_string_lossy().into_owned(),
            );
        let mut app = Self {
            indexes,
            index_target: Pane::Left,
            metadata_epoch: [0, 0],

            drag: None,
            drag_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            window: None,
            reset_pointer: false,
            settings_tab: false,
            journal_settings: Default::default(),
            quitting: false,
            hotkey_draft: prefs.hotkey.unwrap_or_default(),
            hotkey_enabled: prefs.hotkey.is_some(),
            applying_hotkey: false,
            #[cfg(windows)]
            resident: None,
            index_dialog: false,
            metadata: crate::row_metadata::Runtime::new(Arc::new(move || {
                metadata_notify.request_repaint()
            }))?,
            panes,
            focused: Pane::Left,
            prefs,
            services: Services::new(store, Arc::new(move || service_notify.request_repaint()))?,
            pending: Vec::new(),
            settings_enabled,
            prefs_dirty: None,
            action_status: settings_status,
            bookmark_dialog: false,
            bookmark_name: String::new(),
            bookmark_source: None,
            help: false,
            #[cfg(windows)]
            shell: None,
            #[cfg(windows)]
            shell_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            rename_dialog: None,
        };
        let status = match &startup {
            IndexCommand::Load(_) => {
                crate::i18n::Message::new("저장된 색인을 읽는 중...", "Loading saved index...")
            }
            IndexCommand::Drive(_) => {
                app.indexes[0].live = true;
                crate::i18n::Message::new("NTFS MFT를 읽는 중...", "Reading NTFS MFT...")
            }
            _ => crate::i18n::Message::new("폴더를 색인하는 중...", "Indexing folder..."),
        };
        app.request_index(startup, status);
        if app.prefs.split && !app.prefs.sync_indexes {
            app.ensure_right_index();
        }
        Ok(app)
    }
    fn index_slot(&self, pane: Pane) -> usize {
        if self.prefs.sync_indexes {
            0
        } else {
            pane.slot()
        }
    }
    fn active_indexes(&self) -> &'static [Pane] {
        if self.prefs.split && !self.prefs.sync_indexes {
            &Pane::ALL
        } else {
            &[Pane::Left]
        }
    }
    fn request_index(&mut self, operation: IndexCommand, status: crate::i18n::Message) {
        self.request_index_in(self.focused, operation, status);
    }
    fn request_index_in(
        &mut self,
        pane: Pane,
        operation: IndexCommand,
        status: crate::i18n::Message,
    ) {
        let slot = self.index_slot(pane);
        let index = &mut self.indexes[slot];
        index.index_id = index.runtime.index(operation);
        index.index_busy = true;
        index.index_status = status;
        if index.ready {
            index.index_status.append(crate::i18n::Message::new(
                " · 기존 색인으로 검색 가능",
                " · Existing index remains searchable",
            ));
        }
    }
    fn ensure_right_index(&mut self) {
        if self.prefs.sync_indexes || !self.prefs.split {
            return;
        }
        if self.indexes[1].ready {
            self.request_search(Pane::Right, false);
            if !self.indexes[1].index_busy {
                self.request_index_in(
                    Pane::Right,
                    IndexCommand::Refresh,
                    crate::i18n::Message::new(
                        "오른쪽 색인을 갱신하는 중...",
                        "Refreshing right index...",
                    ),
                );
            }
        } else if !self.indexes[1].index_busy {
            let root = if self.indexes[1].folder.is_empty() {
                self.indexes[0].folder.clone()
            } else {
                self.indexes[1].folder.clone()
            };
            if !root.is_empty() {
                self.request_index_in(
                    Pane::Right,
                    IndexCommand::Folder(root.into()),
                    crate::i18n::Message::new(
                        "오른쪽 폴더를 색인하는 중...",
                        "Indexing right folder...",
                    ),
                );
            }
        }
    }
    fn set_index_sync(&mut self, shared: bool) {
        if self.prefs.sync_indexes == shared {
            return;
        }
        self.prefs.sync_indexes = shared;
        self.dirty();
        self.metadata_epoch[1] = self.metadata_epoch[1].wrapping_add(1);
        self.panes[1].view = None;
        self.panes[1].selection.clear();
        self.panes[1].search_busy = false;
        if shared {
            if self.indexes[0].ready && self.prefs.split {
                self.request_search(Pane::Right, false);
            }
        } else {
            self.ensure_right_index();
        }
    }
    fn dirty(&mut self) {
        self.prefs_dirty = Some(Instant::now());
    }
    fn request_search(&mut self, pane: Pane, preserve: bool) {
        let slot = self.index_slot(pane);
        let state = &mut self.panes[pane.slot()];
        let request = state.request(preserve);
        state.search_id = self.indexes[slot].runtime.search_in(pane, request);
    }
    fn submit(&mut self, job: Job) {
        if job.kind().file_work() && self.pending.iter().any(|p| p.kind.file_work()) {
            self.action_status = crate::i18n::Message::new(
                "파일 작업이 진행 중입니다",
                "A file operation is in progress",
            );
            return;
        }
        if self.pending.len() >= 16 {
            self.action_status = crate::i18n::Message::new(
                "작업이 많습니다. 잠시 후 다시 시도하세요",
                "Too many pending tasks. Try again shortly",
            );
            return;
        }
        match self.services.submit(job) {
            Ok(p) => {
                self.pending.push(p);
            }
            Err(e) => self.action_status = crate::i18n::Message::capture(|| format!("{e:#}")),
        }
    }
    fn refresh_after_files(&mut self) {
        for pane in self.active_indexes() {
            let slot = self.index_slot(*pane);
            if self.indexes[slot].ready {
                if self.indexes[slot].index_busy {
                    self.indexes[slot].refresh_after_index = true;
                } else {
                    self.request_index_in(
                        *pane,
                        IndexCommand::Refresh,
                        crate::i18n::Message::new(
                            "파일 작업 결과를 반영하는 중...",
                            "Updating results after file operations...",
                        ),
                    );
                }
            }
        }
    }
    fn poll(&mut self) {
        let mut n = 0;
        while n < self.pending.len() {
            match self.pending[n].receiver.try_recv() {
                Ok(result) => {
                    let pending = self.pending.swap_remove(n);
                    let mutation = pending.kind == Kind::Mutation;
                    match result {
                        Ok(Outcome::JournalQuery(result)) => {
                            self.journal_settings.query_done(result)
                        }
                        Ok(Outcome::JournalApplied(result)) => {
                            self.journal_settings.apply_done(result)
                        }
                        #[cfg(windows)]
                        Ok(Outcome::DragPrepared { id, result }) => {
                            if let Some(drag) = &mut self.drag
                                && drag.id == id
                            {
                                match result {
                                    Ok(prepared) => drag.prepared = Some(prepared),
                                    Err(e) => {
                                        self.action_status = crate::msgf!(
                                            "드래그 준비 실패: {e:#}",
                                            "Could not prepare drag: {e:#}"
                                        );
                                        self.cancel_file_drag();
                                    }
                                }
                            }
                        }
                        #[cfg(windows)]
                        Ok(Outcome::MenuPrepared { id, result }) => {
                            if self.shell.as_ref().is_some_and(|s| s.id == id) {
                                match result {
                                    Ok(prepared) => {
                                        self.shell.as_mut().unwrap().prepared = Some(prepared)
                                    }
                                    Err(e) => {
                                        self.shell = None;
                                        self.action_status = crate::msgf!(
                                            "Windows 메뉴 준비 실패: {e:#}",
                                            "Could not prepare Windows menu: {e:#}"
                                        );
                                    }
                                }
                            }
                        }
                        Ok(Outcome::Done) => {
                            self.action_status = crate::i18n::Message::new("완료", "Done")
                        }
                        Ok(Outcome::Saved) => {}
                        Ok(Outcome::Copied { count, cut }) => {
                            self.action_status = crate::msgf!(
                                "{count}개 {} · 대상 폴더에서 Ctrl+V",
                                "{count} items {} · Press Ctrl+V in the destination folder",
                                if cut {
                                    crate::i18n::text("잘라내기", "Cut")
                                } else {
                                    crate::i18n::text("복사", "Copy")
                                }
                            )
                        }
                        Ok(Outcome::PasteReady {
                            clipboard,
                            destination,
                        }) => self.submit(Job::Transfer {
                            paths: clipboard.paths,
                            cut: clipboard.cut,
                            clipboard_sequence: Some(clipboard.sequence),
                            destination,
                        }),
                        Ok(Outcome::Files(report)) => {
                            self.action_status = if report.complete() {
                                crate::i18n::Message::new(
                                    "파일 작업 완료",
                                    "File operation complete",
                                )
                            } else {
                                crate::i18n::Message::new(
                                    "파일 작업이 중단되었거나 일부만 처리됐습니다",
                                    "The file operation was canceled or only partially completed",
                                )
                            };
                            if let Some(error) = report.error {
                                self.action_status.append(crate::i18n::Message::capture(|| {
                                    format!(" · {error}")
                                }));
                            }
                            if let Some(warning) = report.warning {
                                self.action_status.append(crate::i18n::Message::capture(|| {
                                    format!(" · {warning}")
                                }));
                            }
                        }
                        Err(e) => {
                            self.action_status =
                                crate::msgf!("작업 실패: {e:#}", "Operation failed: {e:#}");
                            if pending.kind == Kind::Settings {
                                self.settings_enabled = false;
                            }
                        }
                    }
                    if pending.kind == Kind::Journal && self.journal_settings.busy {
                        self.journal_settings
                            .submit_failed(self.action_status.clone());
                    }
                    if mutation {
                        self.refresh_after_files();
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => n += 1,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let pending = self.pending.swap_remove(n);
                    if pending.kind == Kind::Mutation {
                        self.refresh_after_files();
                    }
                    self.action_status = crate::i18n::Message::new(
                        "작업 스레드 연결 종료",
                        "Worker connection closed",
                    );
                    if pending.kind == Kind::Journal {
                        self.journal_settings
                            .submit_failed(self.action_status.clone());
                    }
                }
            }
        }
        if self.settings_enabled
            && self
                .prefs_dirty
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(600))
            && self.pending.len() < 16
            && !self.pending.iter().any(|p| p.kind == Kind::Settings)
        {
            match self.services.submit(Job::Save(self.prefs.clone())) {
                Ok(p) => {
                    self.pending.push(p);
                    self.prefs_dirty = None;
                }
                Err(e) => {
                    self.action_status =
                        crate::msgf!("설정 저장 대기: {e:#}", "Settings save pending: {e:#}")
                }
            }
        }
        for slot in 0..2 {
            if let Some(event) = self.indexes[slot].runtime.take_index_event()
                && event.id == self.indexes[slot].index_id
            {
                self.indexes[slot].index_busy = false;
                self.indexes[slot].last_refresh = Instant::now();
                match event.result {
                    Ok(IndexOutcome::Published { revision, source }) => {
                        if let crate::index::Source::Folder { root } = &source {
                            let saved = if slot == 0 {
                                &mut self.prefs.last_folder
                            } else {
                                &mut self.prefs.right_folder
                            };
                            if saved.as_ref() != Some(root) {
                                *saved = Some(root.clone());
                                self.dirty();
                            }
                        }
                        self.indexes[slot].active_source = Some(source);
                        self.indexes[slot].revision = revision;
                        self.indexes[slot].ready = true;
                        self.indexes[slot].index_status = crate::msgf!(
                            "색인 갱신 완료 · {:.0} ms",
                            "Index refreshed · {:.0} ms",
                            event.elapsed_ms
                        );
                        for pane in Pane::ALL {
                            if self.index_slot(pane) == slot {
                                self.metadata_epoch[pane.slot()] =
                                    self.metadata_epoch[pane.slot()].wrapping_add(1);
                                if pane == Pane::Left || self.prefs.split {
                                    self.request_search(pane, true);
                                }
                            }
                        }
                    }
                    Ok(IndexOutcome::Unchanged) => {
                        self.indexes[slot].index_status = crate::i18n::Message::new(
                            "갱신 완료 · 변경 없음",
                            "Refreshed · No changes",
                        )
                    }
                    Ok(IndexOutcome::Saved(path)) => {
                        self.indexes[slot].index_status =
                            crate::msgf!("저장 완료: {}", "Saved: {}", path.display())
                    }
                    Err(e) => {
                        self.indexes[slot].index_status = crate::msgf!(
                            "색인 작업 실패: {e:#}{}",
                            "Index operation failed: {e:#}{}",
                            if self.indexes[slot].ready {
                                crate::i18n::text(" · 기존 색인 유지", " · Existing index retained")
                            } else {
                                ""
                            }
                        )
                    }
                }
            }
            if self.indexes[slot].refresh_after_index
                && !self.indexes[slot].index_busy
                && self.indexes[slot].ready
            {
                self.indexes[slot].refresh_after_index = false;
                let pane = Pane::ALL[slot];
                if self.index_slot(pane) == slot {
                    self.request_index_in(
                        pane,
                        IndexCommand::Refresh,
                        crate::i18n::Message::new(
                            "파일 작업 결과를 반영하는 중...",
                            "Updating results after file operations...",
                        ),
                    );
                }
            }
        }
        for pane in Pane::ALL {
            let slot = self.index_slot(pane);
            if let Some(event) = self.indexes[slot].runtime.take_search_for(pane)
                && event.id == self.panes[pane.slot()].search_id
                && event.revision >= self.indexes[slot].revision
            {
                self.panes[pane.slot()].apply(event.result);
            }
        }
    }

    fn update_metadata(&mut self, ctx: &egui::Context) {
        for pane in Pane::ALL {
            let state = &mut self.panes[pane.slot()];
            if let Some(batch) = self.metadata.take(pane)
                && batch.id == state.metadata_id
            {
                if batch.done {
                    state.metadata_busy = false;
                }
                #[cfg(windows)]
                for (path, value) in &batch.values {
                    if let Some(icon) = &value.icon {
                        if state
                            .icons
                            .get(path)
                            .is_none_or(|(old, _)| !Arc::ptr_eq(old, icon))
                        {
                            let texture = ctx.load_texture(
                                format!("file-icon-{pane:?}-{}", path.display()),
                                icon.image.clone(),
                                egui::TextureOptions::LINEAR,
                            );
                            state.icons.insert(path.clone(), (icon.clone(), texture));
                        }
                    } else {
                        state.icons.remove(path);
                    }
                }
                state.decorations.extend(batch.values);
                ctx.request_repaint();
            }
            let mut paths: Vec<_> = if pane == Pane::Right && !self.prefs.split {
                Vec::new()
            } else {
                state
                    .row_hits
                    .iter()
                    .filter_map(|hit| state.view.as_ref()?.rows.get(hit.index)?.path.clone())
                    .take(crate::row_metadata::MAX_VISIBLE)
                    .collect()
            };
            paths.sort();
            paths.dedup();
            state.trim_metadata(&paths);
            if paths != state.metadata_paths
                || self.metadata_epoch[pane.slot()] != state.metadata_revision
                || (!paths.is_empty()
                    && !state.metadata_busy
                    && state
                        .metadata_at
                        .is_none_or(|t| t.elapsed() >= crate::row_metadata::FRESH_FOR))
            {
                // Keep the last measured display while its replacement is in flight.
                // Freshness belongs to the worker's TTL/revision check, not texture eviction.
                // Leaving the viewport does not invalidate an icon texture.
                // trim_metadata bounds retained display resources; the worker owns freshness.
                match self
                    .metadata
                    .request(pane, self.metadata_epoch[pane.slot()], paths.clone())
                {
                    Ok(id) => {
                        state.metadata_id = id;
                        state.metadata_busy = true;
                        state.metadata_paths = paths;
                        state.metadata_revision = self.metadata_epoch[pane.slot()];
                        state.metadata_at = Some(Instant::now());
                    }
                    Err(e) => {
                        self.action_status = crate::msgf!(
                            "표시 정보 조회 실패: {e:#}",
                            "Could not load display information: {e:#}"
                        )
                    }
                }
            }
        }
    }
    fn cancel_file_drag(&mut self) {
        self.drag_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.drag = None;
    }
    fn begin_file_drag(&mut self, pane: Pane, paths: Vec<PathBuf>) {
        if self.pending.len() >= 16 || self.pending.iter().any(|p| p.kind.file_work()) {
            self.action_status = crate::i18n::Message::new(
                "진행 중인 파일 작업이 끝난 뒤 드래그하세요",
                "Wait for the file operation to finish before dragging",
            );
            return;
        }
        #[cfg(windows)]
        {
            let id = self
                .drag_epoch
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            let paths: Arc<[PathBuf]> = paths.into();
            match self.services.submit(Job::PrepareDrag {
                paths: paths.clone(),
                epoch: self.drag_epoch.clone(),
                id,
            }) {
                Ok(pending) => {
                    self.pending.push(pending);
                    self.drag = Some(FileDrag {
                        id,
                        origin: pane,
                        paths,
                        prepared: None,
                    });
                    self.focused = pane;
                }
                Err(e) => self.action_status = crate::i18n::Message::capture(|| format!("{e:#}")),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (pane, paths);
            self.action_status = crate::i18n::Message::new(
                "Windows에서 파일 드래그를 사용할 수 있습니다",
                "File dragging is available on Windows",
            );
        }
    }
    fn drag_target(&self, pos: egui::Pos2) -> Option<(Pane, egui::Rect, PathBuf)> {
        for pane in Pane::ALL {
            if pane == Pane::Right && !self.prefs.split {
                continue;
            }
            let state = &self.panes[pane.slot()];
            if !state.list_rect.is_some_and(|rect| rect.contains(pos)) {
                continue;
            }
            let hit = state.row_hits.iter().find(|h| h.rect.contains(pos))?;
            if self.drag.as_ref().is_some_and(|d| d.origin == pane)
                && state.selection.items.contains(&hit.index)
            {
                return None;
            }
            let row = state.view.as_ref()?.rows.get(hit.index)?;
            let path = row.path.as_ref()?;
            let destination = if row.directory {
                path.clone()
            } else {
                path.parent()?.into()
            };
            return Some((pane, hit.rect, destination));
        }
        None
    }
    fn update_file_drag(&mut self, ui: &mut egui::Ui) {
        if self.drag.is_none() {
            return;
        }
        let (held, released, escape, modifiers) = ui.ctx().input(|i| {
            (
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.key_pressed(egui::Key::Escape),
                i.modifiers,
            )
        });
        if escape || !held && !released {
            self.cancel_file_drag();
            return;
        }
        let pos = {
            #[cfg(windows)]
            {
                self.window
                    .and_then(|w| crate::native_drag::cursor(w, ui.ctx().pixels_per_point()))
                    .map(|(x, y)| egui::pos2(x, y))
                    .or_else(|| ui.ctx().input(|i| i.pointer.interact_pos()))
            }
            #[cfg(not(windows))]
            {
                ui.ctx().input(|i| i.pointer.interact_pos())
            }
        };
        let target = pos.and_then(|p| self.drag_target(p));
        if released {
            let drag = self.drag.take().unwrap();
            self.drag_epoch
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some((pane, _, destination)) = target {
                self.focused = pane;
                self.submit(Job::Transfer {
                    paths: drag.paths.to_vec(),
                    destination,
                    cut: modifiers.shift && !modifiers.ctrl,
                    clipboard_sequence: None,
                });
            }
            return;
        }
        #[cfg(windows)]
        if held
            && self.window.is_some()
            && pos.is_some_and(|p| !ui.ctx().viewport_rect().contains(p))
            && self.drag.as_ref().unwrap().prepared.is_some()
        {
            let drag = self.drag.take().unwrap();
            self.drag_epoch
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let result =
                crate::native_drag::run(drag.prepared.as_ref().unwrap(), self.window.unwrap());
            self.reset_pointer = true;
            self.action_status = match result {
                Ok(0) => crate::i18n::Message::new("드래그 취소", "Drag canceled"),
                Ok(_) => crate::i18n::Message::new(
                    "Windows 드래그 작업 전달 완료",
                    "Drag operation handed to Windows",
                ),
                Err(e) => crate::msgf!("드래그 실패: {e:#}", "Drag failed: {e:#}"),
            };
            self.refresh_after_files();
            return;
        }
        if let Some(pos) = pos {
            let count = self.drag.as_ref().unwrap().paths.len();
            let label = target.as_ref().map_or_else(
                || {
                    crate::trf!(
                        "{count}개 · 폴더 위에 놓거나 탐색기로 끌어내세요",
                        "{count} items · Drop on a folder or drag to Explorer"
                    )
                },
                |(_, _, path)| {
                    crate::trf!(
                        "{} {count}개 → {}",
                        "{} {count} items → {}",
                        if modifiers.shift && !modifiers.ctrl {
                            crate::i18n::text("이동", "Move")
                        } else {
                            crate::i18n::text("복사", "Copy")
                        },
                        path.display()
                    )
                },
            );
            if let Some((_, rect, _)) = target {
                ui.ctx()
                    .layer_painter(egui::LayerId::new(
                        egui::Order::Foreground,
                        egui::Id::new("drop-target"),
                    ))
                    .rect_stroke(
                        rect,
                        2.,
                        egui::Stroke::new(2., Color32::from_rgb(100, 180, 240)),
                        egui::StrokeKind::Inside,
                    );
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            egui::Area::new(egui::Id::new("file-drag-hint"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pos + egui::vec2(14., 18.))
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(label);
                    });
                });
        }
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }

    fn begin_shell(
        &mut self,
        ctx: &egui::Context,
        items: Vec<crate::fileops::RenameSource>,
        position: Option<egui::Pos2>,
        extended: bool,
    ) {
        if self.pending.len() >= 16 {
            self.action_status = crate::i18n::Message::new(
                "작업이 많습니다. 잠시 후 다시 시도하세요",
                "Too many pending tasks. Try again shortly",
            );
            return;
        }
        #[cfg(windows)]
        {
            let Some(window) = self.window else {
                self.action_status = crate::i18n::Message::new(
                    "Windows 창이 준비되지 않았습니다",
                    "The Windows window is not ready",
                );
                return;
            };
            let point = position.map_or(Ok((0, 0)), |p| {
                crate::shell_menu::screen_point(window, p.x, p.y, ctx.pixels_per_point())
            });
            let point = match point {
                Ok(p) => p,
                Err(e) => {
                    self.action_status = crate::i18n::Message::capture(|| format!("{e:#}"));
                    return;
                }
            };
            self.cancel_file_drag();
            let id = self
                .shell_epoch
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            let paths: Arc<[PathBuf]> = items
                .iter()
                .map(|x| x.path.clone())
                .collect::<Vec<_>>()
                .into();
            self.shell = Some(ShellRequest {
                id,
                paths: paths.clone(),
                items,
                position: point,
                extended,
                properties: position.is_none(),
                prepared: None,
            });
            match self.services.submit(Job::PrepareMenu {
                paths,
                epoch: self.shell_epoch.clone(),
                id,
            }) {
                Ok(p) => self.pending.push(p),
                Err(e) => {
                    self.shell = None;
                    self.action_status = crate::i18n::Message::capture(|| format!("{e:#}"));
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (ctx, items, position, extended);
            self.action_status = crate::i18n::Message::new(
                "Windows 메뉴는 Windows에서 사용할 수 있습니다",
                "Native menus are available on Windows",
            );
        }
    }
    fn update_shell(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            if self.index_dialog
                || self.help
                || self.bookmark_dialog
                || self.rename_dialog.is_some()
            {
                if self.shell.take().is_some() {
                    self.shell_epoch
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                return;
            }
            if self.shell.is_some()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                self.shell_epoch
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.shell = None;
            }
            if !self.shell.as_ref().is_some_and(|s| s.prepared.is_some()) {
                return;
            }
            let request = self.shell.take().unwrap();
            let Some(window) = self.window else {
                return;
            };
            let selection = request.prepared.as_ref().unwrap();
            let result = if request.properties {
                crate::shell_menu::properties(selection, window)
                    .map(|_| crate::shell_menu::Outcome::Invoked)
            } else {
                crate::shell_menu::show(selection, window, request.position, request.extended)
            };
            self.reset_pointer = true;
            match result {
                Ok(crate::shell_menu::Outcome::Cancelled) => {}
                Ok(crate::shell_menu::Outcome::Invoked) => self.refresh_after_files(),
                Ok(crate::shell_menu::Outcome::Extra(extra)) => match extra {
                    crate::shell_menu::Extra::CopyPaths => ctx.copy_text(
                        request
                            .paths
                            .iter()
                            .map(|p| p.to_string_lossy())
                            .collect::<Vec<_>>()
                            .join("\r\n"),
                    ),
                    crate::shell_menu::Extra::CopyNames => ctx.copy_text(
                        request
                            .paths
                            .iter()
                            .filter_map(|p| p.file_name())
                            .map(|p| p.to_string_lossy())
                            .collect::<Vec<_>>()
                            .join("\r\n"),
                    ),
                    crate::shell_menu::Extra::Rename => {
                        self.rename_dialog = Some(RenameDialog::new(request.items));
                    }
                },
                Err(e) => {
                    self.action_status = crate::msgf!(
                        "Windows 메뉴 작업 실패: {e:#} · Ctrl+우클릭으로 앱 메뉴 사용",
                        "Windows menu failed: {e:#} · Use Ctrl+right-click for the app menu"
                    )
                }
            }
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }
    fn has_modal(&self) -> bool {
        self.index_dialog
            || self.bookmark_dialog
            || self.rename_dialog.is_some()
            || {
                #[cfg(windows)]
                {
                    self.shell.is_some()
                }
                #[cfg(not(windows))]
                {
                    false
                }
            }
            || self.help
    }

    fn auto_refresh_due(&self, pane: Pane, now: Instant) -> bool {
        let index = &self.indexes[self.index_slot(pane)];
        index.live
            && !index.index_busy
            && index.ready
            && now.saturating_duration_since(index.last_refresh)
                >= Duration::from_secs(u64::from(self.prefs.refresh_interval_secs))
    }
    fn refresh_controls(&mut self, ui: &mut egui::Ui, pane: Pane) {
        let slot = self.index_slot(pane);
        let mut changed = false;
        let mut interval_changed = false;
        ui.horizontal(|ui| {
            changed |= ui
                .checkbox(
                    &mut self.indexes[slot].live,
                    crate::i18n::text("자동 갱신", "Auto refresh"),
                )
                .changed();
            interval_changed = ui
                .add(
                    egui::DragValue::new(&mut self.prefs.refresh_interval_secs)
                        .range(1..=u32::MAX)
                        .speed(1.)
                        .suffix(crate::i18n::text("초", "s")),
                )
                .on_hover_text(crate::i18n::text(
                    "갱신 간격(양쪽 공통) · 숫자를 더블클릭해 직접 입력",
                    "Refresh interval (both panes) · Double-click the number to enter a value",
                ))
                .changed();
        });
        if changed {
            self.indexes[slot].last_refresh = Instant::now();
        }
        if interval_changed {
            for index in &mut self.indexes {
                index.last_refresh = Instant::now();
            }
            self.dirty();
        }
    }
    fn show_pane(&mut self, ui: &mut egui::Ui, pane: Pane) {
        let slot = self.index_slot(pane);
        let context = PaneContext {
            pane,
            focused: self.focused == pane,
            split: self.prefs.split,
            ready: self.indexes[slot].ready,
            index_busy: self.indexes[slot].index_busy,
            file_drag: self.drag.is_some(),
            native_shell: cfg!(windows) && self.window.is_some(),
            modal: self.has_modal(),
            index_location: self.indexes[slot].location(),
        };
        let actions = ui
            .push_id(pane, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!context.file_drag, |ui| {
                            self.panes[pane.slot()].show(ui, context, &mut self.prefs)
                        })
                        .inner
                    })
                    .inner
            })
            .inner;
        for action in actions {
            match action {
                Action::Focused => self.focused = pane,
                Action::Drag(paths) => self.begin_file_drag(pane, paths),
                Action::ShellMenu {
                    items,
                    position,
                    extended,
                } => self.begin_shell(ui.ctx(), items, Some(position), extended),
                Action::Properties(items) => self.begin_shell(ui.ctx(), items, None, false),
                Action::Search(request) => {
                    self.panes[pane.slot()].search_id =
                        self.indexes[slot].runtime.search_in(pane, request)
                }
                Action::Settings => {
                    let settings = self.panes[pane.slot()].settings;
                    match pane {
                        Pane::Left => self.prefs.set_primary(settings),
                        Pane::Right => self.prefs.secondary = settings,
                    }
                    self.dirty();
                }
                Action::Job(job) => self.submit(job),
                Action::Rename(items) => {
                    if self.pending.iter().any(|p| p.kind.file_work()) {
                        self.action_status = crate::i18n::Message::new(
                            "파일 작업이 진행 중입니다",
                            "A file operation is in progress",
                        );
                    } else {
                        self.rename_dialog = Some(RenameDialog::new(items));
                    }
                }
                Action::Refresh => self.request_index_in(
                    pane,
                    IndexCommand::Refresh,
                    crate::i18n::Message::new("변경 사항을 반영하는 중...", "Applying changes..."),
                ),
                Action::Help => self.help = true,
                Action::IndexManager => {
                    self.index_target = pane;
                    self.index_dialog = true;
                }
                Action::Bookmarks { query, settings } => {
                    self.bookmark_name = query.chars().take(60).collect();
                    self.bookmark_source = Some((query, settings));
                    self.bookmark_dialog = true;
                }
                Action::Message(message) => self.action_status = message,
            }
        }
    }
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();
        self.poll_resident(ctx);
        self.handle_close(ctx, ctx.input(|i| i.viewport().close_requested()));
        self.tick_background(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        #[cfg(windows)]
        {
            use raw_window_handle::HasWindowHandle;
            if let Ok(handle) = frame.window_handle()
                && let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw()
            {
                self.window = Some(handle.hwnd.get());
                if self.resident.is_none() {
                    let notify = ui.ctx().clone();
                    match crate::resident::Controller::start(
                        handle.hwnd.get(),
                        self.prefs.hotkey,
                        Arc::new(move || notify.request_repaint()),
                    ) {
                        Ok(controller) => self.resident = Some(controller),
                        Err(e) => {
                            self.action_status = crate::msgf!(
                                "백그라운드 제어 시작 실패: {e:#}",
                                "Could not start background controls: {e:#}"
                            )
                        }
                    }
                }
            }
        }
        self.show(ui);
    }
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if self.reset_pointer {
            self.reset_pointer = false;
            input.events.insert(
                0,
                egui::Event::PointerButton {
                    pos: ctx.input(|i| i.pointer.latest_pos()).unwrap_or_default(),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            );
            input.events.insert(
                1,
                egui::Event::PointerButton {
                    pos: ctx.input(|i| i.pointer.latest_pos()).unwrap_or_default(),
                    button: egui::PointerButton::Secondary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            );
            input.events.insert(2, egui::Event::PointerGone);
        }
    }
}
impl App {
    fn tick_background(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        for &pane in self.active_indexes() {
            if self.auto_refresh_due(pane, now) {
                self.request_index_in(
                    pane,
                    IndexCommand::Refresh,
                    crate::i18n::Message::new("변경 사항을 반영하는 중...", "Applying changes..."),
                );
            }
            let index = &self.indexes[self.index_slot(pane)];
            if index.live && index.ready && !index.index_busy {
                let remaining = Duration::from_secs(self.prefs.refresh_interval_secs as u64)
                    .saturating_sub(now.saturating_duration_since(index.last_refresh));
                ctx.request_repaint_after(remaining.max(Duration::from_millis(100)));
            }
        }
        if self.settings_enabled
            && let Some(dirty) = self.prefs_dirty
        {
            ctx.request_repaint_after(
                Duration::from_millis(600)
                    .saturating_sub(now.saturating_duration_since(dirty))
                    .max(Duration::from_millis(100)),
            );
        }
    }
    fn handle_close(&mut self, ctx: &egui::Context, requested: bool) {
        if !requested || self.quitting || !self.prefs.close_to_tray {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        #[cfg(windows)]
        if self.resident.as_ref().is_some_and(|r| r.can_hide()) {
            self.cancel_file_drag();
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        }
        self.action_status = crate::i18n::Message::new(
            "트레이 또는 전역 단축키를 준비하지 못했습니다. 설정에서 확인하거나 완전 종료를 눌러 주세요.",
            "The tray icon or global shortcut is unavailable. Check Settings or use Exit completely.",
        );
    }
    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    fn poll_resident(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            let result = self.resident.as_ref().and_then(|r| r.take_result());
            if let Some(result) = result {
                self.applying_hotkey = false;
                match result {
                    Ok(hotkey) => {
                        self.prefs.hotkey = hotkey;
                        self.dirty();
                        self.action_status = crate::i18n::Message::new(
                            "전역 단축키를 적용했습니다",
                            "Global shortcut applied",
                        );
                    }
                    Err(error) => {
                        self.action_status = error;
                        if self.resident.as_ref().is_some_and(|r| !r.can_hide()) {
                            self.settings_tab = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        }
                    }
                }
            }
            let event = self.resident.as_ref().and_then(|r| r.take_event());
            if let Some(event) = event {
                match event {
                    crate::resident::Event::Quit => self.quit(ctx),
                    event => {
                        self.settings_tab = matches!(event, crate::resident::Event::Settings);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        if !self.settings_tab {
                            ctx.memory_mut(|m| {
                                m.request_focus(egui::Id::new(("search-query", self.focused)))
                            });
                        }
                    }
                }
            }
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }
    fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(18.);
        egui::ScrollArea::vertical().id_salt("settings-page").show(ui,|ui|{
            ui.set_max_width(760.);
            ui.heading(crate::i18n::text("설정", "Settings"));ui.label(RichText::new(crate::i18n::text("창 동작, 전역 단축키와 자동 갱신을 관리합니다.", "Manage window behavior, global shortcuts, and automatic refresh.")).weak());ui.add_space(18.);
            egui::Frame::new().fill(crate::appearance::SURFACE).corner_radius(8).inner_margin(16).show(ui, |ui| {
                ui.set_min_width(620.);
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::text("언어", "Language"));
                    let before = self.prefs.language;
                    egui::ComboBox::from_id_salt("application-language")
                        .selected_text(self.prefs.language.label())
                        .show_ui(ui, |ui| {
                            for language in crate::i18n::Language::ALL {
                                ui.selectable_value(&mut self.prefs.language, language, language.label());
                            }
                        });
                    if before != self.prefs.language { crate::i18n::set_language(self.prefs.language); self.dirty(); ui.ctx().request_repaint(); }
                });

            });
            ui.add_space(12.);
            egui::Frame::new().fill(crate::appearance::SURFACE).corner_radius(8).inner_margin(16).show(ui,|ui|{
                ui.set_min_width(620.);ui.label(RichText::new(crate::i18n::text("백그라운드 실행", "Run in background")).strong());ui.add_space(6.);
                if ui.checkbox(&mut self.prefs.close_to_tray,crate::i18n::text("창을 닫으면 백그라운드에서 계속 실행", "Keep running in the background when the window closes")).changed(){self.dirty();}
                ui.label(RichText::new(crate::i18n::text("트레이 아이콘이나 전역 단축키로 검색 창을 다시 엽니다.", "Reopen search with the tray icon or global shortcut.")).small().weak());
                ui.label(RichText::new(crate::i18n::text("완전 종료는 아래 버튼 또는 트레이 메뉴에서 선택할 수 있습니다.", "Use the button below or the tray menu to exit completely.")).small().weak());
                #[cfg(windows)] if let Some(resident)=&self.resident {let status=resident.status();ui.add_space(6.);ui.label(crate::trf!("트레이: {} · 단축키: {}", "Tray: {} · Shortcut: {}",if status.tray {crate::i18n::text("사용 중", "Active")}else{crate::i18n::text("준비되지 않음", "Not ready")},status.hotkey.map_or_else(||crate::i18n::text("사용 안 함", "Disabled").into(),|h|h.label())));if !status.error.is_empty(){ui.colored_label(Color32::from_rgb(240,170,110),status.error.as_str());}}
            });ui.add_space(12.);
            egui::Frame::new().fill(crate::appearance::SURFACE).corner_radius(8).inner_margin(16).show(ui,|ui|{
                ui.set_min_width(620.);ui.label(RichText::new(crate::i18n::text("창 열기 단축키", "Show window shortcut")).strong());ui.add_space(6.);
                ui.checkbox(&mut self.hotkey_enabled,crate::i18n::text("전역 단축키 사용", "Enable global shortcut"));
                ui.add_enabled_ui(self.hotkey_enabled,|ui|{ui.horizontal(|ui|{
                    ui.checkbox(&mut self.hotkey_draft.ctrl,"Ctrl");ui.checkbox(&mut self.hotkey_draft.alt,"Alt");ui.checkbox(&mut self.hotkey_draft.shift,"Shift");ui.checkbox(&mut self.hotkey_draft.win,"Win");
                    egui::ComboBox::from_id_salt("global-hotkey-key").selected_text(crate::hotkey::Hotkey::keys().find(|(k,_)|*k==self.hotkey_draft.key).map_or_else(||crate::i18n::text("선택", "Select").into(),|(_,n)|n)).show_ui(ui,|ui|{for(key,label)in crate::hotkey::Hotkey::keys(){ui.selectable_value(&mut self.hotkey_draft.key,key,label);}});
                });});
                let valid=!self.hotkey_enabled||self.hotkey_draft.validate().is_ok();
                ui.horizontal(|ui|{if ui.add_enabled(valid&&!self.applying_hotkey,egui::Button::new(crate::i18n::text("단축키 적용", "Apply shortcut"))).clicked(){
                    #[cfg(windows)] if let Some(resident)=&self.resident {match resident.apply(self.hotkey_enabled.then_some(self.hotkey_draft)){Ok(())=>self.applying_hotkey=true,Err(e)=>self.action_status=crate::i18n::Message::capture(|| format!("{e:#}"))}}
                }
                if self.applying_hotkey{ui.spinner();ui.label(crate::i18n::text("등록 중…", "Registering…"));}});
                if self.hotkey_enabled && let Err(e)=self.hotkey_draft.validate(){ui.colored_label(Color32::from_rgb(240,170,110),e.to_string());}
                ui.label(RichText::new(crate::i18n::text("다른 앱과 충돌하면 기존 단축키를 유지합니다. F12는 Windows 예약 키입니다.", "If another app uses the shortcut, the previous one is kept. F12 is reserved by Windows.")).small().weak());
            });ui.add_space(12.);
            egui::Frame::new().fill(crate::appearance::SURFACE).corner_radius(8).inner_margin(16).show(ui,|ui|{
                ui.set_min_width(620.);ui.label(RichText::new(crate::i18n::text("자동 갱신", "Auto refresh")).strong());ui.add_space(6.);
                self.refresh_controls(ui,Pane::Left);
                if self.prefs.split && !self.prefs.sync_indexes {ui.label(crate::i18n::text("오른쪽 색인", "Right index"));self.refresh_controls(ui,Pane::Right);}
                ui.label(RichText::new(crate::i18n::text("갱신 간격은 양쪽 검색 창에 공통 적용됩니다.", "The refresh interval applies to both search panes.")).small().weak());
            });ui.add_space(12.);
            if let Some(job)=self.journal_settings.show(ui) {
                if self.pending.len() >= 16 {self.journal_settings.submit_failed(crate::i18n::Message::new("작업이 많습니다. 잠시 후 다시 시도하세요", "Too many pending tasks. Try again shortly"));}
                else {match self.services.submit(job){Ok(p)=>self.pending.push(p),Err(e)=>self.journal_settings.submit_failed(crate::i18n::Message::capture(|| format!("{e:#}")))}}
            }
            ui.add_space(16.);
            ui.horizontal(|ui|{if ui.button(crate::i18n::text("완전 종료", "Exit completely")).clicked(){self.quit(ui.ctx());}ui.label(RichText::new(if self.settings_enabled {crate::i18n::text("설정 변경은 이 PC에 자동 저장됩니다.", "Settings are saved automatically on this PC.")}else{crate::i18n::text("설정 저장이 비활성화되어 있습니다.", "Settings saving is disabled.")}).small().weak());});
        });
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        let active = self.index_slot(self.focused);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .inner_margin(12)
                    .fill(Color32::from_rgb(18, 22, 29)),
            )
            .show(ui, |ui| {
                egui::Panel::bottom("application-status")
                    .resizable(false)
                    .exact_size(32.)
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        ui.spacing_mut().interact_size.y = 18.;
                        let top = ui.max_rect().top();
                        ui.painter().hline(
                            ui.max_rect().x_range(),
                            top,
                            egui::Stroke::new(1., crate::appearance::LINE),
                        );
                        ui.add_space(6.);
                        ui.horizontal(|ui| {
                            let (dot, _) =
                                ui.allocate_exact_size(egui::vec2(14., 18.), egui::Sense::hover());
                            if self.indexes[active].index_busy {
                                egui::Spinner::new()
                                    .color(crate::appearance::ACCENT)
                                    .paint_at(ui, dot);
                            } else {
                                ui.painter().circle_filled(
                                    dot.center(),
                                    3.,
                                    if self.indexes[active].ready {
                                        Color32::from_rgb(104, 192, 146)
                                    } else {
                                        crate::appearance::MUTED
                                    },
                                );
                            }
                            let right = self.panes[self.focused.slot()].view.as_ref().map_or(String::new(), |v| {
                                crate::trf!("{}개 항목", "{} items", crate::appearance::count(v.entries))
                            });
                            let message = if !self.action_status.is_empty() {
                                &self.action_status
                            } else {
                                &self.indexes[active].index_status
                            };
                            let width = (ui.available_width() - 120.).max(100.);
                            ui.add_sized(
                                [width, 18.],
                                egui::Label::new(
                                    RichText::new(message.as_str())
                                        .size(11.)
                                        .color(crate::appearance::MUTED),
                                )
                                .truncate()
                                .halign(egui::Align::Min),
                            )
                            .on_hover_text(message.as_str());
                            ui.label(
                                RichText::new(right)
                                    .size(11.)
                                    .color(crate::appearance::MUTED),
                            );
                        });
                    });
                ui.horizontal(|ui| {
                    let (logo, _) =
                        ui.allocate_exact_size(egui::vec2(30., 30.), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(logo, 8., Color32::from_rgb(37, 65, 99));
                    crate::appearance::search_mark(
                        ui.painter(),
                        logo.center(),
                        crate::appearance::ACCENT,
                    );
                    ui.label(RichText::new(crate::product::NAME).size(18.).strong());
                    if ui.selectable_label(!self.settings_tab, crate::i18n::text("검색", "Search")).clicked() { self.settings_tab = false; }
                    if ui.selectable_label(self.settings_tab, crate::i18n::text("설정", "Settings")).clicked() { self.settings_tab = true; self.cancel_file_drag(); }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(crate::i18n::text("색인 관리", "Manage index"))
                            .on_hover_text(crate::i18n::text("검색 위치, NTFS 드라이브, 저장한 색인 관리", "Manage search locations, NTFS drives, and saved indexes"))
                            .clicked()
                        {
                            self.index_target=self.focused;
                            self.index_dialog = true;
                        }
                        if ui
                            .add_enabled(self.indexes[active].ready && !self.indexes[active].index_busy, egui::Button::new(crate::i18n::text("갱신", "Refresh")))
                            .on_hover_text(crate::i18n::text("검색 목록 새로 고침 · F5", "Refresh search results · F5"))
                            .clicked()
                        {
                            self.request_index(IndexCommand::Refresh, crate::i18n::Message::new("변경 사항을 반영하는 중...", "Applying changes..."));
                        }
                        if ui
                            .selectable_label(self.prefs.split, crate::i18n::text("분할 보기", "Split view"))
                            .on_hover_text(crate::i18n::text("좌우에서 독립 검색 · F6으로 전환", "Search independently in both panes · Switch with F6"))
                            .clicked()
                        {
                            self.prefs.split = !self.prefs.split;
                            self.dirty();
                            if self.prefs.split {
                                if self.prefs.sync_indexes && self.indexes[0].ready {self.request_search(Pane::Right,false);}
                                else{self.ensure_right_index();}
                            }
                            if !self.prefs.split {
                                self.focused = Pane::Left;
                            }
                        }
                        if self.prefs.split {
                            let mut shared=self.prefs.sync_indexes;
                            if ui.checkbox(&mut shared,crate::i18n::text("색인 공유", "Share index")).on_hover_text(crate::i18n::text("켜면 왼쪽 색인을 양쪽에서 사용합니다. 끄면 양쪽에서 다른 폴더를 색인할 수 있습니다.", "Use the left index for both panes. Turn off to index different folders in each pane.")).changed(){self.set_index_sync(shared);}
                        }
                        ui.menu_button(crate::i18n::text("도움말", "Help"), |ui| {
                            if ui.button(crate::i18n::text("검색 문법 · 단축키", "Search syntax · Shortcuts")).clicked() {
                                self.help = true;
                                ui.close();
                            }
                            ui.separator();
                            self.refresh_controls(ui,self.focused);
                        });
                    });
                });
                if self.settings_tab { self.settings_page(ui); } else {
                ui.add_space(4.);
                ui.horizontal(|ui| {
                    let (kind, location) = match &self.indexes[active].active_source {
                        Some(crate::index::Source::Folder { root }) => (
                            crate::i18n::text("폴더", "Folders"),
                            dunce::simplified(root).to_string_lossy().into_owned(),
                        ),
                        Some(crate::index::Source::Ntfs { drive, .. }) => {
                            ("NTFS", format!("{drive}:\\"))
                        }
                        None => (crate::i18n::text("준비 중", "Preparing"), self.indexes[active].folder.clone()),
                    };
                    egui::Frame::new()
                        .fill(crate::appearance::SURFACE)
                        .corner_radius(4)
                        .inner_margin(egui::Margin::symmetric(7, 3))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(kind)
                                    .size(10.)
                                    .color(crate::appearance::ACCENT),
                            );
                        });
                    ui.add(
                        egui::Label::new(
                            RichText::new(&location)
                                .size(12.)
                                .color(crate::appearance::MUTED),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&location);
                    if self.indexes[active].live {
                        ui.label(
                            RichText::new(crate::trf!(
                                "자동 갱신 · {}초", "Auto refresh · {} s",
                                self.prefs.refresh_interval_secs
                            ))
                            .size(10.)
                            .color(crate::appearance::MUTED),
                        );
                    }
                });
                ui.add_space(6.);
                ui.separator();
                ui.add_space(4.);
                if self.prefs.split {
                    if !self.has_modal()
                        && ui
                            .ctx()
                            .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F6))
                    {
                        self.focused = match self.focused {
                            Pane::Left => Pane::Right,
                            Pane::Right => Pane::Left,
                        };
                        ui.ctx().memory_mut(|m| {
                            m.request_focus(egui::Id::new(("search-query", self.focused)))
                        });
                    }
                    let width = ui.available_width();
                    egui::Panel::left("left-search-pane")
                        .resizable(true)
                        .default_size(width / 2.)
                        .min_size(320.)
                        .max_size((width - 320.).max(320.))
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| self.show_pane(ui, Pane::Left));
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| self.show_pane(ui, Pane::Right));
                } else {
                    self.show_pane(ui, Pane::Left);
                }
                }
            });
        self.update_shell(ui.ctx());
        self.update_metadata(ui.ctx());
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        self.update_file_drag(ui);
        if let Some(dialog) = &mut self.rename_dialog {
            let plan = dialog.show(ui.ctx());
            let open = dialog.open;
            if let Some(plan) = plan {
                self.submit(Job::Rename(plan));
            }
            if !open {
                self.rename_dialog = None;
            }
        }

        if self.index_dialog {
            if !self.prefs.split {
                self.index_target = Pane::Left;
            }
            let mut open = true;
            let mut close_after = false;
            egui::Window::new(crate::i18n::text("색인 관리", "Manage index"))
                .id(egui::Id::new("index-manager"))
                .default_pos(ui.ctx().viewport_rect().center() - egui::vec2(280., 275.))
                .open(&mut open)
                .collapsible(false)
                .default_width(560.)
                .default_height(620.)
                .max_width((ui.ctx().viewport_rect().width() - 48.).max(300.))
                .max_height((ui.ctx().viewport_rect().height() - 48.).max(200.))
                .vscroll(true)
                .show(ui.ctx(), |ui| {
                    if self.prefs.split && !self.prefs.sync_indexes {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.index_target, Pane::Left, crate::i18n::text("왼쪽 색인", "Left index"));
                            ui.selectable_value(&mut self.index_target, Pane::Right, crate::i18n::text("오른쪽 색인", "Right index"));
                        });
                    } else {
                        ui.label(
                            RichText::new(if self.prefs.sync_indexes {
                                crate::i18n::text("공유 색인 · 왼쪽 기준", "Shared index · Uses left pane")
                            } else {
                                crate::i18n::text("왼쪽 색인", "Left index")
                            })
                            .small()
                            .weak(),
                        );
                    }
                    let scope = self.index_slot(self.index_target);

                    ui.label(RichText::new(crate::i18n::text("검색할 위치", "Search location")).size(17.).strong());
                    ui.label(
                        RichText::new(crate::i18n::text("성공한 마지막 폴더는 다음 실행 때 자동으로 불러옵니다.", "The last successfully indexed folder is loaded automatically at startup."))
                            .size(12.)
                            .weak(),
                    );
                    ui.add_space(8.);
                    ui.label(RichText::new(crate::i18n::text("폴더", "Folders")).strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.indexes[scope].folder)
                            .desired_width(f32::INFINITY)
                            .hint_text(r"D:\Projects"),
                    );
                    if ui
                        .add_enabled(
                            !self.indexes[scope].folder.trim().is_empty(),
                            egui::Button::new(crate::i18n::text("이 폴더 색인", "Index this folder")).fill(Color32::from_rgb(43, 74, 110)),
                        )
                        .clicked()
                    {
                        self.indexes[scope].live = false;
                        self.request_index_in(
                            self.index_target,
                            IndexCommand::Folder(self.indexes[scope].folder.clone().into()),
                            crate::i18n::Message::new("폴더를 색인하는 중...", "Indexing folder..."),
                        );
                        close_after = true;
                    }
                    ui.add_space(8.);
                    ui.separator();
                    ui.add_space(4.);
                    ui.label(RichText::new(crate::i18n::text("NTFS 드라이브", "NTFS drive")).strong());
                    ui.label(
                        RichText::new(crate::i18n::text("볼륨 읽기 권한과 기존 USN 저널이 필요합니다.", "Requires volume read permission and an existing USN journal."))
                            .size(12.)
                            .weak(),
                    );
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.indexes[scope].drive)
                                .desired_width(44.)
                                .hint_text("D"),
                        );
                        if ui.button(crate::i18n::text("드라이브 색인", "Index drive")).clicked() {
                            let input = self.indexes[scope].drive.trim().trim_end_matches(':');
                            if input.len() == 1 && input.as_bytes()[0].is_ascii_alphabetic() {
                                self.indexes[scope].live = true;
                                self.request_index_in(
                                    self.index_target,
                                    IndexCommand::Drive(input.chars().next().unwrap()),
                                    crate::i18n::Message::new("NTFS MFT를 읽는 중...", "Reading NTFS MFT..."),
                                );
                                close_after = true;
                            } else {
                                self.indexes[scope].index_status =
                                    crate::i18n::Message::new("드라이브 문자를 입력하세요. 예: D", "Enter a drive letter, for example D");
                            }
                        }
                    });
                    ui.add_space(8.);
                    ui.separator();
                    ui.add_space(4.);
                    ui.label(RichText::new(crate::i18n::text("저장된 색인", "Saved index")).strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.indexes[scope].cache)
                            .desired_width(f32::INFINITY)
                            .hint_text("index.reaidx"),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.indexes[scope].cache.trim().is_empty(),
                                egui::Button::new(crate::i18n::text("불러오기", "Load")),
                            )
                            .clicked()
                        {
                            self.request_index_in(
                                self.index_target,
                                IndexCommand::Load(self.indexes[scope].cache.clone().into()),
                                crate::i18n::Message::new("색인 파일을 읽는 중...", "Loading index file..."),
                            );
                            close_after = true;
                        }
                        if ui
                            .add_enabled(
                                self.indexes[scope].ready && !self.indexes[scope].index_busy,
                                egui::Button::new(crate::i18n::text("현재 색인 저장", "Save current index")),
                            )
                            .clicked()
                        {
                            self.request_index_in(
                                self.index_target,
                                IndexCommand::Save(self.indexes[scope].cache.clone().into()),
                                crate::i18n::Message::new("저장하는 중...", "Saving..."),
                            );
                        }
                    });
                    ui.add_space(8.);
                    ui.separator();
                    self.refresh_controls(ui, self.index_target);
                    if let Some(view) = &self.panes[self.index_target.slot()].view {
                        ui.label(
                            RichText::new(crate::trf!(
                                "{}개 항목 · 색인 메모리 {} · 접근 오류 {}개", "{} items · Index memory {} · {} access errors",
                                crate::appearance::count(view.entries),
                                crate::byte_size::format(view.memory as u64),
                                view.coverage
                            ))
                            .size(11.)
                            .weak(),
                        );
                    }
                    ui.add(egui::Label::new(self.indexes[scope].index_status.as_str()).wrap());
                });
            self.index_dialog = open && !close_after;
        }
        if self.help {
            egui::Window::new(crate::i18n::text("검색 도움말", "Search help"))
                .id(egui::Id::new("search-help"))
                .open(&mut self.help)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    for line in [
                        crate::i18n::text("공백: AND · |: OR · !: 제외 · 괄호: 그룹", "Space: AND · |: OR · !: exclude · Parentheses: group"),
                        crate::i18n::text("따옴표: 공백이 있는 이름 · * / ?: 와일드카드", "Quotes: names with spaces · * / ?: wildcards"),
                        crate::i18n::text("file: / folder: · ext:jpg;png · path:폴더명", "file: / folder: · ext:jpg;png · path:folder-name"),
                        "image: / doc: / audio: / video: / zip: / exe:",
                        crate::i18n::text("Ctrl+F / Ctrl+L: 검색창 · F5: 갱신", "Ctrl+F / Ctrl+L: search box · F5: refresh"),
                        crate::i18n::text("↑ / ↓: 결과 선택 · Enter: 기본 앱으로 열기", "↑ / ↓: select result · Enter: open with default app"),
                        crate::i18n::text("Ctrl+Enter: 탐색기에서 보기 · Ctrl+Shift+C: 경로 복사", "Ctrl+Enter: show in Explorer · Ctrl+Shift+C: copy path"),
                        crate::i18n::text("Ctrl/Shift 클릭: 여러 선택 · Ctrl+A: 표시 결과 전체 선택", "Ctrl/Shift+click: select multiple · Ctrl+A: select all displayed results"),
                        crate::i18n::text("Ctrl+C / Ctrl+X: 파일 복사/잘라내기 · Ctrl+V: 선택 폴더에 붙여넣기", "Ctrl+C / Ctrl+X: copy/cut files · Ctrl+V: paste into selected folder"),
                        crate::i18n::text("F2: 이름 변경 · 여러 개는 번호 붙이기 또는 찾아 바꾸기", "F2: rename · Use numbering or find and replace for multiple items"),
                        crate::i18n::text("좌우 분할: 독립 검색 · F6: 검색창 전환 · 경계선: 크기 조절", "Split view: independent searches · F6: switch pane · Drag divider: resize"),
                        crate::i18n::text("빈 곳 드래그: 여러 선택 · 파일 이름 드래그: 선택 파일 끌기", "Drag empty space: select multiple · Drag a filename: drag selected files"),
                        crate::i18n::text("다른 창의 폴더에 놓기: 복사 · Shift: 이동 · Esc: 취소", "Drop on another pane's folder: copy · Shift: move · Esc: cancel"),
                        crate::i18n::text("이름 헤더 클릭: 정렬 방향 변경", "Click a column header to change sort order"),
                        crate::i18n::text("크기: 보이는 파일·폴더부터 자동 계산 · ≥: 일부 합계", "Size: visible files and folders are calculated first · ≥: partial total"),
                        crate::i18n::text("파일 아이콘: Windows 기본 아이콘과 설치된 상태 오버레이 사용", "File icons: native Windows icons and installed status overlays"),
                        crate::i18n::text("수정일·확장자: 추가 열 표시", "Modified date and extension: show additional columns"),
                        crate::i18n::text("최근 검색은 Enter 또는 파일 작업 시 기록됩니다", "Recent searches are recorded on Enter or file operations"),
                    ] {
                        ui.label(line);
                    }
                });
        }
        if self.bookmark_dialog {
            let mut open = true;
            let mut add = false;
            let mut remove = None;
            egui::Window::new(crate::i18n::text("즐겨찾기 관리", "Manage bookmarks"))
                .id(egui::Id::new("bookmarks-dialog"))
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.label(crate::i18n::text(
                        "현재 검색어와 필터·검색 옵션·정렬을 저장합니다",
                        "Save the current query, filter, search options, and sort order",
                    ));
                    ui.text_edit_singleline(&mut self.bookmark_name);
                    add = ui
                        .add_enabled(
                            self.prefs.bookmarks.len() < 32
                                && !self.bookmark_name.trim().is_empty()
                                && self.bookmark_name.len() <= 200
                                && self
                                    .bookmark_source
                                    .as_ref()
                                    .is_some_and(|(q, _)| q.len() <= 8192),
                            egui::Button::new(crate::i18n::text(
                                "현재 검색 추가",
                                "Add current search",
                            )),
                        )
                        .clicked();
                    ui.separator();
                    for (n, b) in self.prefs.bookmarks.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(&b.name);
                            ui.label(egui::RichText::new(&b.query).weak());
                            if ui
                                .small_button(crate::i18n::text("삭제", "Delete"))
                                .clicked()
                            {
                                remove = Some(n);
                            }
                        });
                    }
                });
            if add {
                self.prefs.bookmarks.push(Bookmark {
                    name: self.bookmark_name.trim().into(),
                    query: self.bookmark_source.as_ref().unwrap().0.clone(),
                    options: self.bookmark_source.as_ref().unwrap().1.options,
                    filter: self.bookmark_source.as_ref().unwrap().1.filter,
                    order: self.bookmark_source.as_ref().unwrap().1.order,
                });
                self.dirty();
                open = false;
            }
            if let Some(n) = remove {
                self.prefs.bookmarks.remove(n);
                self.dirty();
            }
            self.bookmark_dialog = open;
        }
        self.tick_background(ui.ctx());
        if self.indexes.iter().any(|i| i.index_busy)
            || self.panes[0].search_busy
            || (self.prefs.split && self.panes[1].search_busy)
            || self
                .active_indexes()
                .iter()
                .any(|p| self.indexes[self.index_slot(*p)].live)
            || !self.pending.is_empty()
            || (self.settings_enabled && self.prefs_dirty.is_some())
        {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }
}
impl Drop for App {
    fn drop(&mut self) {
        #[cfg(windows)]
        drop(self.resident.take());
        self.cancel_file_drag();
        #[cfg(windows)]
        self.shell_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.settings_enabled && self.prefs_dirty.is_some() {
            let _ = self.services.save_on_exit(self.prefs.clone());
        }
    }
}
pub fn launch(
    folder: Option<PathBuf>,
    drive: Option<char>,
    index: Option<PathBuf>,
    workers: usize,
    renderer: crate::graphics::RendererChoice,
) -> Result<()> {
    #[cfg(windows)]
    let _ole = crate::native_drag::Apartment::initialize()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(Arc::new(crate::app_icon::data()))
            .with_drag_and_drop(false)
            .with_inner_size([1400., 820.])
            .with_min_inner_size([980., 560.]),
        ..crate::graphics::options(renderer)
    };
    eframe::run_native(
        crate::product::NAME,
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, folder, drive, index, workers)?))),
    )
    .map_err(|e| anyhow::anyhow!("Desktop application failed: {e}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn detail_columns_change_without_search_and_use_visible_metadata() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        for _ in 0..3 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        let search = app.panes[0].search_id;
        let rows = app.panes[0].view.as_ref().unwrap().rows.as_ptr();
        let out = draw(&ctx, &mut app, input(None));
        let pos = text_rect(&out.shapes, "검색 옵션").center();
        for pressed in [true, false] {
            let _ = draw(&ctx, &mut app, pointer(pos, Some(pressed)));
        }
        let out = draw(&ctx, &mut app, input(None));
        let pos = text_rect(&out.shapes, "수정일 · 파일 종류 열").center();
        for pressed in [true, false] {
            let _ = draw(&ctx, &mut app, pointer(pos, Some(pressed)));
        }
        assert!(app.panes[0].settings.details);
        assert_eq!(app.panes[0].search_id, search);
        assert_eq!(app.panes[0].view.as_ref().unwrap().rows.as_ptr(), rows);
        // A measurement from the visible-row worker supplies date and size even
        // when a legacy saved index has no cached display metadata.
        let path = app.panes[0].view.as_ref().unwrap().rows[0]
            .path
            .clone()
            .unwrap();
        app.panes[0].view.as_mut().unwrap().rows[0].details = None;
        let measured = Details {
            size: Some(77),
            modified: Some(std::time::UNIX_EPOCH + Duration::from_secs(86400)),
            error: None,
        };
        let date = measured.date_text();
        app.panes[0].decorations.insert(
            path,
            crate::row_metadata::Decoration {
                details: measured,
                size: Some(crate::row_metadata::Size {
                    bytes: 77,
                    complete: true,
                    note: None,
                }),
                size_note: None,
                #[cfg(windows)]
                icon: None,
                icon_note: None,
                measured: Instant::now(),
            },
        );
        let out = draw(&ctx, &mut app, input(None));
        text_rect(&out.shapes, "77 B");
        text_rect(&out.shapes, &date);
    }
    use super::*;
    use crate::background::View;
    fn input(key: Option<(egui::Key, egui::Modifiers)>) -> egui::RawInput {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1120., 760.),
            )),
            ..Default::default()
        };
        if let Some((key, modifiers)) = key {
            input.events.push(egui::Event::ModifiersChanged(modifiers));
            input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            });
        }
        input
    }
    fn draw(ctx: &egui::Context, app: &mut App, input: egui::RawInput) -> egui::FullOutput {
        let mut output = ctx.run_ui(input, |ui| app.show(ui));
        // No GPU backend in this test. Explicitly acknowledge unuploaded texture deltas.
        output.textures_delta.clear();
        output
    }

    fn fixture() -> App {
        let primary = SearchPane {
            view: Some(View {
                rows: vec![
                    Row {
                        name: "a.txt".into(),
                        path: Some(r"C:\sample\a.txt".into()),
                        directory: false,
                        details: Some(Details {
                            size: Some(4),
                            modified: Some(std::time::SystemTime::now()),
                            error: None,
                        }),
                    },
                    Row {
                        name: "folder".into(),
                        path: Some(r"C:\sample\folder".into()),
                        directory: true,
                        details: None,
                    },
                ],
                total: 2,
                entries: 2,
                memory: 1024,
                elapsed_ms: 1.,
                source: "test".into(),
                coverage: 0,
            }),
            ..Default::default()
        };
        let left = Runtime::new(Arc::new(|| {}), 2).unwrap();
        let right = left.independent().unwrap();
        let mut indexes = [IndexSession::new(left), IndexSession::new(right)];
        indexes[0].ready = true;
        indexes[0].folder = r"C:\sample".into();
        indexes[0].drive = "C".into();
        indexes[0].cache = "test.reaidx".into();
        indexes[0].active_source = Some(crate::index::Source::Folder {
            root: std::env::current_dir().unwrap(),
        });
        App {
            indexes,
            index_target: Pane::Left,
            metadata_epoch: [0, 0],

            drag: None,
            drag_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            window: None,
            reset_pointer: false,
            settings_tab: false,
            journal_settings: Default::default(),
            quitting: false,
            hotkey_draft: Default::default(),
            hotkey_enabled: true,
            applying_hotkey: false,
            #[cfg(windows)]
            resident: None,
            metadata: crate::row_metadata::Runtime::new(Arc::new(|| {})).unwrap(),
            panes: [primary, SearchPane::default()],
            focused: Pane::Left,
            prefs: Preferences::default(),
            services: Services::new(None, Arc::new(|| {})).unwrap(),
            pending: Vec::new(),
            settings_enabled: false,
            prefs_dirty: None,
            action_status: crate::i18n::Message::default(),
            index_dialog: false,
            bookmark_dialog: false,
            bookmark_name: String::new(),
            bookmark_source: None,
            help: false,
            #[cfg(windows)]
            shell: None,
            #[cfg(windows)]
            shell_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            rename_dialog: None,
        }
    }

    #[test]
    fn language_live_switch_uses_an_isolated_process() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "gui::tests::language_live_child",
            "--ignored",
            "--nocapture",
        ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[test]
    #[ignore = "Language mutation is isolated from other UI tests"]
    fn language_live_child() {
        use crate::i18n::{self, Language, Message};
        fn choose(ctx: &egui::Context, app: &mut App, language: Language) {
            let out = draw(ctx, app, input(None));
            let pos = text_rect(&out.shapes, app.prefs.language.label()).center();
            for pressed in [true, false] {
                let _ = draw(ctx, app, pointer(pos, Some(pressed)));
            }
            let out = draw(ctx, app, input(None));
            let pos = text_rect(&out.shapes, language.label()).center();
            for pressed in [true, false] {
                let _ = draw(ctx, app, pointer(pos, Some(pressed)));
            }
            let _ = draw(ctx, app, input(None));
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("preferences.json");
        let (mut store, mut prefs) = Store::load(path.clone()).unwrap();
        prefs.language = Language::English;
        prefs.remember("한글 query");
        store.save(&prefs).unwrap();
        let (_, saved) = Store::load(path.clone()).unwrap();
        i18n::set_language(saved.language);
        let ctx = egui::Context::default();
        crate::appearance::configure(&ctx);
        let mut app = fixture();
        app.prefs = saved;
        app.settings_tab = true;
        app.panes[0].query = "한글 query".into();
        app.panes[0].selection.click(1, false, false, false);
        let initial_search = app.panes[0].search_id;
        let initial_index = app.indexes[0].index_id;
        let view = app.panes[0].view.as_ref().unwrap().rows.as_ptr();
        let error = crate::fileops::valid_name("").unwrap_err();
        let original_path = r"C:\한글\이름은 1~255 UTF-16 문자여야 합니다";
        app.action_status = Message::capture(|| format!("{error:#} · {original_path}"));
        app.panes[0].search_status =
            crate::msgf!("{}개 검색 결과 · {:.2} ms", "{} results · {:.2} ms", 2, 1.);
        for _ in 0..3 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        assert!(app.action_status.starts_with("Names must contain"));
        #[cfg(windows)]
        assert_eq!(
            crate::resident::test_tray_labels(),
            ["Open", "Settings", "Exit"]
        );
        choose(&ctx, &mut app, Language::Korean);
        let out = draw(&ctx, &mut app, input(None));
        text_rect(&out.shapes, "언어");
        text_rect(&out.shapes, "백그라운드 실행");
        assert!(app.prefs_dirty.is_some());
        assert_eq!(app.prefs.language, Language::Korean);
        assert!(app.action_status.starts_with("이름은 1~255"));
        assert!(app.action_status.ends_with(original_path));
        assert!(app.panes[0].search_status.starts_with("2개 검색 결과"));
        assert_eq!(crate::index::SortColumn::Path.label(), "위치");
        #[cfg(windows)]
        assert_eq!(
            crate::resident::test_tray_labels(),
            ["열기", "설정", "종료"]
        );
        // A result produced on a worker before switching is rendered after switching.
        let worker_message = std::thread::spawn(|| {
            anyhow::anyhow!(Message::new("작업 스레드 종료", "Worker stopped"))
        })
        .join()
        .unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(Err(worker_message)).unwrap();
        app.pending.push(Pending {
            kind: Kind::Other,
            receiver: rx,
        });
        choose(&ctx, &mut app, Language::English);
        app.poll();
        assert!(app.action_status.contains("Worker stopped"));
        assert!(!app.action_status.contains("작업"));
        let out = draw(&ctx, &mut app, input(None));
        text_rect(&out.shapes, "Language");
        text_rect(&out.shapes, "Run in background");
        assert_eq!(crate::filters::Filter::Folders.label(), "Folders");
        assert_eq!(app.panes[0].query, "한글 query");
        assert_eq!(app.prefs.history, ["한글 query"]);
        assert_eq!(app.panes[0].selection.focus, Some(1));
        assert_eq!(app.panes[0].view.as_ref().unwrap().rows.as_ptr(), view);
        assert_eq!(app.panes[0].search_id, initial_search);
        assert_eq!(app.indexes[0].index_id, initial_index);
        store.save(&app.prefs).unwrap();
        assert_eq!(Store::load(path).unwrap().1.language, Language::English);
        // CPU preview of the actual live-switched UI; no desktop window.
        let mut renderer = crate::ui_preview::Renderer::default();
        let ctx = egui::Context::default();
        crate::appearance::configure(&ctx);
        let preview = std::path::Path::new("artifacts/ui/language-live-settings.rgba");
        std::fs::create_dir_all(preview.parent().unwrap()).unwrap();
        for _ in 0..6 {
            let out = ctx.run_ui(input(None), |ui| app.show(ui));
            text_rect(&out.shapes, "Language");
            renderer.save(&ctx, out, [1120, 760], preview);
        }
    }
    #[test]
    fn result_open_actions_keep_directory_intent_for_pointer_and_keyboard() {
        use crate::search_pane::{Action, PaneContext};
        fn render(
            ctx: &egui::Context,
            pane: &mut SearchPane,
            prefs: &mut Preferences,
            raw: egui::RawInput,
        ) -> Vec<Action> {
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(raw, |ui| {
                actions = pane.show(
                    ui,
                    PaneContext {
                        pane: Pane::Left,
                        focused: true,
                        split: false,
                        ready: true,
                        index_busy: false,
                        modal: false,
                        file_drag: false,
                        native_shell: false,
                        index_location: String::new(),
                    },
                    prefs,
                );
            });
            out.textures_delta.clear();
            actions
        }
        for directory in [false, true] {
            let ctx = egui::Context::default();
            let mut app = fixture();
            let pane = &mut app.panes[0];
            let row = usize::from(directory);
            let expected = pane.view.as_ref().unwrap().rows[row].path.clone().unwrap();
            for _ in 0..3 {
                render(&ctx, pane, &mut app.prefs, input(None));
            }
            let pos = pane.row_hits[row].name.center();
            // Capture actions without submitting them to the native desktop service.
            let mut captured = Vec::new();
            for pressed in [true, false, true, false] {
                captured.extend(render(
                    &ctx,
                    pane,
                    &mut app.prefs,
                    pointer(pos, Some(pressed)),
                ));
            }
            assert!(captured.iter().any(|action| matches!(action,
                Action::Job(Job::Open { path, directory: d }) if *d == directory && path == &expected)));
            let actions = render(
                &ctx,
                pane,
                &mut app.prefs,
                input(Some((egui::Key::Enter, egui::Modifiers::NONE))),
            );
            assert!(actions.iter().any(|action| matches!(action,
                Action::Job(Job::Open { path, directory: d }) if *d == directory && path == &expected)));
        }
    }

    #[test]
    fn result_size_header_and_values_share_the_left_edge_in_both_panes() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        app.prefs.split = true;
        app.panes[1].view = fixture().panes[0].view.take();
        app.panes[0].settings.details = true;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        let folder = dir.path().join("folder");
        std::fs::write(&file, b"1234").unwrap();
        std::fs::create_dir(&folder).unwrap();
        for pane in &mut app.panes {
            let rows = &mut pane.view.as_mut().unwrap().rows;
            rows[0].path = Some(file.clone());
            rows[1].path = Some(folder.clone());
        }
        for _ in 0..5 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        let out = draw(&ctx, &mut app, input(None));
        for pane in &app.panes {
            let hit = &pane.row_hits[0];
            let cells = ResultColumns::new(hit.rect, pane.settings.details);
            let size = out
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t)
                        if t.galley.job.text == "4 B"
                            && (t.pos.y - hit.rect.center().y).abs() < 20.
                            && (t.pos.x - cells.size.left()).abs() < 1. =>
                    {
                        Some(t.pos)
                    }
                    _ => None,
                })
                .expect("size cell is not left-aligned");
            assert!((size.x - cells.size.left()).abs() < 0.1);
            assert!(out.shapes.iter().any(|s|matches!(&s.shape,egui::Shape::Text(t) if t.galley.job.text=="크기" && (t.pos.x-cells.size.left()).abs()<0.1)));
        }
    }

    #[test]
    fn headers_sort_each_column_per_pane_and_settings_tab_keeps_search_state() {
        fn header_text_rect(shapes: &[egui::epaint::ClippedShape], label: &str) -> egui::Rect {
            shapes
                .iter()
                .rev()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.job.text == label => {
                        Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing header: {label}"))
        }
        use crate::index::SortColumn;
        let ctx = egui::Context::default();
        let mut app = fixture();
        app.panes[0].query = "preserved".into();
        app.panes[0].settings.details = true;
        // No index snapshot in this UI fixture: examine the generated order request, not a mock sort.
        app.indexes[0].ready = false;
        for column in SortColumn::ALL {
            let out = draw(&ctx, &mut app, input(None));
            let label = if app.panes[0].settings.order.column == column {
                app.panes[0].settings.order.label()
            } else {
                column.label().into()
            };
            let rect = header_text_rect(&out.shapes, &label);
            for event in [None, Some(true), Some(false)] {
                let _ = draw(&ctx, &mut app, pointer(rect.center(), event));
            }
            assert_eq!(app.panes[0].settings.order.column, column);
            assert_eq!(app.panes[1].settings.order, Default::default());
            let previous = app.panes[0].settings.order.descending;
            let out = draw(&ctx, &mut app, input(None));
            let rect = header_text_rect(&out.shapes, &app.panes[0].settings.order.label());
            for event in [None, Some(true), Some(false)] {
                let _ = draw(&ctx, &mut app, pointer(rect.center(), event));
            }
            assert_eq!(app.panes[0].settings.order.descending, !previous);
        }
        let saved = app.panes[0].settings.order;
        let out = draw(&ctx, &mut app, input(None));
        let rect = text_rect(&out.shapes, "설정");
        for event in [None, Some(true), Some(false)] {
            let _ = draw(&ctx, &mut app, pointer(rect.center(), event));
        }
        assert!(app.settings_tab);
        assert_eq!(app.panes[0].query, "preserved");
        assert_eq!(app.panes[0].settings.order, saved);
        let out = draw(&ctx, &mut app, input(None));
        let _ = text_rect(&out.shapes, "창 열기 단축키");
    }
    #[test]
    fn close_requires_a_recovery_path_and_explicit_quit_bypasses_background_mode() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        let mut out = ctx.run_ui(input(None), |ui| app.handle_close(ui.ctx(), true));
        assert!(
            out.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose)
        );
        assert!(
            !out.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Visible(false))
        );
        out.textures_delta.clear();
        #[cfg(windows)]
        {
            let hotkey = crate::hotkey::Hotkey {
                ctrl: true,
                alt: true,
                shift: true,
                win: false,
                key: 133,
            };
            app.resident = Some(crate::resident::Controller::test_controller(hotkey).unwrap());
            let deadline = Instant::now() + Duration::from_secs(3);
            while !app.resident.as_ref().unwrap().can_hide() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            let mut out = ctx.run_ui(input(None), |ui| app.handle_close(ui.ctx(), true));
            assert!(
                out.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .contains(&egui::ViewportCommand::CancelClose)
            );
            assert!(
                out.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .contains(&egui::ViewportCommand::Visible(false))
            );
            out.textures_delta.clear();
        }
        app.prefs.close_to_tray = false;
        let mut out = ctx.run_ui(input(None), |ui| app.handle_close(ui.ctx(), true));
        assert!(
            !out.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose)
        );
        out.textures_delta.clear();
        app.prefs.close_to_tray = true;
        let mut out = ctx.run_ui(input(None), |ui| {
            app.quit(ui.ctx());
            app.handle_close(ui.ctx(), true);
        });
        assert!(app.quitting);
        assert!(
            out.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Close)
        );
        assert!(
            !out.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose)
        );
        out.textures_delta.clear();
    }

    #[test]
    fn actual_gui_renders_and_search_focus_result_navigation_and_copy_work() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        let output = draw(&ctx, &mut app, input(None));
        assert!(!output.shapes.is_empty());
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::L, egui::Modifiers::CTRL))),
        );
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new(("search-query", Pane::Left)))));
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::ArrowDown, egui::Modifiers::NONE))),
        );
        assert_eq!(app.panes[0].selection.focus, Some(0));
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::ArrowDown, egui::Modifiers::NONE))),
        );
        assert_eq!(app.panes[0].selection.focus, Some(1));
        let out = draw(
            &ctx,
            &mut app,
            input(Some((
                egui::Key::C,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            ))),
        );
        assert!(
            out.platform_output
                .commands
                .iter()
                .any(|c| matches!(c,egui::OutputCommand::CopyText(s) if s==r"C:\sample\folder"))
        );
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::ArrowUp, egui::Modifiers::SHIFT))),
        );
        assert_eq!(
            app.panes[0].selection.items,
            std::collections::BTreeSet::from([0, 1])
        );
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::A, egui::Modifiers::CTRL))),
        );
        assert_eq!(app.panes[0].selection.items.len(), 2);
        let out = draw(
            &ctx,
            &mut app,
            input(Some((
                egui::Key::C,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            ))),
        );
        assert!(out.platform_output.commands.iter().any(|c|matches!(c,egui::OutputCommand::CopyText(s) if s== "C:\\sample\\a.txt\r\nC:\\sample\\folder")));
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::F2, egui::Modifiers::NONE))),
        );
        assert!(app.rename_dialog.is_some());
        app.rename_dialog = None;
        // Restore selected paths after a refresh reorders or removes rows.
        let saved = app.panes[0]
            .selection
            .save(&app.panes[0].view.as_ref().unwrap().rows);
        app.panes[0].view.as_mut().unwrap().rows.swap(0, 1);
        app.panes[0]
            .selection
            .restore(saved, &app.panes[0].view.as_ref().unwrap().rows);
        assert_eq!(app.panes[0].selection.items.len(), 2);
        assert_eq!(app.panes[0].selection.focus, Some(1));
        app.panes[0].settings.details = true;
        let out = draw(&ctx, &mut app, input(None));
        assert!(!out.shapes.is_empty());
    }
    #[test]
    #[ignore = "measures real wheel input and native icon texture churn without a visible window"]
    fn focused_wheel_measurement() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        crate::appearance::configure(&ctx);
        let mut app = fixture();
        let mut rows = Vec::new();
        for n in 0..27 {
            let parent = dir.path().join(format!(
                "Global_Trunk/Server/deploy/Inhouse/BranchPublisherLive/Region{n:02}/config"
            ));
            std::fs::create_dir_all(&parent).unwrap();
            let path = parent.join("ServerConfig.xml");
            std::fs::write(&path, b"<config />").unwrap();
            rows.push(Row {
                name: "ServerConfig.xml".into(),
                path: Some(path),
                directory: false,
                details: None,
            });
        }
        app.panes[0].query = r"serverconfig.xml \global_tru".into();
        app.panes[0].settings.details = true;
        app.panes[0].view.as_mut().unwrap().rows = rows;
        app.panes[0].view.as_mut().unwrap().total = 27;
        let search_id = app.panes[0].search_id;
        let mut frame_ms = Vec::new();
        let mut uploads = 0usize;
        let mut frees = 0usize;
        let mut upload_bytes = 0usize;
        let mut max_offset = 0f32;
        let start = Instant::now();
        for frame in 0..126 {
            let mut raw = input(None);
            raw.time = Some(frame as f64 / 60.);
            raw.focused = true;
            raw.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1120., 560.),
            ));
            if frame >= 6 {
                raw.events
                    .push(egui::Event::PointerMoved(egui::pos2(700., 380.)));
                raw.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    delta: egui::vec2(0., if frame / 15 % 2 == 0 { -90. } else { 90. }),
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let frame_start = Instant::now();
            let mut out = ctx.run_ui(raw, |ui| app.show(ui));
            frame_ms.push(frame_start.elapsed().as_secs_f64() * 1000.);
            for (id, deltas) in &out.textures_delta.set {
                for delta in deltas {
                    if *id != egui::TextureId::Managed(0) && delta.pos.is_none() {
                        uploads += 1;
                        let size = delta.image.size();
                        upload_bytes += size[0] * size[1] * 4;
                    }
                }
            }
            frees += out
                .textures_delta
                .free
                .iter()
                .filter(|id| **id != egui::TextureId::Managed(0))
                .count();
            out.textures_delta.clear();
            max_offset = max_offset.max(app.panes[0].scroll_offset.y);
            let wait = Instant::now();
            while app.panes[0].metadata_busy {
                app.update_metadata(&ctx);
                assert!(wait.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(app.panes[0].search_id, search_id);
        assert!(max_offset > 100.);
        let first_ms = frame_ms[0];
        let mut warm = frame_ms[6..].to_vec();
        warm.sort_by(f64::total_cmp);
        println!(
            "{}",
            serde_json::json!({"completed_frames": frame_ms.len(), "first_frame_ms":first_ms, "warm_median_ms":warm[warm.len()/2], "warm_max_ms":warm[warm.len()-1], "elapsed_ms":start.elapsed().as_secs_f64()*1000., "icon_uploads":uploads, "icon_frees":frees, "icon_upload_bytes":upload_bytes, "max_offset":max_offset, "scope":"debug egui input/layout + native icons on 27 temporary XML files; no GPU presentation, no user workspace mutation"})
        );
    }

    #[test]
    #[ignore = "measures completed searches with real egui input; no native window"]
    fn search_typing_measurement() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        let wait = |app: &mut App| {
            let start = Instant::now();
            while app.indexes[0].index_busy || app.panes[0].search_busy {
                app.poll();
                assert!(start.elapsed() < Duration::from_secs(15));
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        let start = Instant::now();
        app.request_index(
            IndexCommand::Load("artifacts/rea-folder.reaidx".into()),
            "measurement".into(),
        );
        wait(&mut app);
        let load_ms = start.elapsed().as_secs_f64() * 1000.;
        for _ in 0..5 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(("search-query", Pane::Left))));
        let mut samples = Vec::new();
        let mut blank_frames = 0;
        let mut expected_nonempty_frames = 0;
        for n in 0..102 {
            let query = ["r", "re", "rea", "re", "r", ""][n % 6];
            let previous_rows = app.panes[0].view.as_ref().unwrap().rows.len();
            let start = Instant::now();
            let mut raw = input(None);
            if n % 6 < 3 {
                raw.events
                    .push(egui::Event::Text(["r", "e", "a"][n % 6].into()));
            } else {
                raw = input(Some((egui::Key::Backspace, egui::Modifiers::NONE)));
            }
            let _ = draw(&ctx, &mut app, raw);
            assert_eq!(app.panes[0].query, query);
            assert!(app.panes[0].search_busy);
            if previous_rows > 0 {
                expected_nonempty_frames += 1;
                if app.panes[0].row_hits.is_empty() {
                    blank_frames += 1;
                }
            }
            wait(&mut app);
            let mut released = input(None);
            for key in [egui::Key::A, egui::Key::Backspace] {
                released.events.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let _ = draw(&ctx, &mut app, released);
            samples.push(start.elapsed().as_secs_f64() * 1000.);
        }
        let first = samples[0];
        let mut warm = samples[6..].to_vec();
        warm.sort_by(f64::total_cmp);
        println!(
            "{}",
            serde_json::json!({"snapshot_entries":app.panes[0].view.as_ref().unwrap().entries,"snapshot_load_and_initial_search_ms":load_ms,"completed_input_edits":samples.len(),"first_edit_ms":first,"warm_median_ms":warm[warm.len()/2],"warm_max_ms":warm[warm.len()-1],"frames_with_previous_rows":expected_nonempty_frames,"blank_pending_frames":blank_frames,"samples_ms":samples,"scope":"debug build, real egui input/meshes + background searches, no GPU presentation; OS caches not cleared"})
        );
        assert_eq!(blank_frames, 0);
    }

    #[test]
    fn typing_keeps_published_rows_and_geometry_until_latest_result_arrives() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        for _ in 0..5 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        app.panes[0].selection.single(1);
        let before = draw(&ctx, &mut app, input(None));
        let rects: Vec<_> = app.panes[0].row_hits.iter().map(|h| h.rect).collect();
        let count = text_rect(&before.shapes, "2개 결과");
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(("search-query", Pane::Left))));
        for letter in ["a", "b", "c"] {
            let mut raw = input(None);
            raw.events.push(egui::Event::Text(letter.into()));
            let out = draw(&ctx, &mut app, raw);
            assert!(app.panes[0].search_busy);
            assert_eq!(app.panes[0].view.as_ref().unwrap().total, 2);
            assert_eq!(app.panes[0].selection.focus, Some(1));
            assert_eq!(
                app.panes[0]
                    .row_hits
                    .iter()
                    .map(|h| h.rect)
                    .collect::<Vec<_>>(),
                rects
            );
            assert_eq!(text_rect(&out.shapes, "2개 결과"), count);
            assert!(text_rect(&out.shapes, "a.txt").is_positive());
        }
        assert_eq!(app.panes[0].query, "abc");
        let mut next = fixture().panes[0].view.take().unwrap();
        next.rows.clear();
        next.total = 0;
        app.panes[0].apply(Ok(next));
        let _ = draw(&ctx, &mut app, input(None));
        assert!(app.panes[0].row_hits.is_empty());
        assert!(app.panes[0].selection.items.is_empty());
        assert!(!app.panes[0].search_busy);
    }

    #[test]
    fn split_panes_keep_queries_and_shortcuts_independent() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        app.prefs.split = true;
        app.panes[0].query = "left".into();
        app.panes[1].query = "right".into();
        let _ = draw(&ctx, &mut app, input(None));
        assert!(app.panes.iter().all(|p| p.list_rect.is_some()));
        assert!(app.panes[0].list_rect.unwrap().right() < app.panes[1].list_rect.unwrap().left());
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::F6, egui::Modifiers::NONE))),
        );
        assert_eq!(app.focused, Pane::Right);
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new(("search-query", Pane::Right)))));
        let mut text = input(None);
        text.events.push(egui::Event::Text("X".into()));
        let _ = draw(&ctx, &mut app, text);
        assert_eq!(app.panes[0].query, "left");
        assert_eq!(app.panes[1].query, "rightX");
    }
    fn pointer(pos: egui::Pos2, pressed: Option<bool>) -> egui::RawInput {
        let mut raw = input(None);
        raw.events.push(egui::Event::PointerMoved(pos));
        if let Some(pressed) = pressed {
            raw.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        raw
    }
    #[test]
    fn blank_space_drag_selects_rows_without_starting_file_transfer() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        let _ = draw(&ctx, &mut app, input(None));
        let rect = app.panes[0].list_rect.unwrap();
        let hits = &app.panes[0].row_hits;
        let start = egui::pos2(rect.right() - 45., hits[1].rect.bottom() + 30.);
        let end = egui::pos2(rect.right() - 90., hits[0].rect.center().y);
        let _ = draw(&ctx, &mut app, pointer(start, Some(true)));
        let _ = draw(&ctx, &mut app, pointer(end, None));
        assert_eq!(
            app.panes[0].selection.items,
            std::collections::BTreeSet::from([0, 1])
        );
        let _ = draw(&ctx, &mut app, pointer(end, Some(false)));
        assert!(app.drag.is_none());
        assert!(app.panes[0].gesture.rectangle.is_none());
    }
    #[cfg(windows)]
    #[test]
    fn dragging_selected_files_between_panes_copies_and_shift_moves_real_files() {
        for cut in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let destination = dir.path().join("destination");
            std::fs::create_dir(&destination).unwrap();
            for name in ["one.txt", "two.txt"] {
                std::fs::write(dir.path().join(name), name).unwrap();
            }
            let ctx = egui::Context::default();
            let mut app = fixture();
            app.prefs.split = true;
            app.panes[0].query = "file: ext:txt".into();
            app.panes[1].query = "folder: destination".into();
            app.request_index(IndexCommand::Folder(dir.path().into()), "test".into());
            let wait = |app: &mut App| {
                let start = Instant::now();
                loop {
                    app.poll();
                    if !app.indexes[0].index_busy
                        && app.panes.iter().all(|p| !p.search_busy)
                        && app.pending.is_empty()
                    {
                        break;
                    }
                    assert!(
                        start.elapsed() < Duration::from_secs(10),
                        "{} / {}",
                        app.indexes[0].index_status,
                        app.action_status
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
            };
            wait(&mut app);
            assert_eq!(app.panes[0].view.as_ref().unwrap().rows.len(), 2);
            assert_eq!(app.panes[1].view.as_ref().unwrap().rows.len(), 1);
            app.panes[0].selection.all(2);
            let _ = draw(&ctx, &mut app, input(None));
            let source = app.panes[0].row_hits[0].name.center();
            let target = app.panes[1].row_hits[0].name.center();
            let _ = draw(&ctx, &mut app, pointer(source, Some(true)));
            let _ = draw(&ctx, &mut app, pointer(source + egui::vec2(15., 0.), None));
            assert_eq!(app.drag.as_ref().unwrap().paths.len(), 2);
            // In-flight transfers own the selection snapshot independently of later UI selection.
            app.panes[0].selection.clear();
            let _ = draw(&ctx, &mut app, pointer(target, None));
            let mut release = pointer(target, Some(false));
            if cut {
                release
                    .events
                    .insert(0, egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
            }
            let _ = draw(&ctx, &mut app, release);
            assert!(app.drag.is_none());
            wait(&mut app);
            assert_eq!(app.action_status, "파일 작업 완료");
            for name in ["one.txt", "two.txt"] {
                assert_eq!(
                    std::fs::read(destination.join(name)).unwrap(),
                    name.as_bytes()
                );
                assert_eq!(dir.path().join(name).exists(), !cut);
            }
            assert_eq!(app.focused, Pane::Right);
        }
    }
    #[test]
    fn dragging_scrollbars_never_starts_box_selection_or_file_drag() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        app.prefs.split = true;
        app.panes[0].settings.details = true;
        let view = app.panes[0].view.as_mut().unwrap();
        view.rows = (0..300)
            .map(|n| Row {
                name: format!("file-{n:03}.txt"),
                path: Some(format!(r"C:\sample\file-{n:03}.txt").into()),
                directory: false,
                details: None,
            })
            .collect();
        app.panes[0].selection.single(2);
        // Layout and scrollbar animations settle before input, as in a visible window.
        for _ in 0..5 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        for horizontal in [false, true] {
            let rect = app.panes[0].list_rect.unwrap();
            let start = if horizontal {
                egui::pos2(rect.left() + 15., rect.bottom() + 6.)
            } else {
                egui::pos2(rect.right() + 6., rect.top() + 15.)
            };
            let end = if horizontal {
                start + egui::vec2(80., -8.)
            } else {
                start + egui::vec2(-8., 80.)
            };
            let before = app.panes[0].scroll_offset;
            let _ = draw(&ctx, &mut app, pointer(start, Some(true)));
            let _ = draw(&ctx, &mut app, pointer(end, None));
            assert!(!app.panes[0].gesture.active());
            assert!(app.drag.is_none());
            assert_eq!(
                app.panes[0].selection.items,
                std::collections::BTreeSet::from([2])
            );
            let after = app.panes[0].scroll_offset;
            assert!(
                if horizontal {
                    after.x > before.x
                } else {
                    after.y > before.y
                },
                "scrollbar failed to scroll: {before:?} -> {after:?}, start={start:?}"
            );
            let _ = draw(&ctx, &mut app, pointer(end, Some(false)));
        }
    }
    #[test]
    fn visible_rows_receive_file_and_recursive_folder_sizes_then_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("nested");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(dir.path().join("file.txt"), b"12345").unwrap();
        std::fs::write(folder.join("child.bin"), b"123456").unwrap();
        let ctx = egui::Context::default();
        let mut app = fixture();
        let wait = |app: &mut App, expected| {
            let start = Instant::now();
            loop {
                app.poll();
                let _ = draw(&ctx, app, input(None));
                if let Some(view) = &app.panes[0].view {
                    let row = view.rows.iter().find(|r| r.name == "nested");
                    if let Some(d) = row
                        .and_then(|r| r.path.as_ref())
                        .and_then(|p| app.panes[0].decorations.get(p))
                        && d.size
                            .as_ref()
                            .is_some_and(|s| s.complete && s.bytes == expected)
                    {
                        #[cfg(windows)]
                        assert!(d.icon.is_some(), "{:?}", d.icon_note);
                        break;
                    }
                }
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "metadata failed: {}",
                    app.indexes[0].index_status
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        app.request_index(IndexCommand::Folder(dir.path().into()), "test".into());
        wait(&mut app, 6);
        let row = app.panes[0]
            .view
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .find(|r| r.name == "file.txt")
            .unwrap();
        assert_eq!(
            app.panes[0].decorations[row.path.as_ref().unwrap()]
                .size
                .as_ref()
                .unwrap()
                .bytes,
            5
        );
        std::fs::write(folder.join("child.bin"), b"changed bytes").unwrap();
        app.request_index(IndexCommand::Refresh, "test refresh".into());
        wait(&mut app, 13);
    }
    #[test]
    fn arrow_selection_keeps_visible_rows_stationary_without_requery() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        app.panes[0].view.as_mut().unwrap().rows = (0..120)
            .map(|n| Row {
                name: format!("row-{n:03}.txt"),
                path: Some(format!(r"C:\rea-navigation-fixture\row-{n:03}.txt").into()),
                directory: false,
                details: None,
            })
            .collect();
        for _ in 0..5 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        let target = app.panes[0].row_hits[4].name.center();
        let _ = draw(&ctx, &mut app, pointer(target, Some(true)));
        let _ = draw(&ctx, &mut app, pointer(target, Some(false)));
        assert_eq!(app.panes[0].selection.focus, Some(4));
        let before_offset = app.panes[0].scroll_offset;
        let before_rows: Vec<_> = app.panes[0]
            .row_hits
            .iter()
            .map(|h| (h.index, h.rect.top()))
            .collect();
        let metadata = app.panes[0].metadata_id;
        let search = app.panes[0].search_id;
        let revision = app.indexes[0].revision;
        let index = app.indexes[0].index_id;
        let start = Instant::now();
        for n in 0..100 {
            let key = if n % 2 == 0 {
                egui::Key::ArrowDown
            } else {
                egui::Key::ArrowUp
            };
            let _ = draw(&ctx, &mut app, input(Some((key, egui::Modifiers::NONE))));
            let _ = draw(&ctx, &mut app, input(None));
        }
        let after_rows: Vec<_> = app.panes[0]
            .row_hits
            .iter()
            .map(|h| (h.index, h.rect.top()))
            .collect();
        println!(
            "{}",
            serde_json::json!({"completed_arrow_moves":100,"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"offset_before":[before_offset.x,before_offset.y],"offset_after":[app.panes[0].scroll_offset.x,app.panes[0].scroll_offset.y],"metadata_requests":app.panes[0].metadata_id-metadata,"search_requests":app.panes[0].search_id-search,"index_requests":app.indexes[0].index_id-index})
        );
        assert_eq!(app.panes[0].selection.focus, Some(4));
        assert_eq!(app.panes[0].search_id, search);
        assert_eq!(app.indexes[0].revision, revision);
        assert_eq!(app.indexes[0].index_id, index);
        assert_eq!(app.panes[0].scroll_offset, before_offset);
        assert_eq!(after_rows, before_rows);
        assert_eq!(app.panes[0].metadata_id, metadata);
    }
    #[test]
    fn arrow_navigation_scrolls_only_enough_to_reveal_offscreen_rows() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        app.panes[0].view.as_mut().unwrap().rows = (0..120)
            .map(|n| Row {
                name: format!("row-{n:03}"),
                path: None,
                directory: false,
                details: None,
            })
            .collect();
        for _ in 0..5 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        let viewport = app.panes[0].list_rect.unwrap();
        let last = app.panes[0]
            .row_hits
            .iter()
            .rfind(|h| h.rect.bottom() <= viewport.bottom())
            .unwrap()
            .index;
        app.panes[0].selection.single(last);
        app.panes[0].results_focus = true;
        let before = app.panes[0].scroll_offset;
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::ArrowDown, egui::Modifiers::NONE))),
        );
        for _ in 0..3 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        let pane = &app.panes[0];
        let hit = pane.row_hits.iter().find(|h| h.index == last + 1).unwrap();
        let rect = pane.list_rect.unwrap();
        assert!(hit.rect.top() >= rect.top() - 0.1 && hit.rect.bottom() <= rect.bottom() + 0.1);
        assert!(pane.scroll_offset.y > before.y && pane.scroll_offset.y - before.y <= 40.);
        // At the upper boundary, move back by one row rather than aligning every row to the top.
        app.panes[0].scroll_wanted = Some(300.);
        for _ in 0..3 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        let rect = app.panes[0].list_rect.unwrap();
        let first = app.panes[0]
            .row_hits
            .iter()
            .find(|h| h.rect.top() >= rect.top() - 0.1)
            .unwrap()
            .index;
        app.panes[0].selection.single(first);
        let before = app.panes[0].scroll_offset;
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::ArrowUp, egui::Modifiers::NONE))),
        );
        for _ in 0..3 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        let pane = &app.panes[0];
        let hit = pane.row_hits.iter().find(|h| h.index == first - 1).unwrap();
        let rect = pane.list_rect.unwrap();
        assert!(hit.rect.top() >= rect.top() - 0.1 && hit.rect.bottom() <= rect.bottom() + 0.1);
        assert!(pane.scroll_offset.y < before.y && before.y - pane.scroll_offset.y <= 40.);
        assert_eq!(pane.scroll_offset.x, before.x);
    }
    #[cfg(windows)]
    #[test]
    fn refreshing_visible_metadata_keeps_last_icon_until_replacement_arrives() {
        for revision_change in [false, true] {
            let ctx = egui::Context::default();
            let mut app = fixture();
            let _ = draw(&ctx, &mut app, input(None));
            let start = Instant::now();
            while app.panes[0].metadata_busy {
                app.update_metadata(&ctx);
                assert!(start.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(1));
            }
            let path = app.panes[0].metadata_paths[0].clone();
            let old = Instant::now() - Duration::from_secs(20);
            let icon = Arc::new(crate::shell_icon::Icon {
                image: egui::ColorImage::filled([32, 32], Color32::WHITE),
                image_index: 1,
                overlay_index: 0,
            });
            let texture = ctx.load_texture(
                "last-measured-icon",
                icon.image.clone(),
                egui::TextureOptions::LINEAR,
            );
            let id = texture.id();
            let request = app.panes[0].metadata_id;
            app.panes[0]
                .icons
                .insert(path.clone(), (icon.clone(), texture));
            app.panes[0].decorations.insert(
                path.clone(),
                crate::row_metadata::Decoration {
                    details: Details {
                        size: Some(7),
                        modified: None,
                        error: None,
                    },
                    size: Some(crate::row_metadata::Size {
                        bytes: 7,
                        complete: true,
                        note: None,
                    }),
                    size_note: None,
                    icon: Some(icon),
                    icon_note: None,
                    measured: old,
                },
            );
            if revision_change {
                app.indexes[0].revision += 1;
                app.metadata_epoch[0] = app.metadata_epoch[0].wrapping_add(1);
            } else {
                app.panes[0].metadata_at = Some(old);
            }
            app.update_metadata(&ctx);
            assert!(app.panes[0].metadata_busy);
            assert_eq!(app.panes[0].metadata_id, request + 1);
            assert_eq!(app.panes[0].icons[&path].1.id(), id);
            let last = &app.panes[0].decorations[&path];
            assert_eq!(last.size.as_ref().unwrap().bytes, 7);
            assert_eq!(
                last.measured, old,
                "Do not report the retained display as fresh"
            );
            let _ = draw(&ctx, &mut app, input(None));
        }
    }
    #[test]
    fn saved_folder_startup_preserves_explicit_source_choices() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("저장 폴더");
        let explicit = dir.path().join("명시 폴더");
        let cache = dir.path().join("index.reaidx");
        let prefs = Preferences {
            last_folder: Some(saved.clone()),
            ..Default::default()
        };
        assert!(
            matches!(startup_source(None,None,None,&prefs).unwrap(),IndexCommand::Folder(p) if p==saved)
        );
        assert!(
            matches!(startup_source(Some(explicit.clone()),None,None,&prefs).unwrap(),IndexCommand::Folder(p) if p==explicit)
        );
        assert!(matches!(
            startup_source(None, Some('E'), None, &prefs).unwrap(),
            IndexCommand::Drive('E')
        ));
        assert!(
            matches!(startup_source(Some(explicit),Some('E'),Some(cache.clone()),&prefs).unwrap(),IndexCommand::Load(p) if p==cache)
        );
        assert!(
            matches!(startup_source(None,None,None,&Preferences::default()).unwrap(),IndexCommand::Folder(p) if p==std::env::current_dir().unwrap())
        );
    }
    #[test]
    fn last_successful_folder_is_saved_on_shutdown_and_failed_index_does_not_replace_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("실제 색인 폴더");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("one.txt"), b"one").unwrap();
        let settings = dir.path().join("data/preferences.json");
        let (store, _) = Store::load(settings.clone()).unwrap();
        let mut app = fixture();
        app.services = Services::new(Some(store), Arc::new(|| {})).unwrap();
        app.settings_enabled = true;
        let wait = |app: &mut App| {
            let start = Instant::now();
            loop {
                app.poll();
                if !app.indexes[0].index_busy {
                    break;
                }
                assert!(start.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        app.request_index(IndexCommand::Folder(root.clone()), "test".into());
        app.indexes[0].folder = "텍스트 상자만 바뀐 경로".into();
        wait(&mut app);
        let actual = root.canonicalize().unwrap();
        assert_eq!(app.prefs.last_folder.as_ref(), Some(&actual));
        app.request_index(
            IndexCommand::Folder(dir.path().join("missing")),
            "failed test".into(),
        );
        wait(&mut app);
        assert_eq!(app.prefs.last_folder.as_ref(), Some(&actual));
        drop(app);
        let (_, loaded) = Store::load(settings).unwrap();
        assert_eq!(loaded.last_folder.as_ref(), Some(&actual));
        assert!(
            matches!(startup_source(None,None,None,&loaded).unwrap(),IndexCommand::Folder(p) if p==actual)
        );
    }
    #[test]
    fn refreshing_keeps_rows_and_tracks_selection_changes_until_publication() {
        let mut app = fixture();
        let pane = &mut app.panes[0];
        pane.selection.click(0, false, false, false);
        pane.request(true);
        assert_eq!(pane.view.as_ref().unwrap().rows.len(), 2);
        pane.selection.click(1, false, false, false);
        let mut next = fixture().panes[0].view.take().unwrap();
        next.rows.swap(0, 1);
        pane.apply(Ok(next));
        assert_eq!(pane.selection.focus, Some(0));
        assert_eq!(
            pane.selected_paths().unwrap(),
            vec![PathBuf::from(r"C:\sample\folder")]
        );
        pane.request(false);
        assert_eq!(pane.view.as_ref().unwrap().rows.len(), 2);
        assert_eq!(pane.selection.focus, Some(0));
        // A refresh arriving before this new query completes must still reset selection.
        pane.request(true);
        let next = fixture().panes[0].view.take().unwrap();
        pane.apply(Ok(next));
        assert!(pane.selection.items.is_empty());
        assert_eq!(pane.scroll_wanted, Some(0.));
    }

    #[test]
    #[ignore = "exports actual egui meshes for visual review without a native window"]
    fn ui_visual_previews() {
        let outdir = std::path::Path::new("artifacts/ui");
        std::fs::create_dir_all(outdir).unwrap();
        for (name, size, split, manager) in [
            ("single", [1400, 880], false, false),
            ("split", [1400, 880], true, false),
            ("independent", [1400, 880], true, false),
            ("independent-manager", [1100, 760], true, true),
            ("compact", [980, 560], false, false),
            ("index-manager", [1100, 760], false, true),
            ("context-menu", [1100, 760], false, false),
            ("settings", [1100, 760], false, false),
            ("settings-compact", [980, 560], false, false),
            ("sort-details", [1400, 880], false, false),
            ("loading-search", [980, 560], false, false),
            ("loading-index", [980, 560], false, false),
        ] {
            let ctx = egui::Context::default();
            crate::appearance::configure(&ctx);
            let mut app = fixture();
            let mut paths: Vec<_> = std::fs::read_dir("src")
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            paths.sort();
            let rows: Vec<_> = paths
                .into_iter()
                .map(|p| Row {
                    name: p.file_name().unwrap().to_string_lossy().into_owned(),
                    path: Some(p.canonicalize().unwrap()),
                    directory: p.is_dir(),
                    details: Some(Details::read(&p)),
                })
                .collect();
            for pane in &mut app.panes {
                pane.view = Some(View {
                    total: rows.len(),
                    entries: 148_236,
                    memory: 8_519_680,
                    elapsed_ms: 0.84,
                    source: "folder".into(),
                    coverage: 0,
                    rows: rows
                        .iter()
                        .map(|r| Row {
                            name: r.name.clone(),
                            path: r.path.clone(),
                            directory: r.directory,
                            details: r.path.as_deref().map(Details::read),
                        })
                        .collect(),
                });
                pane.selection.click(2, false, false, false);
                pane.settings.details = false;
            }
            app.panes[1].query = "gui".into();
            app.prefs.split = split;
            if name.starts_with("independent") {
                app.prefs.sync_indexes = false;
                let root = std::env::current_dir().unwrap().join("examples");
                app.indexes[1].folder = root.to_string_lossy().into_owned();
                app.indexes[1].active_source =
                    Some(crate::index::Source::Folder { root: root.clone() });
                app.indexes[1].ready = true;
                app.panes[1].query.clear();
                let rows: Vec<_> = std::fs::read_dir(&root)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .map(|p| Row {
                        name: p.file_name().unwrap().to_string_lossy().into_owned(),
                        path: Some(p.clone()),
                        directory: p.is_dir(),
                        details: Some(Details::read(&p)),
                    })
                    .collect();
                app.panes[1].view = Some(View {
                    total: rows.len(),
                    entries: rows.len(),
                    memory: 4096,
                    elapsed_ms: 0.1,
                    source: "folder".into(),
                    coverage: 0,
                    rows,
                });
                app.index_target = Pane::Right;
            }

            app.settings_tab = name.starts_with("settings");
            if name == "sort-details" {
                app.panes[0].settings.details = true;
                app.panes[0].settings.order.column = crate::index::SortColumn::Size;
            }
            app.index_dialog = manager;
            if manager {
                app.prefs.refresh_interval_secs = 10;
                app.indexes[0].live = true;
            }
            app.indexes[0].index_status = "검색 준비 완료".into();
            if name.starts_with("loading-") {
                app.panes[0].view = None;
                app.panes[0].selection.clear();
                if name == "loading-index" {
                    app.indexes[0].index_busy = true;
                    app.indexes[0].index_status = "폴더를 색인하는 중...".into();
                } else {
                    app.panes[0].search_busy = true;
                    app.panes[0].search_started = Some(Instant::now() - Duration::from_millis(200));
                }
            }
            let mut renderer = crate::ui_preview::Renderer::default();
            for frame in 0..5 {
                let mut raw = egui::RawInput {
                    time: Some(frame as f64 * 0.25),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(size[0] as f32, size[1] as f32),
                    )),
                    ..Default::default()
                };
                if name == "context-menu" && (frame == 2 || frame == 3) {
                    let pos = app.panes[0].row_hits[2].name.center();
                    raw.events.push(egui::Event::PointerMoved(pos));
                    raw.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Secondary,
                        pressed: frame == 2,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                let output = ctx.run_ui(raw, |ui| app.show(ui));
                renderer.save(&ctx, output, size, &outdir.join(format!("{name}.rgba")));
                std::thread::sleep(Duration::from_millis(40));
            }
            if !manager && !app.settings_tab && !name.starts_with("loading-") {
                assert!(app.panes[0].row_hits.len() >= 7);
                let rect = app.panes[0].list_rect.unwrap();
                assert!(rect.right() <= size[0] as f32);
                assert!(rect.bottom() <= size[1] as f32);
            }
        }
    }
    #[test]
    fn search_menus_preserve_keyboard_ownership_and_history_clear_does_not_search() {
        fn click(ctx: &egui::Context, app: &mut App, text: &str) {
            let out = draw(ctx, app, input(None));
            let pos = text_rect(&out.shapes, text).center();
            let _ = draw(ctx, app, pointer(pos, None));
            let _ = draw(ctx, app, pointer(pos, Some(true)));
            let _ = draw(ctx, app, pointer(pos, Some(false)));
        }
        let ctx = egui::Context::default();
        let mut app = fixture();
        app.prefs.history.push("saved search".into());
        app.panes[0].selection.single(0);
        click(&ctx, &mut app, "검색 옵션");
        assert!(egui::Popup::is_any_open(&ctx));
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::ArrowDown, egui::Modifiers::NONE))),
        );
        assert_eq!(
            app.panes[0].selection.focus,
            Some(0),
            "menu arrows must not navigate files"
        );
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::Escape, egui::Modifiers::NONE))),
        );
        click(&ctx, &mut app, "최근 · 즐겨찾기");
        let id = app.panes[0].search_id;
        click(&ctx, &mut app, "최근 검색 지우기");
        assert!(app.prefs.history.is_empty());
        assert_eq!(app.panes[0].search_id, id);
        assert_eq!(app.panes[0].view.as_ref().unwrap().rows.len(), 2);
        click(&ctx, &mut app, "색인 관리");
        assert!(app.index_dialog);
        assert!(app.has_modal());
        let _ = draw(
            &ctx,
            &mut app,
            input(Some((egui::Key::ArrowDown, egui::Modifiers::NONE))),
        );
        assert_eq!(app.panes[0].selection.focus, Some(0));
    }
    fn text_rect(shapes: &[egui::epaint::ClippedShape], text: &str) -> egui::Rect {
        fn find(shape: &egui::Shape, text: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(t) if t.galley.job.text == text => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                }
                egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, text)),
                _ => None,
            }
        }
        shapes
            .iter()
            .find_map(|s| find(&s.shape, text))
            .unwrap_or_else(|| panic!("missing control: {text}"))
    }
    #[test]
    fn context_menu_shortcuts_share_left_edge_and_the_whole_row_is_clickable() {
        for directory in [false, true] {
            let ctx = egui::Context::default();
            let mut app = fixture();
            for _ in 0..3 {
                let _ = draw(&ctx, &mut app, input(None));
            }
            let row = usize::from(directory);
            let pos = app.panes[0].row_hits[row].name.center();
            let _ = draw(&ctx, &mut app, pointer(pos, None));
            for pressed in [true, false] {
                let mut raw = pointer(pos, Some(pressed));
                for event in &mut raw.events {
                    if let egui::Event::PointerButton { button, .. } = event {
                        *button = egui::PointerButton::Secondary;
                    }
                }
                let _ = draw(&ctx, &mut app, raw);
            }
            let out = draw(&ctx, &mut app, input(None));
            let x = text_rect(&out.shapes, "Ctrl+C").left();
            for shortcut in [
                "Ctrl+X",
                "Ctrl+V",
                "F2",
                "Enter",
                "Ctrl+Shift+C",
                "Ctrl+Enter",
            ] {
                let rect = text_rect(&out.shapes, shortcut);
                assert!(
                    (rect.left() - x).abs() < 0.1,
                    "shortcut {shortcut} starts at {} instead of {x}",
                    rect.left()
                );
            }
            let label = if directory {
                "이 폴더에 붙여넣기"
            } else {
                "이 파일의 폴더에 붙여넣기"
            };
            assert!(text_rect(&out.shapes, label).right() + 10. < x);
            let shortcut = text_rect(&out.shapes, "F2").center();
            let _ = draw(&ctx, &mut app, pointer(shortcut, None));
            let _ = draw(&ctx, &mut app, pointer(shortcut, Some(true)));
            let _ = draw(&ctx, &mut app, pointer(shortcut, Some(false)));
            assert!(
                app.rename_dialog.is_some(),
                "shortcut column must activate the same row action"
            );
        }
    }
    #[test]
    fn configured_refresh_interval_controls_due_time_and_never_overlaps_an_index_job() {
        let mut app = fixture();
        app.indexes[0].live = true;
        app.prefs.refresh_interval_secs = 17;
        let start = app.indexes[0].last_refresh;
        assert!(!app.auto_refresh_due(Pane::Left, start + Duration::from_secs(16)));
        assert!(app.auto_refresh_due(Pane::Left, start + Duration::from_secs(17)));
        app.indexes[0].index_busy = true;
        assert!(!app.auto_refresh_due(Pane::Left, start + Duration::from_secs(100)));
        app.indexes[0].index_busy = false;
        app.indexes[0].live = false;
        assert!(!app.auto_refresh_due(Pane::Left, start + Duration::from_secs(100)));
        app.indexes[0].live = true;
        app.indexes[0].ready = false;
        assert!(!app.auto_refresh_due(Pane::Left, start + Duration::from_secs(100)));
        app.indexes[0].ready = true;
        app.prefs.refresh_interval_secs = u32::MAX;
        assert!(
            !app.auto_refresh_due(Pane::Left, start + Duration::from_secs(u32::MAX as u64 - 1))
        );
        assert!(app.auto_refresh_due(Pane::Left, start + Duration::from_secs(u32::MAX as u64)));
    }
    #[test]
    fn split_index_modes_publish_to_the_right_pane_and_restore_each_folder() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("왼쪽");
        let right = dir.path().join("오른쪽");
        let next = dir.path().join("오른쪽 다음");
        for (root, name) in [
            (&left, "left-only.txt"),
            (&right, "right-only.txt"),
            (&next, "next-only.txt"),
        ] {
            std::fs::create_dir(root).unwrap();
            std::fs::write(root.join(name), b"data").unwrap();
        }
        let config = dir.path().join("data/preferences.json");
        let (store, _) = Store::load(config.clone()).unwrap();
        let mut app = fixture();
        app.services = Services::new(Some(store), Arc::new(|| {})).unwrap();
        app.settings_enabled = true;
        app.prefs.split = true;
        app.indexes[0].folder = left.to_string_lossy().into_owned();
        app.indexes[1].folder = right.to_string_lossy().into_owned();
        let wait = |app: &mut App| {
            let start = Instant::now();
            loop {
                app.poll();
                if !app.indexes.iter().any(|x| x.index_busy)
                    && !app.panes.iter().any(|x| x.search_busy)
                {
                    break;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "index/search completion timed out"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        let has = |app: &App, pane: Pane, name: &str| {
            app.panes[pane.slot()]
                .view
                .as_ref()
                .is_some_and(|v| v.rows.iter().any(|r| r.name == name))
        };
        app.request_index_in(
            Pane::Left,
            IndexCommand::Folder(left.clone()),
            "left".into(),
        );
        wait(&mut app);
        assert!(has(&app, Pane::Left, "left-only.txt") && has(&app, Pane::Right, "left-only.txt"));
        app.set_index_sync(false);
        wait(&mut app);
        assert!(has(&app, Pane::Left, "left-only.txt") && !has(&app, Pane::Left, "right-only.txt"));
        assert!(
            has(&app, Pane::Right, "right-only.txt") && !has(&app, Pane::Right, "left-only.txt")
        );
        let left_revision = app.indexes[0].revision;
        app.request_index_in(
            Pane::Right,
            IndexCommand::Folder(next.clone()),
            "new right".into(),
        );
        app.set_index_sync(true);
        let pending_right = app.indexes[1].index_id;
        app.set_index_sync(false);
        assert_eq!(
            app.indexes[1].index_id, pending_right,
            "mode switch cancelled explicit pending folder choice"
        );
        app.set_index_sync(true);
        wait(&mut app);
        assert!(
            has(&app, Pane::Right, "left-only.txt"),
            "late independent publication replaced shared pane"
        );
        assert!(!has(&app, Pane::Right, "next-only.txt"));
        assert_eq!(app.indexes[0].revision, left_revision);
        app.set_index_sync(false);
        wait(&mut app);
        assert!(has(&app, Pane::Right, "next-only.txt"));
        let before = app.prefs.right_folder.clone();
        app.request_index_in(
            Pane::Right,
            IndexCommand::Folder(dir.path().join("missing")),
            "failure".into(),
        );
        wait(&mut app);
        assert!(has(&app, Pane::Right, "next-only.txt"));
        assert!(has(&app, Pane::Left, "left-only.txt"));
        assert_eq!(app.prefs.right_folder, before);
        std::fs::write(left.join("left-new.txt"), b"l").unwrap();
        std::fs::write(next.join("right-new.txt"), b"r").unwrap();
        app.refresh_after_files();
        wait(&mut app);
        assert!(has(&app, Pane::Left, "left-new.txt") && has(&app, Pane::Right, "right-new.txt"));
        assert!(app.metadata_epoch[0] > 0 && app.metadata_epoch[1] > 1);
        drop(app);
        let (_, saved) = Store::load(config).unwrap();
        assert!(!saved.sync_indexes);
        assert_eq!(saved.last_folder, Some(left.canonicalize().unwrap()));
        assert_eq!(saved.right_folder, Some(next.canonicalize().unwrap()));
    }
    #[test]
    fn split_index_checkbox_routes_the_manager_to_each_pane() {
        let ctx = egui::Context::default();
        let mut app = fixture();
        app.prefs.split = true;
        let out = draw(&ctx, &mut app, input(None));
        let pos = text_rect(&out.shapes, "색인 공유").center();
        let _ = draw(&ctx, &mut app, pointer(pos, None));
        let _ = draw(&ctx, &mut app, pointer(pos, Some(true)));
        let _ = draw(&ctx, &mut app, pointer(pos, Some(false)));
        assert!(!app.prefs.sync_indexes);
        app.index_target = Pane::Right;
        app.index_dialog = true;
        for _ in 0..3 {
            let _ = draw(&ctx, &mut app, input(None));
        }
        let out = draw(&ctx, &mut app, input(None));
        assert!(text_rect(&out.shapes, "왼쪽 색인").is_positive());
        assert!(text_rect(&out.shapes, "오른쪽 색인").is_positive());
        app.index_dialog = false;
        let out = draw(&ctx, &mut app, input(None));
        let pos = text_rect(&out.shapes, "색인 공유").center();
        let _ = draw(&ctx, &mut app, pointer(pos, None));
        let _ = draw(&ctx, &mut app, pointer(pos, Some(true)));
        let _ = draw(&ctx, &mut app, pointer(pos, Some(false)));
        assert!(app.prefs.sync_indexes);
    }
    #[test]
    fn native_menu_input_captures_focus_first_and_ctrl_keeps_application_menu() {
        use crate::search_pane::Action;
        let ctx = egui::Context::default();
        let mut app = fixture();
        let pane = &mut app.panes[0];
        pane.selection.all(2);
        pane.selection.focus = Some(1);
        let draw = |pane: &mut SearchPane, prefs: &mut Preferences, raw: egui::RawInput| {
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    actions = pane.show(
                        ui,
                        PaneContext {
                            pane: Pane::Left,
                            focused: true,
                            split: false,
                            ready: true,
                            index_busy: false,
                            file_drag: false,
                            modal: false,
                            native_shell: true,
                            index_location: String::new(),
                        },
                        prefs,
                    );
                });
            });
            out.textures_delta.clear();
            actions
        };
        for _ in 0..3 {
            draw(pane, &mut app.prefs, input(None));
        }
        let pos = pane.row_hits[1].name.center();
        draw(pane, &mut app.prefs, pointer(pos, None));
        let mut actions = Vec::new();
        for pressed in [true, false] {
            let mut raw = pointer(pos, Some(pressed));
            for event in &mut raw.events {
                if let egui::Event::PointerButton { button, .. } = event {
                    *button = egui::PointerButton::Secondary;
                }
            }
            actions.extend(draw(pane, &mut app.prefs, raw));
        }
        let items = actions
            .into_iter()
            .find_map(|a| {
                if let Action::ShellMenu { items, .. } = a {
                    Some(items)
                } else {
                    None
                }
            })
            .expect("native menu action");
        assert_eq!(items.len(), 2);
        assert!(items[0].directory && items[0].path == std::path::Path::new(r"C:\sample\folder"));
        assert!(!items[1].directory);
        pane.selection.clear();
        assert_eq!(
            items.len(),
            2,
            "captured menu selection is independent of later UI selection"
        );
        pane.selection.single(0);
        let pos = pane.row_hits[0].name.center();
        let mut actions = Vec::new();
        for pressed in [true, false] {
            let mut raw = pointer(pos, Some(pressed));
            raw.events
                .insert(0, egui::Event::ModifiersChanged(egui::Modifiers::CTRL));
            for event in &mut raw.events {
                if let egui::Event::PointerButton {
                    button, modifiers, ..
                } = event
                {
                    *button = egui::PointerButton::Secondary;
                    *modifiers = egui::Modifiers::CTRL;
                }
            }
            actions.extend(draw(pane, &mut app.prefs, raw));
        }
        assert!(pane.application_menu);
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Action::ShellMenu { .. }))
        );
    }
}
