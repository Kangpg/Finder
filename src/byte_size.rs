//! Size values stay in bytes; display labels use the app's 1024-based B/KB/MB/GB/TB convention.
use anyhow::{Result, ensure};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit {
    #[default]
    B,
    KB,
    MB,
    GB,
    TB,
}
impl Unit {
    pub const ALL: [Self; 5] = [Self::B, Self::KB, Self::MB, Self::GB, Self::TB];
    pub fn factor(self) -> u64 {
        1024u64.pow(self as u32)
    }
    pub fn label(self) -> &'static str {
        ["B", "KB", "MB", "GB", "TB"][self as usize]
    }
    pub fn best(bytes: u64) -> Self {
        Self::ALL
            .into_iter()
            .rev()
            .find(|u| bytes >= u.factor())
            .unwrap_or(Self::B)
    }
    pub fn exact(self, bytes: u64) -> String {
        let factor = self.factor() as u128;
        let mut text = (bytes as u128 / factor).to_string();
        let mut remainder = bytes as u128 % factor;
        if remainder != 0 {
            text.push('.');
        }
        while remainder != 0 {
            remainder *= 10;
            text.push(char::from(b'0' + (remainder / factor) as u8));
            remainder %= factor;
        }
        text
    }
    pub fn parse(self, text: &str) -> Result<u64> {
        let text = text.trim();
        ensure!(
            !text.is_empty() && text.len() <= 64,
            crate::i18n::Message::new("크기를 숫자로 입력하세요", "Enter a numeric size")
        );
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        ensure!(
            !whole.is_empty()
                && whole.bytes().all(|b| b.is_ascii_digit())
                && fraction.bytes().all(|b| b.is_ascii_digit()),
            crate::i18n::Message::new(
                "크기는 양수 또는 0인 숫자로 입력하세요",
                "Enter a nonnegative size"
            )
        );
        let whole: u64 = whole.parse().map_err(|_| {
            anyhow::anyhow!(crate::i18n::Message::new(
                "크기 범위 초과",
                "Size is out of range"
            ))
        })?;
        let factor = self.factor() as u128;
        let mut tail = 0u128;
        for digit in fraction.bytes().rev() {
            let next = tail + u128::from(digit - b'0') * factor;
            ensure!(
                next.is_multiple_of(10),
                crate::i18n::Message::new(
                    "1 B보다 작은 크기는 입력할 수 없습니다",
                    "Sizes smaller than 1 B are not supported"
                )
            );
            tail = next / 10;
        }
        let bytes = (whole as u128) * factor + tail;
        u64::try_from(bytes).map_err(|_| {
            anyhow::anyhow!(crate::i18n::Message::new(
                "크기 범위 초과",
                "Size is out of range"
            ))
        })
    }
}
pub fn format(bytes: u64) -> String {
    let unit = Unit::best(bytes);
    if unit == Unit::B {
        format!("{bytes} B")
    } else {
        format!(
            "{:.1} {}",
            bytes as f64 / unit.factor() as f64,
            unit.label()
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sizes_and_editable_units_keep_exact_bytes_through_tb() {
        for (bytes, text) in [
            (0, "0 B"),
            (1023, "1023 B"),
            (1024, "1.0 KB"),
            (1 << 20, "1.0 MB"),
            (1 << 30, "1.0 GB"),
            (1 << 40, "1.0 TB"),
        ] {
            assert_eq!(format(bytes), text);
        }
        for bytes in [0, 1, 4096, 128 * 1024 * 1024, 1 << 40, u64::MAX] {
            for unit in Unit::ALL {
                assert_eq!(unit.parse(&unit.exact(bytes)).unwrap(), bytes);
            }
        }
        assert_eq!(Unit::GB.parse("0.125").unwrap(), 128 * 1024 * 1024);
        for invalid in [
            "-1",
            "NaN",
            "inf",
            "1e4",
            "1.2.3",
            "",
            "18446744073709551616",
        ] {
            assert!(Unit::B.parse(invalid).is_err());
        }
        assert!(Unit::B.parse("0.5").is_err());
        assert!(Unit::TB.parse("18446744073709551615").is_err());
    }
}
