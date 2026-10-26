//! Pure text assembly for the P0.3 overlay: joining selected OCR words into
//! one paragraph. A space goes between two words unless both touching
//! characters are CJK — Chinese/Japanese/Korean text is written without
//! inter-word spaces, mixed Latin+CJK text still separates at the boundary.

/// True for Han, kana, Hangul and CJK/full-width punctuation.
pub fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3000..=0x303F // CJK symbols and punctuation
        | 0x3040..=0x30FF // hiragana + katakana
        | 0x3400..=0x4DBF // CJK extension A
        | 0x4E00..=0x9FFF // CJK unified ideographs
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK compatibility ideographs
        | 0xFF00..=0xFF60 // full-width forms
        | 0xFFE0..=0xFFE6
    )
}

/// Join words in reading order into one paragraph. Empty words are skipped.
pub fn join_words<'a, I: IntoIterator<Item = &'a str>>(words: I) -> String {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for word in words {
        let Some(first) = word.chars().next() else {
            continue;
        };
        if let Some(p) = prev {
            if !(is_cjk(p) && is_cjk(first)) {
                out.push(' ');
            }
        }
        out.push_str(word);
        prev = word.chars().last();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_words_get_spaces() {
        assert_eq!(join_words(["hello", "world"]), "hello world");
        assert_eq!(join_words(["Hello,", "world!"]), "Hello, world!");
        assert_eq!(join_words(["one"]), "one");
    }

    #[test]
    fn cjk_runs_have_no_spaces() {
        assert_eq!(join_words(["你好", "世界"]), "你好世界");
        assert_eq!(join_words(["こんにちは", "世界"]), "こんにちは世界");
        assert_eq!(join_words(["안녕", "하세요"]), "안녕하세요");
        // Full-width punctuation counts as CJK.
        assert_eq!(join_words(["你好，", "世界"]), "你好，世界");
    }

    #[test]
    fn mixed_boundaries_keep_one_space() {
        assert_eq!(join_words(["hello", "世界"]), "hello 世界");
        assert_eq!(join_words(["世界", "hello"]), "世界 hello");
        assert_eq!(join_words(["2026", "年"]), "2026 年");
    }

    #[test]
    fn cjk_detection_ranges() {
        assert!(is_cjk('你'));
        assert!(is_cjk('あ'));
        assert!(is_cjk('한'));
        assert!(is_cjk('，'));
        assert!(!is_cjk('a'));
        assert!(!is_cjk('1'));
        assert!(!is_cjk(' '));
        assert!(!is_cjk(','));
    }

    #[test]
    fn empty_words_are_skipped() {
        assert_eq!(join_words(["", "a", "", "b"]), "a b");
        assert_eq!(join_words(std::iter::empty()), "");
    }
}
