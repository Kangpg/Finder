use super::gui::{RESULT_ROW_HEIGHT, result_header, result_row};
use crate::{
    background::{Pane, SearchRequest, View},
    fileops::RenameSource,
    filters::Filter,
    preferences::{Preferences, SearchSettings},
    selection::Selection,
    services::Job,
};
use anyhow::{Context, Result, ensure};
use eframe::egui::{self, AtomExt, Color32, RichText};
use std::path::PathBuf;

#[derive(Clone, Copy)]
enum FileMenuItem {
    Copy,
    Cut,
    Paste,
    Rename,
    Open,
    CopyName,
    CopyPath,
    Reveal,
    OpenLocation,
    OpenParent,
    Edit,
    Info,
}
impl FileMenuItem {
    const ALL: [Self; 12] = [
        Self::Copy,
        Self::Cut,
        Self::Paste,
        Self::Rename,
        Self::Open,
        Self::CopyName,
        Self::CopyPath,
        Self::Reveal,
        Self::OpenLocation,
        Self::OpenParent,
        Self::Edit,
        Self::Info,
    ];
    fn text(self, directory: bool) -> (&'static str, &'static str) {
        match self {
            Self::Copy => (crate::i18n::text("복사", "Copy"), "Ctrl+C"),
            Self::Cut => (crate::i18n::text("잘라내기", "Cut"), "Ctrl+X"),
            Self::Paste => (
                if directory {
                    crate::i18n::text("이 폴더에 붙여넣기", "Paste into this folder")
                } else {
                    crate::i18n::text("이 파일의 폴더에 붙여넣기", "Paste into this file's folder")
                },
                "Ctrl+V",
            ),
            Self::Rename => (crate::i18n::text("이름 변경…", "Rename…"), "F2"),
            Self::Open => (crate::i18n::text("열기", "Open"), "Enter"),
            Self::CopyName => (crate::i18n::text("이름 복사", "Copy name"), ""),
            Self::CopyPath => (crate::i18n::text("경로 복사", "Copy path"), "Ctrl+Shift+C"),
            Self::Reveal => (
                crate::i18n::text("탐색기에서 보기", "Show in Explorer"),
                "Ctrl+Enter",
            ),
            Self::OpenLocation => (
                crate::i18n::text("해당 폴더 열기", "Open containing folder"),
                "",
            ),
            Self::OpenParent => (
                crate::i18n::text("상위 폴더 열기", "Open parent folder"),
                "",
            ),
            Self::Edit => (crate::i18n::text("편집 앱으로 열기", "Open in editor"), ""),
            Self::Info => (crate::i18n::text("속성", "Properties"), "Alt+Enter"),
        }
    }
}
struct FileMenu {
    directory: bool,
    label_width: f32,
    shortcut_width: f32,
    height: f32,
}
impl FileMenu {
    fn new(ui: &egui::Ui, directory: bool) -> Self {
        let font = egui::TextStyle::Button.resolve(ui.style());
        let measure = |text: &str| {
            ui.painter()
                .layout_no_wrap(text.into(), font.clone(), Color32::WHITE)
                .size()
                .x
        };
        let mut label_width = 0f32;
        let mut shortcut_width = 0f32;
        for item in FileMenuItem::ALL {
            let (label, shortcut) = item.text(directory);
            label_width = label_width.max(measure(label));
            shortcut_width = shortcut_width.max(measure(shortcut));
        }
        let height = ui.fonts_mut(|f| f.row_height(&font));
        Self {
            directory,
            label_width,
            shortcut_width,
            height,
        }
    }
    fn button(&self, ui: &mut egui::Ui, item: FileMenuItem, enabled: bool) -> egui::Response {
        let (label, shortcut) = item.text(self.directory);
        ui.add_enabled(
            enabled,
            egui::Button::new((
                RichText::new(label)
                    .atom_size(egui::vec2(self.label_width, self.height))
                    .atom_align(egui::Align2::LEFT_CENTER),
                RichText::new(shortcut)
                    .weak()
                    .atom_size(egui::vec2(self.shortcut_width, self.height))
                    .atom_align(egui::Align2::LEFT_CENTER),
            ))
            .gap(24.),
        )
    }
}

pub enum Action {
    Search(SearchRequest),
    Settings,
    Job(Job),
    Rename(Vec<RenameSource>),
    Help,
    Bookmarks {
        query: String,
        settings: SearchSettings,
    },
    Refresh,
    IndexManager,
    Focused,
    Message(crate::i18n::Message),
    Drag(Vec<PathBuf>),
    ShellMenu {
        items: Vec<RenameSource>,
        position: egui::Pos2,
        extended: bool,
    },
    Properties(Vec<RenameSource>),
}
pub struct PaneContext {
    pub pane: Pane,
    pub focused: bool,
    pub split: bool,
    pub ready: bool,
    pub index_busy: bool,
    pub modal: bool,
    pub file_drag: bool,
    pub native_shell: bool,
    pub index_location: String,
}
#[derive(Default)]
pub struct SearchPane {
    pub view: Option<View>,
    #[cfg(windows)]
    pub icons: std::collections::HashMap<
        PathBuf,
        (std::sync::Arc<crate::shell_icon::Icon>, egui::TextureHandle),
    >,
    pub decorations: std::collections::HashMap<PathBuf, crate::row_metadata::Decoration>,
    pub metadata_paths: Vec<PathBuf>,
    pub metadata_revision: u64,
    pub metadata_id: u64,
    pub metadata_busy: bool,
    pub metadata_at: Option<std::time::Instant>,
    pub query: String,
    pub settings: SearchSettings,
    pub selection: Selection,
    pub preserve_selection: bool,
    pub search_error: bool,
    pub results_focus: bool,
    pub application_menu: bool,
    pub reveal_row: Option<usize>,
    pub search_status: crate::i18n::Message,
    pub search_busy: bool,
    pub(crate) search_started: Option<std::time::Instant>,
    pub search_id: u64,
    pub list_rect: Option<egui::Rect>,
    pub row_hits: Vec<crate::list_drag::RowHit>,
    pub gesture: crate::list_drag::ListDrag,
    pub scroll_offset: egui::Vec2,
    pub scroll_wanted: Option<f32>,
}
impl SearchPane {
    pub fn new(settings: SearchSettings) -> Self {
        Self {
            settings,
            ..Default::default()
        }
    }
    pub fn request(&mut self, preserve: bool) -> SearchRequest {
        // Keep the published view intact until the latest request completes.
        // An index refresh cannot turn a pending query change into a selection-preserving refresh.
        self.preserve_selection = preserve && (!self.search_busy || self.preserve_selection);
        self.gesture.cancel();
        if !self.search_busy {
            self.search_started = Some(std::time::Instant::now());
        }
        self.search_busy = true;
        self.search_error = false;
        self.search_status = crate::i18n::Message::new("검색 중...", "Searching...");
        SearchRequest {
            query: self.query.clone(),
            options: self.settings.options,
            filter: self.settings.filter,
            order: self.settings.order,
            limit: 5000,
        }
    }
    pub fn apply(&mut self, result: Result<View>) {
        self.search_busy = false;
        self.search_started = None;
        match result {
            Ok(view) => {
                self.search_status = crate::msgf!(
                    "{}개 검색 결과 · {:.2} ms",
                    "{} results · {:.2} ms",
                    view.total,
                    view.elapsed_ms
                );
                if self.preserve_selection
                    && let Some(previous) = &self.view
                {
                    let saved = self.selection.save(&previous.rows);
                    self.selection.restore(saved, &view.rows);
                } else {
                    self.selection.clear();
                    self.reveal_row = None;
                    self.scroll_wanted = Some(0.);
                }
                self.gesture.cancel();
                self.preserve_selection = false;
                self.search_error = false;
                self.view = Some(view);
            }
            Err(e) => {
                self.search_error = true;
                self.preserve_selection = false;
                self.search_status = crate::i18n::Message::capture(|| format!("{e:#}"));
                self.view = None;
            }
        }
    }
    pub(crate) fn trim_metadata(&mut self, visible: &[PathBuf]) {
        let excess = self
            .decorations
            .len()
            .saturating_sub(crate::row_metadata::MAX_CACHE);
        if excess == 0 {
            return;
        }
        let mut retired: Vec<_> = self
            .decorations
            .iter()
            .filter(|(path, _)| !visible.contains(path))
            .map(|(path, value)| (value.measured, path.clone()))
            .collect();
        retired.sort_unstable();
        for (_, path) in retired.into_iter().take(excess) {
            self.decorations.remove(&path);
            #[cfg(windows)]
            self.icons.remove(&path);
        }
    }
    fn queue_search(&mut self, actions: &mut Vec<Action>) {
        actions.push(Action::Search(self.request(false)));
    }
    pub fn selected_paths(&self) -> Result<Vec<PathBuf>> {
        ensure!(
            !self.selection.items.is_empty(),
            crate::i18n::Message::new(
                "파일/폴더를 먼저 선택하세요",
                "Select a file or folder first"
            )
        );
        let rows = &self
            .view
            .as_ref()
            .context(crate::i18n::Message::new(
                "검색 결과가 없습니다",
                "No search results",
            ))?
            .rows;
        self.selection
            .items
            .iter()
            .map(|&n| {
                rows.get(n)
                    .and_then(|r| r.path.clone())
                    .context(crate::i18n::Message::new(
                        "선택 항목의 경로를 확인할 수 없습니다",
                        "The selected item's path is unavailable",
                    ))
            })
            .collect()
    }
    fn selected_path(&self) -> Option<PathBuf> {
        self.view
            .as_ref()?
            .rows
            .get(self.selection.focus.unwrap_or(0))?
            .path
            .clone()
    }
    fn copy_files(&mut self, cut: bool, actions: &mut Vec<Action>) {
        match self.selected_paths() {
            Ok(paths) => {
                #[cfg(windows)]
                let expected_sequence = crate::file_clipboard::sequence();
                #[cfg(not(windows))]
                let expected_sequence = 0;
                actions.push(Action::Job(Job::CopyFiles {
                    paths,
                    cut,
                    expected_sequence,
                }));
            }
            Err(e) => actions.push(Action::Message(crate::i18n::Message::capture(|| {
                format!("{e:#}")
            }))),
        }
    }
    fn paste_destination(&self) -> Option<PathBuf> {
        let row = self.view.as_ref()?.rows.get(self.selection.focus?)?;
        let path = row.path.as_ref()?;
        Some(if row.directory {
            path.clone()
        } else {
            path.parent()?.into()
        })
    }
    fn selected_sources(&self, focused_first: bool) -> Result<Vec<RenameSource>> {
        let paths = self.selected_paths()?;
        let rows = &self.view.as_ref().unwrap().rows;
        let mut items: Vec<_> = self
            .selection
            .items
            .iter()
            .zip(paths)
            .map(|(&n, path)| RenameSource {
                path,
                directory: rows[n].directory,
            })
            .collect();
        if focused_first
            && let Some(path) = self.selected_path()
            && let Some(n) = items.iter().position(|x| x.path == path)
        {
            let focused = items.remove(n);
            items.insert(0, focused);
        }
        Ok(items)
    }
    fn begin_rename(&self, actions: &mut Vec<Action>) {
        match self.selected_sources(false) {
            Ok(items) => actions.push(Action::Rename(items)),
            Err(e) => actions.push(Action::Message(crate::i18n::Message::capture(|| {
                format!("{e:#}")
            }))),
        }
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        mut context: PaneContext,
        prefs: &mut Preferences,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        if ui.ctx().input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| ui.max_rect().contains(p))
        }) {
            context.focused = true;
            actions.push(Action::Focused);
        }
        if context.split {
            ui.horizontal(|ui| {
                let color = if context.focused {
                    crate::appearance::ACCENT
                } else {
                    crate::appearance::MUTED
                };
                ui.label(
                    RichText::new(match context.pane {
                        Pane::Left => crate::i18n::text("왼쪽 검색", "Left search"),
                        Pane::Right => crate::i18n::text("오른쪽 검색", "Right search"),
                    })
                    .size(11.)
                    .color(color),
                );
                if context.focused {
                    ui.label(
                        RichText::new(crate::i18n::text("활성", "Active"))
                            .size(10.)
                            .color(color),
                    );
                }
            })
            .response
            .on_hover_text(crate::i18n::text(
                "F6으로 검색창 전환",
                "Switch search panes with F6",
            ));
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 64.).max(60.);
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 18.),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&context.index_location).size(11.).weak(),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&context.index_location);
                    },
                );
                if ui
                    .small_button(crate::i18n::text("색인…", "Index…"))
                    .clicked()
                {
                    actions.push(Action::IndexManager);
                }
            });
        }
        let query_id = egui::Id::new(("search-query", context.pane));
        let allow_result_keys = (context.focused && !ui.ctx().egui_wants_keyboard_input()
            || ui.ctx().memory(|m| m.has_focus(query_id)))
            && !context.modal
            && !context.file_drag
            && !egui::Popup::is_any_open(ui.ctx());
        let (open_key, reveal_key, down_key, up_key) = if allow_result_keys {
            ui.ctx().input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    i.consume_key(egui::Modifiers::CTRL, egui::Key::Enter),
                    i.consume_key(i.modifiers, egui::Key::ArrowDown),
                    i.consume_key(i.modifiers, egui::Key::ArrowUp),
                )
            })
        } else {
            (false, false, false, false)
        };
        if context.focused
            && ui.ctx().input_mut(|i| {
                i.consume_key(egui::Modifiers::CTRL, egui::Key::F)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::L)
            })
        {
            ui.ctx().memory_mut(|m| m.request_focus(query_id));
        }
        let mut changed = false;
        let mut settings_changed = false;
        let mut display_changed = false;
        let mut prefs_changed = false;
        let focused = ui.ctx().memory(|m| m.has_focus(query_id));
        let search_frame = egui::Frame::new()
            .fill(Color32::from_rgb(24, 31, 42))
            .corner_radius(8)
            .stroke(egui::Stroke::new(
                1.,
                if focused {
                    crate::appearance::ACCENT
                } else {
                    crate::appearance::LINE
                },
            ))
            .inner_margin(egui::Margin::symmetric(12, 8));
        search_frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                let (icon, _) = ui.allocate_exact_size(egui::vec2(22., 26.), egui::Sense::hover());
                crate::appearance::search_mark(
                    ui.painter(),
                    icon.center(),
                    if context.focused {
                        crate::appearance::ACCENT
                    } else {
                        crate::appearance::MUTED
                    },
                );
                let width = (ui.available_width() - 32. - ui.spacing().item_spacing.x).max(80.);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(query_id)
                        .frame(egui::Frame::NONE)
                        .hint_text(crate::i18n::text(
                            "파일이나 폴더 이름으로 검색",
                            "Search by file or folder name",
                        ))
                        .font(egui::FontId::proportional(17.))
                        .desired_width(width),
                );
                changed |= response.changed();
                if ui
                    .add_enabled(
                        !self.query.is_empty(),
                        egui::Button::new("×")
                            .frame(false)
                            .min_size(egui::vec2(24., 24.)),
                    )
                    .on_hover_text(crate::i18n::text("검색어 지우기", "Clear search"))
                    .clicked()
                {
                    self.query.clear();
                    changed = true;
                    ui.ctx().memory_mut(|m| m.request_focus(query_id));
                }
            });
        });
        let mut recent = None;
        let mut bookmark = None;
        let option_count = [
            self.settings.options.case_sensitive,
            self.settings.options.whole_word,
            self.settings.options.path_match,
            self.settings.options.regex,
        ]
        .into_iter()
        .filter(|v| *v)
        .count();
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("filter")
                .width(90.)
                .selected_text(self.settings.filter.label())
                .show_ui(ui, |ui| {
                    for filter in Filter::ALL {
                        settings_changed |= ui
                            .selectable_value(&mut self.settings.filter, filter, filter.label())
                            .changed();
                    }
                });
            egui::ComboBox::from_id_salt("order")
                .width(100.)
                .selected_text(self.settings.order.label())
                .show_ui(ui, |ui| {
                    for column in crate::index::SortColumn::ALL {
                        settings_changed |= ui
                            .selectable_value(
                                &mut self.settings.order.column,
                                column,
                                column.label(),
                            )
                            .changed();
                    }
                    ui.separator();
                    settings_changed |= ui
                        .checkbox(
                            &mut self.settings.order.descending,
                            crate::i18n::text("내림차순", "Descending"),
                        )
                        .changed();
                    settings_changed |= ui
                        .checkbox(
                            &mut self.settings.order.folders_first,
                            crate::i18n::text("폴더 먼저", "Folders first"),
                        )
                        .changed();
                });
            let option_label = if option_count == 0 {
                crate::i18n::text("검색 옵션", "Search options").into()
            } else {
                crate::trf!(
                    "검색 옵션 · {option_count}",
                    "Search options · {option_count}"
                )
            };
            ui.menu_button(
                RichText::new(option_label).color(if option_count > 0 {
                    crate::appearance::ACCENT
                } else {
                    ui.visuals().text_color()
                }),
                |ui| {
                    ui.set_min_width(200.);
                    ui.label(
                        RichText::new(crate::i18n::text("검색 방식", "Search mode"))
                            .small()
                            .weak(),
                    );
                    settings_changed |= ui
                        .checkbox(
                            &mut self.settings.options.case_sensitive,
                            crate::i18n::text("대소문자 구분", "Case sensitive"),
                        )
                        .changed();
                    settings_changed |= ui
                        .checkbox(
                            &mut self.settings.options.whole_word,
                            crate::i18n::text("단어 전체 일치", "Whole word"),
                        )
                        .changed();
                    settings_changed |= ui
                        .checkbox(
                            &mut self.settings.options.path_match,
                            crate::i18n::text("전체 경로 검색", "Search full path"),
                        )
                        .changed();
                    settings_changed |= ui
                        .checkbox(
                            &mut self.settings.options.regex,
                            crate::i18n::text("정규식", "Regular expression"),
                        )
                        .changed();
                    ui.separator();
                    ui.label(
                        RichText::new(crate::i18n::text("표시", "Display"))
                            .small()
                            .weak(),
                    );
                    display_changed |= ui
                        .checkbox(
                            &mut self.settings.details,
                            crate::i18n::text(
                                "수정일 · 파일 종류 열",
                                "Modified date · File type columns",
                            ),
                        )
                        .on_hover_text(crate::i18n::text(
                            "파일·폴더 크기와 Windows 아이콘은 항상 표시합니다",
                            "File and folder sizes and Windows icons are always shown",
                        ))
                        .changed();
                    ui.separator();
                    if ui
                        .button(crate::i18n::text(
                            "검색 위치 · 색인 관리",
                            "Search location · Manage index",
                        ))
                        .clicked()
                    {
                        actions.push(Action::IndexManager);
                        ui.close();
                    }
                    if ui
                        .button(crate::i18n::text(
                            "검색 문법과 단축키",
                            "Search syntax and shortcuts",
                        ))
                        .clicked()
                    {
                        actions.push(Action::Help);
                        ui.close();
                    }
                },
            );
            ui.menu_button(
                crate::i18n::text("최근 · 즐겨찾기", "Recent · Bookmarks"),
                |ui| {
                    ui.set_min_width(240.);
                    ui.label(
                        RichText::new(crate::i18n::text("최근 검색", "Recent searches"))
                            .small()
                            .weak(),
                    );
                    if prefs.history.is_empty() {
                        ui.label(
                            RichText::new(crate::i18n::text(
                                "아직 저장된 검색이 없습니다",
                                "No saved searches yet",
                            ))
                            .weak(),
                        );
                    }
                    for query in &prefs.history {
                        if ui
                            .add(egui::Button::new(query).frame(false).truncate())
                            .clicked()
                        {
                            recent = Some(query.clone());
                            ui.close();
                        }
                    }
                    if !prefs.history.is_empty()
                        && ui
                            .button(crate::i18n::text(
                                "최근 검색 지우기",
                                "Clear recent searches",
                            ))
                            .clicked()
                    {
                        prefs.history.clear();
                        prefs_changed = true;
                        ui.close();
                    }
                    ui.separator();
                    ui.label(
                        RichText::new(crate::i18n::text("즐겨찾기", "Bookmarks"))
                            .small()
                            .weak(),
                    );
                    for b in &prefs.bookmarks {
                        if ui.button(&b.name).clicked() {
                            bookmark = Some(b.clone());
                            ui.close();
                        }
                    }
                    if ui
                        .button(crate::i18n::text(
                            "현재 검색 저장 · 즐겨찾기 관리",
                            "Save current search · Manage bookmarks",
                        ))
                        .clicked()
                    {
                        actions.push(Action::Bookmarks {
                            query: self.query.clone(),
                            settings: self.settings,
                        });
                        ui.close();
                    }
                },
            );
        });
        if let Some(q) = recent {
            self.query = q;
            changed = true;
        }
        if let Some(b) = bookmark {
            self.query = b.query;
            self.settings.options = b.options;
            self.settings.filter = b.filter;
            self.settings.order = b.order;
            changed = true;
            settings_changed = true;
        }
        if settings_changed || display_changed || prefs_changed {
            actions.push(Action::Settings);
        }
        if (changed || settings_changed) && context.ready {
            self.queue_search(&mut actions);
        }
        ui.horizontal(|ui| {
            ui.set_min_height(22.);
            // Reserve the indicator slot so completion does not shift the result count.
            ui.allocate_ui(egui::vec2(12., 12.), |ui| {
                if self.search_busy {
                    let elapsed = self
                        .search_started
                        .map_or(std::time::Duration::ZERO, |t| t.elapsed());
                    let delay = std::time::Duration::from_millis(150);
                    if elapsed >= delay {
                        ui.add(egui::Spinner::new().size(12.));
                    } else {
                        ui.ctx().request_repaint_after(delay - elapsed);
                    }
                }
            });
            if self.search_error {
                ui.add(
                    egui::Label::new(
                        RichText::new(self.search_status.as_str())
                            .size(12.)
                            .color(Color32::from_rgb(255, 163, 155)),
                    )
                    .truncate(),
                )
                .on_hover_text(self.search_status.as_str());
            } else if let Some(view) = &self.view {
                let label = crate::trf!(
                    "{}개 결과",
                    "{} results",
                    crate::appearance::count(view.total)
                );
                ui.label(RichText::new(label).size(12.).strong())
                    .on_hover_text(self.search_status.as_str());
                if view.total > view.rows.len() {
                    ui.label(
                        RichText::new(crate::trf!(
                            "처음 {}개 표시",
                            "Showing first {} items",
                            crate::appearance::count(view.rows.len())
                        ))
                        .size(11.)
                        .weak(),
                    );
                }
                if !self.selection.items.is_empty() {
                    ui.label(
                        RichText::new(crate::trf!(
                            "· {}개 선택",
                            "· {} selected",
                            crate::appearance::count(self.selection.items.len())
                        ))
                        .size(11.)
                        .color(crate::appearance::ACCENT),
                    );
                }
            } else {
                ui.label(
                    RichText::new(if context.index_busy {
                        crate::i18n::text("파일 목록을 준비하고 있습니다", "Preparing file list")
                    } else {
                        crate::i18n::text("검색을 시작해 보세요", "Start searching")
                    })
                    .size(12.)
                    .weak(),
                );
            }
        });
        ui.spacing_mut().item_spacing.y = 2.;
        let (header_rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.), egui::Sense::hover());
        let view = self.view.as_ref();
        let rows = view.map_or(&[][..], |v| v.rows.as_slice());
        let mut action = None;
        let mut copy_action = None;
        let mut rename_action = false;
        let mut paste_action = None;
        let mut shell_menu = None;
        let mut properties_action = false;
        let mut copy_text = None;
        let details = self.settings.details;
        self.row_hits.clear();
        // Reserve physical space for scrollbars: floating bars otherwise overlap raw selection hit tests.
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        let mut scroll = egui::ScrollArea::both()
            .id_salt("results")
            .auto_shrink([false, false])
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: egui::scroll_area::DragScroll::Never,
                ..Default::default()
            });
        if let Some(offset) = self.scroll_wanted.take() {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        if let Some(n) = self.reveal_row.take()
            && let Some(viewport) = self.list_rect
        {
            let top = n as f32 * (RESULT_ROW_HEIGHT + ui.spacing().item_spacing.y);
            let bottom = top + RESULT_ROW_HEIGHT;
            let current = self.scroll_offset.y;
            let offset = if top < current {
                top
            } else if bottom > current + viewport.height() {
                bottom - viewport.height()
            } else {
                current
            };
            if (offset - current).abs() > 0.1 {
                scroll = scroll.vertical_scroll_offset(offset.max(0.));
            }
        }
        let rows_empty = rows.is_empty();
        let output = scroll.show_rows(ui, RESULT_ROW_HEIGHT, rows.len(), |ui, range| {
            for n in range {
                let row = &rows[n];
                // Keep each menu attached to its path when a refresh reorders the rows.
                let row_id = row.path.as_ref().map_or_else(
                    || ui.make_persistent_id(("unresolved", n, &row.name)),
                    |path| ui.make_persistent_id(path),
                );
                ui.push_id(row_id, |ui| {
                    let (response, name_rect) = result_row(
                        ui,
                        row,
                        self.selection.items.contains(&n),
                        details,
                        row.path.as_ref().and_then(|p| self.decorations.get(p)),
                        {
                            #[cfg(windows)]
                            {
                                row.path
                                    .as_ref()
                                    .and_then(|p| self.icons.get(p))
                                    .map(|(_, texture)| texture)
                            }
                            #[cfg(not(windows))]
                            {
                                None
                            }
                        },
                    );
                    self.row_hits.push(crate::list_drag::RowHit {
                        index: n,
                        rect: response.rect,
                        name: name_rect,
                    });
                    if response.clicked() || response.secondary_clicked() {
                        let modifiers = ui.ctx().input(|i| i.modifiers);
                        self.selection.click(
                            n,
                            modifiers.ctrl,
                            modifiers.shift,
                            response.secondary_clicked(),
                        );
                        self.results_focus = true;
                    }
                    if response.double_clicked()
                        && let Some(path) = &row.path
                    {
                        action = Some(Job::Open {
                            path: path.clone(),
                            directory: row.directory,
                        });
                    }
                    if response.secondary_clicked() && !context.modal {
                        let modifiers = ui.ctx().input(|i| i.modifiers);
                        self.application_menu = !context.native_shell || modifiers.ctrl;
                        if !self.application_menu {
                            shell_menu = response
                                .interact_pointer_pos()
                                .map(|p| (p, modifiers.shift));
                        }
                    }
                    if self.application_menu && !context.modal {
                        response.context_menu(|ui| {
                            let menu = FileMenu::new(ui, row.directory);
                            let path = row.path.as_ref();
                            let valid = !self.selection.items.is_empty()
                                && self
                                    .selection
                                    .items
                                    .iter()
                                    .all(|&n| rows.get(n).is_some_and(|r| r.path.is_some()));
                            ui.label(crate::trf!(
                                "{}개 선택",
                                "{} selected",
                                self.selection.items.len()
                            ));
                            if menu.button(ui, FileMenuItem::Copy, valid).clicked() {
                                copy_action = Some(false);
                                ui.close();
                            }
                            if menu.button(ui, FileMenuItem::Cut, valid).clicked() {
                                copy_action = Some(true);
                                ui.close();
                            }
                            let destination = path.and_then(|p| {
                                if row.directory {
                                    Some(p.clone())
                                } else {
                                    p.parent().map(PathBuf::from)
                                }
                            });
                            if menu
                                .button(ui, FileMenuItem::Paste, destination.is_some())
                                .clicked()
                            {
                                paste_action = destination;
                                ui.close();
                            }
                            if menu.button(ui, FileMenuItem::Rename, valid).clicked() {
                                rename_action = true;
                                ui.close();
                            }
                            ui.separator();
                            if menu
                                .button(ui, FileMenuItem::Open, path.is_some())
                                .clicked()
                                && let Some(path) = path
                            {
                                action = Some(Job::Open {
                                    path: path.clone(),
                                    directory: row.directory,
                                });
                                ui.close();
                            }
                            if menu.button(ui, FileMenuItem::CopyName, true).clicked() {
                                copy_text = Some(
                                    self.selection
                                        .items
                                        .iter()
                                        .filter_map(|&n| rows.get(n))
                                        .map(|r| r.name.as_str())
                                        .collect::<Vec<_>>()
                                        .join("\r\n"),
                                );
                                ui.close();
                            }
                            if menu.button(ui, FileMenuItem::CopyPath, valid).clicked() {
                                copy_text = Some(
                                    self.selection
                                        .items
                                        .iter()
                                        .filter_map(|&n| rows.get(n).and_then(|r| r.path.as_ref()))
                                        .map(|p| p.to_string_lossy())
                                        .collect::<Vec<_>>()
                                        .join("\r\n"),
                                );
                                ui.close();
                            }
                            if menu
                                .button(
                                    ui,
                                    FileMenuItem::OpenLocation,
                                    path.is_some() && self.selection.items.len() == 1,
                                )
                                .clicked()
                                && let Some(path) = path
                            {
                                action = Some(if row.directory {
                                    Job::Open {
                                        path: path.clone(),
                                        directory: true,
                                    }
                                } else {
                                    Job::Reveal(path.clone())
                                });
                                ui.close();
                            }
                            if menu
                                .button(ui, FileMenuItem::Reveal, path.is_some())
                                .clicked()
                                && let Some(path) = path
                            {
                                action = Some(Job::Reveal(path.clone()));
                                ui.close();
                            }
                            if menu
                                .button(
                                    ui,
                                    FileMenuItem::OpenParent,
                                    path.and_then(|p| p.parent()).is_some(),
                                )
                                .clicked()
                                && let Some(parent) = path.and_then(|p| p.parent())
                            {
                                action = Some(Job::Open {
                                    path: parent.into(),
                                    directory: true,
                                });
                                ui.close();
                            }
                            if menu
                                .button(ui, FileMenuItem::Edit, path.is_some() && !row.directory)
                                .clicked()
                                && let Some(path) = path
                            {
                                action = Some(Job::Edit(path.clone()));
                                ui.close();
                            }
                            ui.separator();
                            if menu
                                .button(ui, FileMenuItem::Info, path.is_some())
                                .clicked()
                                && let Some(path) = path
                            {
                                let _ = path;
                                properties_action = true;
                                ui.close();
                            }
                        });
                    }
                });
            }
        });
        if let Some(column) = result_header(
            ui,
            header_rect,
            output.inner_rect.width(),
            output.state.offset.x,
            self.settings.details,
            self.settings.order,
        ) {
            self.settings.order.select(column);
            actions.push(Action::Settings);
            if context.ready {
                self.queue_search(&mut actions);
            }
        }
        if rows_empty {
            let rect = output.inner_rect;
            let center = rect.center();
            let painter = ui.painter().with_clip_rect(rect);
            let busy = self.search_busy || context.index_busy;
            if busy {
                let mut spinner_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                spinner_ui.set_clip_rect(rect);
                egui::Spinner::new()
                    .color(crate::appearance::ACCENT)
                    .paint_at(
                        &spinner_ui,
                        egui::Rect::from_center_size(
                            center - egui::vec2(0., 50.),
                            egui::vec2(24., 24.),
                        ),
                    );
            }
            let title = if busy {
                crate::i18n::text("파일을 찾고 있습니다", "Finding files")
            } else if self.search_error {
                crate::i18n::text("검색 조건을 확인해 주세요", "Check the search query")
            } else if !context.ready {
                crate::i18n::text("검색할 폴더를 선택하세요", "Select a folder to search")
            } else {
                crate::i18n::text("일치하는 파일이 없습니다", "No matching files")
            };
            painter.text(
                center - egui::vec2(0., 12.),
                egui::Align2::CENTER_CENTER,
                title,
                egui::FontId::proportional(16.),
                crate::appearance::MUTED,
            );
            painter.text(
                center + egui::vec2(0., 18.),
                egui::Align2::CENTER_CENTER,
                if !context.ready {
                    crate::i18n::text(
                        "상단의 색인 관리에서 위치를 지정할 수 있습니다",
                        "Choose a location in Manage index above",
                    )
                } else {
                    crate::i18n::text(
                        "검색어를 줄이거나 파일 종류 필터를 확인해 보세요",
                        "Try a shorter query or check the file type filter",
                    )
                },
                egui::FontId::proportional(12.),
                crate::appearance::MUTED,
            );
        }
        self.list_rect = Some(output.inner_rect);
        self.scroll_offset = output.state.offset;
        let mut pointer = ui.ctx().input(|i| crate::list_drag::Pointer {
            pos: i.pointer.interact_pos(),
            pressed: i.pointer.primary_pressed(),
            held: i.pointer.primary_down(),
            released: i.pointer.primary_released(),
            escape: i.key_pressed(egui::Key::Escape),
            modifiers: i.modifiers,
        });
        // Inspect layer ownership outside the input read lock.
        pointer.pressed &= pointer
            .pos
            .is_some_and(|p| ui.ctx().layer_id_at(p) == Some(ui.layer_id()));
        if !context.modal && !context.file_drag {
            let count = self.view.as_ref().map_or(0, |v| v.rows.len());
            let update = self.gesture.update(
                pointer,
                crate::list_drag::Layout {
                    viewport: output.inner_rect,
                    offset: output.state.offset,
                    row_count: count,
                    row_height: RESULT_ROW_HEIGHT,
                    spacing: ui.spacing().item_spacing.y,
                    hits: &self.row_hits,
                },
                &mut self.selection,
            );
            if let Some(n) = update.files {
                if !self.selection.items.contains(&n) {
                    self.selection.click(
                        n,
                        ui.ctx().input(|i| i.modifiers.ctrl),
                        ui.ctx().input(|i| i.modifiers.shift),
                        false,
                    );
                }
                self.selection.focus = Some(n);
                self.results_focus = true;
                ui.ctx().memory_mut(|m| m.surrender_focus(query_id));
                match self.selected_paths() {
                    Ok(paths) => actions.push(Action::Drag(paths)),
                    Err(e) => actions.push(Action::Message(crate::i18n::Message::capture(|| {
                        format!("{e:#}")
                    }))),
                }
            }
            if let Some(rect) = self.gesture.rectangle {
                let painter = ui.painter().with_clip_rect(output.inner_rect);
                painter.rect_filled(rect, 0., Color32::from_rgba_unmultiplied(80, 150, 220, 40));
                painter.rect_stroke(
                    rect,
                    0.,
                    egui::Stroke::new(1., Color32::from_rgb(100, 175, 240)),
                    egui::StrokeKind::Inside,
                );
            }
            if self.gesture.active() {
                if update.auto_scroll != 0. {
                    let max = ((count as f32 * (RESULT_ROW_HEIGHT + ui.spacing().item_spacing.y))
                        - output.inner_rect.height())
                    .max(0.);
                    self.scroll_wanted = Some(
                        (output.state.offset.y
                            + update.auto_scroll
                                * ui.ctx().input(|i| i.stable_dt.min(0.05))
                                * 600.)
                            .clamp(0., max),
                    );
                }
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(16));
            }
        } else {
            self.gesture.cancel();
        }
        if let Some((position, extended)) = shell_menu {
            match self.selected_sources(true) {
                Ok(items) => actions.push(Action::ShellMenu {
                    items,
                    position,
                    extended,
                }),
                Err(e) => actions.push(Action::Message(crate::i18n::Message::capture(|| {
                    format!("{e:#}")
                }))),
            }
        }
        if properties_action {
            match self.selected_sources(true) {
                Ok(items) => actions.push(Action::Properties(items)),
                Err(e) => actions.push(Action::Message(crate::i18n::Message::capture(|| {
                    format!("{e:#}")
                }))),
            }
        }
        if let Some(job) = action {
            prefs.remember(&self.query);
            actions.push(Action::Settings);
            actions.push(Action::Job(job));
        }
        if let Some(text) = copy_text {
            ui.ctx().copy_text(text);
        }
        if let Some(cut) = copy_action {
            self.copy_files(cut, &mut actions);
        }
        if let Some(destination) = paste_action {
            actions.push(Action::Job(Job::ReadPaste(destination)));
        }
        if rename_action {
            self.begin_rename(&mut actions);
        }
        if ui
            .ctx()
            .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F5))
            && !context.modal
            && context.ready
            && !context.index_busy
        {
            actions.push(Action::Refresh);
        }
        if ui
            .ctx()
            .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F1))
            && !context.modal
        {
            actions.push(Action::Help);
        }
        if allow_result_keys {
            let down = down_key;
            let up = up_key;
            let count = self.view.as_ref().map_or(0, |v| v.rows.len());
            if count > 0 && (down || up) {
                let next = match self.selection.focus {
                    None => 0,
                    Some(n) if down => (n + 1).min(count - 1),
                    Some(n) => n.saturating_sub(1),
                };
                let modifiers = ui.ctx().input(|i| i.modifiers);
                self.selection
                    .navigate(next, modifiers.ctrl, modifiers.shift);
                self.reveal_row = Some(next);
                self.results_focus = true;
                ui.ctx().memory_mut(|m| m.surrender_focus(query_id));
            }
            if ui.ctx().input_mut(|i| {
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::C)
            }) && let Ok(paths) = self.selected_paths()
            {
                ui.ctx().copy_text(
                    paths
                        .iter()
                        .map(|p| p.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("\r\n"),
                );
            }
            if ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::Enter))
                && let Ok(items) = self.selected_sources(true)
            {
                actions.push(Action::Properties(items));
            }
            let result_shortcuts = self.results_focus && !ui.ctx().egui_wants_keyboard_input();
            if result_shortcuts {
                if ui
                    .ctx()
                    .input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::A))
                {
                    self.selection.all(count);
                }
                if ui
                    .ctx()
                    .input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::C))
                {
                    self.copy_files(false, &mut actions);
                }
                if ui
                    .ctx()
                    .input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::X))
                {
                    self.copy_files(true, &mut actions);
                }
                if ui
                    .ctx()
                    .input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::V))
                    && let Some(destination) = self.paste_destination()
                {
                    actions.push(Action::Job(Job::ReadPaste(destination)));
                }
                if ui
                    .ctx()
                    .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F2))
                {
                    self.begin_rename(&mut actions);
                }
            }
            let reveal = reveal_key;
            let open = open_key;
            if open || reveal {
                prefs.remember(&self.query);
                actions.push(Action::Settings);
                if !self.search_busy
                    && let Some(row) = self
                        .view
                        .as_ref()
                        .and_then(|v| v.rows.get(self.selection.focus.unwrap_or(0)))
                    && let Some(path) = &row.path
                {
                    actions.push(Action::Job(if reveal {
                        Job::Reveal(path.clone())
                    } else {
                        Job::Open {
                            path: path.clone(),
                            directory: row.directory,
                        }
                    }));
                }
            }
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_cache_preserves_visible_textures_and_bounds_retired_rows() {
        let ctx = egui::Context::default();
        let mut pane = SearchPane::default();
        let start = std::time::Instant::now();
        for n in 0..crate::row_metadata::MAX_CACHE + 3 {
            let path = PathBuf::from(format!("row-{n:04}"));
            #[cfg(windows)]
            {
                let icon = std::sync::Arc::new(crate::shell_icon::Icon {
                    image: egui::ColorImage::filled([1, 1], Color32::WHITE),
                    image_index: 1,
                    overlay_index: 0,
                });
                let texture =
                    ctx.load_texture(format!("cache-{n}"), icon.image.clone(), Default::default());
                pane.icons.insert(path.clone(), (icon.clone(), texture));
            }
            pane.decorations.insert(
                path,
                crate::row_metadata::Decoration {
                    details: crate::details::Details {
                        size: None,
                        modified: None,
                        error: None,
                    },
                    size: None,
                    size_note: None,
                    #[cfg(windows)]
                    icon: None,
                    icon_note: None,
                    measured: start + std::time::Duration::from_millis(n as u64),
                },
            );
        }
        let visible = PathBuf::from("row-0000");
        #[cfg(windows)]
        let original_texture = pane.icons[&visible].1.id();
        pane.trim_metadata(std::slice::from_ref(&visible));
        assert_eq!(pane.decorations.len(), crate::row_metadata::MAX_CACHE);
        assert!(pane.decorations.contains_key(&visible));
        assert!(!pane.decorations.contains_key(&PathBuf::from("row-0001")));
        assert!(!pane.decorations.contains_key(&PathBuf::from("row-0003")));
        pane.trim_metadata(&[]);
        assert!(
            pane.decorations.contains_key(&visible),
            "leaving the viewport discarded a reusable display"
        );
        #[cfg(windows)]
        {
            assert_eq!(pane.icons.len(), crate::row_metadata::MAX_CACHE);
            assert_eq!(pane.icons[&visible].1.id(), original_texture);
        }
    }
}
