//! Al Majlis patch: Arabic joining for clusters drawn by a fallback [`GlyphRasterizer`].
//!
//! When no installed font has Arabic (the web build: no system fonts), each grapheme cluster
//! is drawn on its own by the fallback rasterizer (eframe's Canvas 2D), so the browser only
//! ever sees one letter and draws its isolated form. Here the letter is swapped for its
//! contextual presentation form (Arabic Presentation Forms-A/B), chosen from its logical
//! neighbours, so the letters join. Lam followed by alef becomes the lam-alef ligature, drawn
//! by the lam; the alef is then drawn as nothing (zero width). Only what is drawn changes:
//! the text, the glyph count and the cursor positions stay as they are. Bidi reordering is
//! done by the layout as before.
//!
//! [`GlyphRasterizer`]: crate::text::GlyphRasterizer

use super::unicode::is_combining_mark;

/// What to draw for a cluster's first char.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ArabicForm {
    /// Not an Arabic letter with forms: draw the cluster as it is.
    Unchanged,

    /// Draw this presentation form instead of the char.
    Form(char),

    /// The alef of a lam-alef ligature: the lam draws both, draw nothing.
    Hidden,
}

/// The presentation forms of a letter: isolated, final, and for dual-joining letters initial
/// and medial.
#[derive(Clone, Copy)]
struct Forms {
    isolated: char,
    final_: char,
    initial: Option<char>,
    medial: Option<char>,
}

const fn d(isolated: u32) -> Forms {
    // Forms-B dual-joining letters are four consecutive code points: isolated, final, initial, medial.
    Forms {
        isolated: char_of(isolated),
        final_: char_of(isolated + 1),
        initial: Some(char_of(isolated + 2)),
        medial: Some(char_of(isolated + 3)),
    }
}

const fn r(isolated: u32) -> Forms {
    Forms {
        isolated: char_of(isolated),
        final_: char_of(isolated + 1),
        initial: None,
        medial: None,
    }
}

const fn char_of(u: u32) -> char {
    match char::from_u32(u) {
        Some(c) => c,
        None => '\u{FFFD}',
    }
}

fn forms(c: char) -> Option<Forms> {
    Some(match c {
        '\u{0622}' => r(0xFE81),
        '\u{0623}' => r(0xFE83),
        '\u{0624}' => r(0xFE85),
        '\u{0625}' => r(0xFE87),
        '\u{0626}' => d(0xFE89),
        '\u{0627}' => r(0xFE8D),
        '\u{0628}' => d(0xFE8F),
        '\u{0629}' => r(0xFE93),
        '\u{062A}' => d(0xFE95),
        '\u{062B}' => d(0xFE99),
        '\u{062C}' => d(0xFE9D),
        '\u{062D}' => d(0xFEA1),
        '\u{062E}' => d(0xFEA5),
        '\u{062F}' => r(0xFEA9),
        '\u{0630}' => r(0xFEAB),
        '\u{0631}' => r(0xFEAD),
        '\u{0632}' => r(0xFEAF),
        '\u{0633}' => d(0xFEB1),
        '\u{0634}' => d(0xFEB5),
        '\u{0635}' => d(0xFEB9),
        '\u{0636}' => d(0xFEBD),
        '\u{0637}' => d(0xFEC1),
        '\u{0638}' => d(0xFEC5),
        '\u{0639}' => d(0xFEC9),
        '\u{063A}' => d(0xFECD),
        '\u{0641}' => d(0xFED1),
        '\u{0642}' => d(0xFED5),
        '\u{0643}' => d(0xFED9),
        '\u{0644}' => d(0xFEDD),
        '\u{0645}' => d(0xFEE1),
        '\u{0646}' => d(0xFEE5),
        '\u{0647}' => d(0xFEE9),
        '\u{0648}' => r(0xFEED),
        // Alef maksura is dual-joining; its initial and medial forms are in Forms-A.
        '\u{0649}' => Forms {
            isolated: '\u{FEEF}',
            final_: '\u{FEF0}',
            initial: Some('\u{FBE8}'),
            medial: Some('\u{FBE9}'),
        },
        '\u{064A}' => d(0xFEF1),
        // Forms-A: alef wasla and the common Persian/Urdu letters.
        '\u{0671}' => r(0xFB50),
        '\u{067E}' => d(0xFB56),
        '\u{0686}' => d(0xFB7A),
        '\u{0698}' => r(0xFB8A),
        '\u{06A9}' => d(0xFB8E),
        '\u{06AF}' => d(0xFB92),
        '\u{06CC}' => d(0xFBFC),
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Joining {
    /// Joins on both sides.
    Dual,
    /// Joins only to the letter before it (on its right).
    Right,
    /// Tatweel and ZWJ: make their neighbours join.
    Causing,
    NonJoining,
}

fn joining(c: char) -> Joining {
    if c == '\u{0640}' || c == '\u{200D}' {
        return Joining::Causing;
    }
    match forms(c) {
        Some(f) if f.initial.is_some() => Joining::Dual,
        Some(_) => Joining::Right,
        None => Joining::NonJoining,
    }
}

/// Marks (harakat, superscript alef…) sit on a letter and do not break the joining.
fn is_transparent(c: char) -> bool {
    is_combining_mark(c)
}

fn is_alef(c: char) -> bool {
    matches!(c, '\u{0622}' | '\u{0623}' | '\u{0625}' | '\u{0627}')
}

/// The lam-alef ligature (isolated form) for lam followed by `alef`.
fn lam_alef(alef: char) -> char {
    match alef {
        '\u{0622}' => '\u{FEF5}',
        '\u{0623}' => '\u{FEF7}',
        '\u{0625}' => '\u{FEF9}',
        _ => '\u{FEFB}',
    }
}

/// Can the letter before a char connect to it?
fn joins_forward(prev: Option<char>) -> bool {
    prev.is_some_and(|p| matches!(joining(p), Joining::Dual | Joining::Causing))
}

/// Can the letter after a char connect to it?
fn joins_backward(next: Option<char>) -> bool {
    next.is_some_and(|n| matches!(joining(n), Joining::Dual | Joining::Right | Joining::Causing))
}

/// The form to draw for the char at byte `offset` of `text` (one run of text, in logical order).
pub(super) fn contextual_form(text: &str, offset: usize) -> ArabicForm {
    let Some(c) = text.get(offset..).and_then(|s| s.chars().next()) else {
        return ArabicForm::Unchanged;
    };
    let Some(f) = forms(c) else {
        return ArabicForm::Unchanged;
    };
    let mut before = text[..offset].chars().rev().filter(|&c| !is_transparent(c));
    let prev = before.next();
    let next = text[offset + c.len_utf8()..]
        .chars()
        .find(|&c| !is_transparent(c));

    if is_alef(c) && prev == Some('\u{0644}') {
        return ArabicForm::Hidden;
    }

    // The lam before this lam decides whether the ligature joins on its right.
    let joined_before = joins_forward(prev);
    if c == '\u{0644}'
        && let Some(alef) = next.filter(|&n| is_alef(n))
    {
        let ligature = lam_alef(alef);
        return ArabicForm::Form(if joined_before {
            char_of(ligature as u32 + 1)
        } else {
            ligature
        });
    }

    let joined_after = f.initial.is_some() && joins_backward(next);
    ArabicForm::Form(match (joined_before, joined_after) {
        (true, true) => f.medial.unwrap_or(f.final_),
        (true, false) => f.final_,
        (false, true) => f.initial.unwrap_or(f.isolated),
        (false, false) => f.isolated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms_of(text: &str) -> String {
        text.char_indices()
            .map(|(i, c)| match contextual_form(text, i) {
                ArabicForm::Unchanged => c.to_string(),
                ArabicForm::Form(f) => f.to_string(),
                ArabicForm::Hidden => String::new(),
            })
            .collect()
    }

    #[test]
    fn letters_take_their_joining_forms() {
        // بيت: initial, medial, final.
        assert_eq!(forms_of("بيت"), "\u{FE91}\u{FEF4}\u{FE96}");
        // بار: alef and reh join only to the letter before them; after dal nothing joins.
        assert_eq!(forms_of("بار"), "\u{FE91}\u{FE8E}\u{FEAD}");
        assert_eq!(forms_of("دار"), "\u{FEA9}\u{FE8D}\u{FEAD}");
        assert_eq!(forms_of("ب"), "\u{FE8F}");
    }

    #[test]
    fn lam_alef_is_one_ligature() {
        // لا: the ligature, the alef draws nothing.
        assert_eq!(forms_of("لا"), "\u{FEFB}");
        // سلام: seen initial, lam-alef final ligature, meem isolated.
        assert_eq!(forms_of("سلام"), "\u{FEB3}\u{FEFC}\u{FEE1}");
        assert_eq!(forms_of("الإمارات").chars().count(), "الإمارات".chars().count() - 1);
    }

    #[test]
    fn marks_and_spaces() {
        // A mark between two letters does not break the join; a space does.
        assert_eq!(forms_of("بَب"), "\u{FE91}\u{064E}\u{FE90}");
        assert_eq!(forms_of("ب ب"), "\u{FE8F} \u{FE8F}");
        assert_eq!(forms_of("abc 123"), "abc 123");
    }
}
