#[cfg(feature = "gui")]
pub mod background;
pub mod filters;
pub mod folder;
#[cfg(feature = "gui")]
pub mod gui;
pub mod index;
#[cfg(windows)]
pub mod ntfs;
pub mod query;
pub mod storage;
pub mod usn;

pub mod details;
pub mod preferences;
#[cfg(feature = "gui")]
pub mod services;

#[cfg(windows)]
pub mod file_clipboard;
pub mod fileops;
#[cfg(feature = "gui")]
pub mod selection;

#[cfg(feature = "gui")]
mod rename_dialog;

#[cfg(feature = "gui")]
mod search_pane;

#[cfg(windows)]
pub mod native_drag;

#[cfg(feature = "gui")]
mod list_drag;

#[cfg(windows)]
mod shell_path;

#[cfg(feature = "gui")]
pub mod row_metadata;
#[cfg(all(windows, feature = "gui"))]
pub mod shell_icon;

#[cfg(feature = "gui")]
mod appearance;

#[cfg(all(test, feature = "gui"))]
mod ui_preview;

#[cfg(all(windows, feature = "gui"))]
pub mod shell_menu;

mod ordering;

pub mod hotkey;
#[cfg(all(windows, feature = "gui"))]
mod resident;

mod index_metadata;

#[cfg(feature = "gui")]
mod app_icon;

mod locations;

pub mod byte_size;
pub mod journal;
#[cfg(feature = "gui")]
mod journal_settings;

pub mod cli;

pub mod i18n;

mod trigram;

#[cfg(feature = "gui")]
pub mod graphics;

pub mod app_paths;
#[cfg(feature = "gui")]
mod icon_art;
pub mod product;
