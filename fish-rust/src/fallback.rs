//! This file only contains fallback implementations of functions which have been found to be missing
//! or broken by the configuration scripts.
//!
//! Many of these functions are more or less broken and incomplete.

use crate::widecharwidth::{WcLookupTable, WcWidth};
use crate::{common::is_console_session, wchar::wstr};
use once_cell::sync::Lazy;
use std::cmp;
use std::sync::atomic::{AtomicI32, Ordering};
use std::{ffi::CString, mem, os::fd::RawFd};

// Width of ambiguous characters. 1 is typical default.
static FISH_AMBIGUOUS_WIDTH: AtomicI32 = AtomicI32::new(1);

// Width of emoji characters.
// 1 is the typical emoji width in Unicode 8.
static FISH_EMOJI_WIDTH: AtomicI32 = AtomicI32::new(1);

fn fish_get_emoji_width() -> i32 {
    FISH_EMOJI_WIDTH.load(Ordering::Relaxed)
}

extern "C" {
    pub fn wcwidth(c: libc::wchar_t) -> libc::c_int;
}
fn system_wcwidth(c: char) -> i32 {
    const _: () = assert!(mem::size_of::<libc::wchar_t>() >= mem::size_of::<char>());
    unsafe { wcwidth(c as libc::wchar_t) }
}

static WC_LOOKUP_TABLE: Lazy<WcLookupTable> = Lazy::new(WcLookupTable::new);

// Big hack to use our versions of wcswidth where we know them to be broken, which is
// EVERYWHERE (https://github.com/fish-shell/fish-shell/issues/2199)
pub fn fish_wcwidth(c: char) -> i32 {
    // The system version of wcwidth should accurately reflect the ability to represent characters
    // in the console session, but knows nothing about the capabilities of other terminal emulators
    // or ttys. Use it from the start only if we are logged in to the physical console.
    if is_console_session() {
        return system_wcwidth(c);
    }

    // Check for VS16 which selects emoji presentation. This "promotes" a character like U+2764
    // (width 1) to an emoji (probably width 2). So treat it as width 1 so the sums work. See #2652.
    // VS15 selects text presentation.
    let variation_selector_16 = '\u{FE0F}';
    let variation_selector_15 = '\u{FE0E}';
    if c == variation_selector_16 {
        return 1;
    } else if c == variation_selector_15 {
        return 0;
    }

    // Check for Emoji_Modifier property. Only the Fitzpatrick modifiers have this, in range
    // 1F3FB..1F3FF. This is a hack because such an emoji appearing on its own would be drawn as
    // width 2, but that's unlikely to be useful. See #8275.
    if ('\u{F3FB}'..='\u{1F3FF}').contains(&c) {
        return 0;
    }

    let width = WC_LOOKUP_TABLE.classify(c);
    match width {
        WcWidth::NonCharacter | WcWidth::NonPrint | WcWidth::Combining | WcWidth::Unassigned => {
            // Fall back to system wcwidth in this case.
            system_wcwidth(c)
        }
        WcWidth::Ambiguous | WcWidth::PrivateUse => {
            // TR11: "All private-use characters are by default classified as Ambiguous".
            FISH_AMBIGUOUS_WIDTH.load(Ordering::Relaxed)
        }
        WcWidth::One => 1,
        WcWidth::Two => 2,
        WcWidth::WidenedIn9 => fish_get_emoji_width(),
    }
}

/// fish's internal versions of wcwidth and wcswidth, which can use an internal implementation if
/// the system one is busted.
pub fn fish_wcswidth(s: &wstr) -> i32 {
    let mut result = 0;
    for c in s.chars() {
        let w = fish_wcwidth(c);
        if w < 0 {
            return -1;
        }
        result += w;
    }
    result
}

// Replacement for mkostemp(str, O_CLOEXEC)
// This uses mkostemp if available,
// otherwise it uses mkstemp followed by fcntl
pub fn fish_mkstemp_cloexec(name_template: CString) -> (RawFd, CString) {
    let name = name_template.into_raw();
    #[cfg(not(target_os = "macos"))]
    let fd = {
        use libc::O_CLOEXEC;
        unsafe { libc::mkostemp(name, O_CLOEXEC) }
    };
    #[cfg(target_os = "macos")]
    let fd = {
        use libc::{FD_CLOEXEC, F_SETFD};
        let fd = unsafe { libc::mkstemp(name) };
        if fd != -1 {
            unsafe { libc::fcntl(fd, F_SETFD, FD_CLOEXEC) };
        }
        fd
    };
    (fd, unsafe { CString::from_raw(name) })
}

pub fn fish_tparm() {
    todo!()
}

pub fn wcscasecmp(lhs: &wstr, rhs: &wstr) -> cmp::Ordering {
    use std::char::ToLowercase;
    use widestring::utfstr::CharsUtf32;

    /// This struct streams the underlying lowercase chars of a `UTF32String` without allocating.
    ///
    /// `char::to_lowercase()` returns an iterator of chars and we sometimes need to cmp the last
    /// char of one char's `to_lowercase()` with the first char of the other char's
    /// `to_lowercase()`. This makes that possible.
    struct ToLowerBuffer<'a> {
        current: ToLowercase,
        chars: CharsUtf32<'a>,
    }

    impl<'a> Iterator for ToLowerBuffer<'a> {
        type Item = char;

        fn next(&mut self) -> Option<Self::Item> {
            if let Some(c) = self.current.next() {
                return Some(c);
            }

            self.current = self.chars.next()?.to_lowercase();
            self.next()
        }
    }

    impl<'a> ToLowerBuffer<'a> {
        pub fn from(w: &'a wstr) -> Self {
            let mut empty = 'a'.to_lowercase();
            let _ = empty.next();
            debug_assert!(empty.next().is_none());
            let mut chars = w.chars();
            Self {
                current: chars.next().map(|c| c.to_lowercase()).unwrap_or(empty),
                chars,
            }
        }
    }

    let lhs = ToLowerBuffer::from(lhs);
    let rhs = ToLowerBuffer::from(rhs);
    lhs.cmp(rhs)
}

#[test]
fn test_wcscasecmp() {
    use crate::wchar::L;
    use std::cmp::Ordering;

    // Comparison with empty
    assert_eq!(wcscasecmp(L!("a"), L!("")), Ordering::Greater);
    assert_eq!(wcscasecmp(L!(""), L!("a")), Ordering::Less);
    assert_eq!(wcscasecmp(L!(""), L!("")), Ordering::Equal);

    // Basic comparison
    assert_eq!(wcscasecmp(L!("A"), L!("a")), Ordering::Equal);
    assert_eq!(wcscasecmp(L!("B"), L!("a")), Ordering::Greater);
    assert_eq!(wcscasecmp(L!("A"), L!("B")), Ordering::Less);

    // Multi-byte comparison
    assert_eq!(wcscasecmp(L!("İ"), L!("i\u{307}")), Ordering::Equal);
    assert_eq!(wcscasecmp(L!("ia"), L!("İa")), Ordering::Less);
}
