use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: u32,
}
impl Default for Hotkey {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            key: 32,
        }
    }
}
impl Hotkey {
    pub fn validate(self) -> Result<()> {
        ensure!(
            self.ctrl || self.alt || self.win,
            crate::i18n::Message::new(
                "Ctrl, Alt, Windows 중 하나 이상을 선택하세요",
                "Select at least one of Ctrl, Alt, or Windows"
            )
        );
        ensure!(
            Self::keys().any(|(key, _)| key == self.key),
            crate::i18n::Message::new(
                "지원하지 않는 단축키입니다 (F12는 Windows 예약 키)",
                "Unsupported shortcut (F12 is reserved by Windows)"
            )
        );
        Ok(())
    }
    pub fn keys() -> impl Iterator<Item = (u32, String)> {
        [
            (32, "Space"),
            (36, "Home"),
            (35, "End"),
            (45, "Insert"),
            (33, "Page Up"),
            (34, "Page Down"),
        ]
        .into_iter()
        .map(|(key, s)| (key, s.into()))
        .chain(
            (48..=57)
                .chain(65..=90)
                .map(|key| (key, char::from_u32(key).unwrap().to_string())),
        )
        .chain(
            (1..=24)
                .filter(|n| *n != 12)
                .map(|n| (111 + n, format!("F{n}"))),
        )
    }
    pub fn label(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".into());
        }
        if self.alt {
            parts.push("Alt".into());
        }
        if self.shift {
            parts.push("Shift".into());
        }
        if self.win {
            parts.push("Win".into());
        }
        parts.push(
            Self::keys()
                .find(|(k, _)| *k == self.key)
                .map_or_else(|| "?".into(), |(_, n)| n),
        );
        parts.join(" + ")
    }
    pub(crate) fn modifiers(self) -> u32 {
        u32::from(self.alt)
            | u32::from(self.ctrl) << 1
            | u32::from(self.shift) << 2
            | u32::from(self.win) << 3
    }
}
