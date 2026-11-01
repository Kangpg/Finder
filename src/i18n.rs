//! App-owned text and retained messages. Never translate arbitrary output text.
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    cell::Cell,
    fmt,
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "ko")]
    Korean,
    #[serde(rename = "en")]
    English,
}
impl Language {
    pub const ALL: [Self; 2] = [Self::Korean, Self::English];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Korean => "한국어",
            Self::English => "English",
        }
    }
}
static CURRENT: AtomicU8 = AtomicU8::new(0);
thread_local! {
    static FORMATTING: Cell<Option<Language>> = const { Cell::new(None) };
}
pub(crate) fn set_language(language: Language) {
    CURRENT.store(
        match language {
            Language::Korean => 0,
            Language::English => 1,
        },
        Ordering::Relaxed,
    );
}
pub fn current() -> Language {
    FORMATTING
        .get()
        .unwrap_or_else(|| match CURRENT.load(Ordering::Relaxed) {
            1 => Language::English,
            _ => Language::Korean,
        })
}
#[inline]
pub fn text(korean: &'static str, english: &'static str) -> &'static str {
    match current() {
        Language::Korean => korean,
        Language::English => english,
    }
}

// Formatting overrides are thread-local and unwind-safe. Capturing a message
// formats its pure display expression twice; it never repeats the operation.
fn in_language<T>(language: Language, render: impl FnOnce() -> T) -> T {
    struct Restore(Option<Language>);
    impl Drop for Restore {
        fn drop(&mut self) {
            FORMATTING.set(self.0);
        }
    }
    let _restore = Restore(FORMATTING.replace(Some(language)));
    render()
}
#[derive(Debug, PartialEq, Eq)]
struct Pair {
    korean: Cow<'static, str>,
    english: Option<Cow<'static, str>>,
}
/// Localized text retains both producer-owned forms, including nested errors.
/// Foreign text uses one unchanged representation. Clones share the allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message(Arc<Pair>);
impl Message {
    pub fn new(korean: &'static str, english: &'static str) -> Self {
        Self(Arc::new(Pair {
            korean: korean.into(),
            english: Some(english.into()),
        }))
    }
    pub fn capture(mut render: impl FnMut() -> String) -> Self {
        let korean = in_language(Language::Korean, &mut render);
        let english = in_language(Language::English, render);
        let english = (english != korean).then_some(Cow::Owned(english));
        Self(Arc::new(Pair {
            korean: Cow::Owned(korean),
            english,
        }))
    }
    pub fn error(error: impl fmt::Display) -> Self {
        Self::capture(|| format!("{error:#}"))
    }
    pub fn as_str(&self) -> &str {
        match current() {
            Language::Korean => &self.0.korean,
            Language::English => self.0.english.as_deref().unwrap_or(&self.0.korean),
        }
    }
    pub fn append(&mut self, suffix: Self) {
        *self = Self::capture(|| format!("{self}{suffix}"));
    }
    pub fn heap_bytes(&self) -> usize {
        let capacity = |s: &Cow<'static, str>| match s {
            Cow::Owned(s) => s.capacity(),
            Cow::Borrowed(_) => 0,
        };
        std::mem::size_of::<Pair>()
            + 2 * std::mem::size_of::<usize>()
            + capacity(&self.0.korean)
            + self.0.english.as_ref().map_or(0, capacity)
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}
impl Default for Message {
    fn default() -> Self {
        Self::from("")
    }
}
impl From<String> for Message {
    fn from(value: String) -> Self {
        Self(Arc::new(Pair {
            korean: Cow::Owned(value),
            english: None,
        }))
    }
}
impl From<&str> for Message {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}
impl Deref for Message {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl std::error::Error for Message {}
impl PartialEq<&str> for Message {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}
impl PartialEq<String> for Message {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

#[macro_export]
macro_rules! msgf {
    ($($args:tt)*) => {
        $crate::i18n::Message::capture(|| $crate::trf!($($args)*))
    };
}

/// Both literals originate at the caller so Rust validates format arguments and
/// implicit captures in both languages. Only the selected branch is evaluated.
#[macro_export]
macro_rules! trf {
    ($ko:literal, $en:literal $(, $($args:tt)*)?) => {
        match $crate::i18n::current() {
            $crate::i18n::Language::Korean => format!($ko $(, $($args)*)?),
            $crate::i18n::Language::English => format!($en $(, $($args)*)?),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_messages_keep_nested_errors_arguments_and_foreign_text() {
        let raw = r"D:\이름\작업 스레드 종료.txt";
        let error = anyhow::anyhow!(Message::new("작업 스레드 종료", "Worker stopped"))
            .context(Message::new("작업 실패", "Operation failed"));
        let mut message = Message::error(error);
        message.append(Message::capture(|| format!(" · {raw}")));
        in_language(Language::Korean, || {
            assert_eq!(
                message.as_str(),
                format!("작업 실패: 작업 스레드 종료 · {raw}")
            )
        });
        in_language(Language::English, || {
            assert_eq!(
                message.as_str(),
                format!("Operation failed: Worker stopped · {raw}")
            )
        });
        let foreign = Message::from(raw);
        for language in Language::ALL {
            in_language(language, || assert_eq!(foreign.as_str(), raw));
        }
    }
    #[test]
    fn nested_formatting_is_thread_local_and_restores_after_unwind() {
        let before = current();
        let result =
            std::panic::catch_unwind(|| in_language(Language::English, || panic!("format test")));
        assert!(result.is_err());
        assert_eq!(current(), before);
        std::thread::scope(|scope| {
            for language in Language::ALL {
                scope.spawn(move || {
                    in_language(language, || {
                        let message = Message::capture(|| text("한국어", "English").to_owned());
                        assert_eq!(current(), language);
                        assert_eq!(message.as_str(), text("한국어", "English"));
                    })
                });
            }
        });
        assert_eq!(current(), before);
    }
}
