//! Live-tweakable constants, after Processing's Tweak Mode.
//!
//! Every module-scope `const` of the editor text whose value is a plain
//! literal — a number, or a `vecN` of numbers — becomes a control in the
//! Constants window. For the compile, [`instrument`] turns those `const`s
//! into `var<private>`s that the entry points load from the `i_tweaks`
//! uniform array (see [`crate::compile`]), so dragging a slider only uploads
//! uniforms: no recompile, no pipeline rebuild, and buffer shaders keep their
//! state. When the drag ends, [`write_back`] puts the new literal into the
//! source, so saving and sharing keep it.
//!
//! A trailing comment fine-tunes a constant:
//!
//! ```wgsl
//! const SEA_HEIGHT: f32 = 0.6; // tweak: 0..2
//! const WATER: vec3<f32> = vec3<f32>(0.5, 0.5, 0.4); // tweak: color
//! const PI: f32 = 3.14159; // tweak: off
//! ```
//!
//! The rewrite keeps every line where it was (only the `const` keyword
//! changes, and the loader is appended), so compile errors and analyzer
//! diagnostics still point at the lines the user sees.

use std::ops::Range;

/// How many constants can be tweaked live (the length of `i_tweaks`).
pub const MAX_TWEAKS: usize = 32;

/// The generated function that loads the tweaked values; appended to the
/// user module and called first by every entry point.
pub const LOADER: &str = "st_load_tweaks";

/// What a constant holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    F32,
    I32,
    U32,
    /// `vecN<f32>` with N in 2..=4.
    Vec(u8),
}

impl Kind {
    pub fn components(self) -> usize {
        match self {
            Kind::Vec(n) => n as usize,
            _ => 1,
        }
    }
}

/// The trailing `// tweak: …` annotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hint {
    /// No annotation: the range follows from the value.
    Auto,
    Range(f32, f32),
    /// A color picker (for `vec3` / `vec4`).
    Color,
    /// Not shown and not tweakable.
    Off,
}

/// One tweakable constant found in the source.
#[derive(Clone, Debug, PartialEq)]
pub struct Constant {
    pub name: String,
    pub kind: Kind,
    /// The literal value; components past [`Kind::components`] are 0.
    pub value: [f32; 4],
    pub hint: Hint,
    /// Byte range of the `const` keyword.
    keyword: Range<usize>,
    /// Byte range of the initializer expression.
    init: Range<usize>,
    /// How the initializer is spelled, for writing a new value back.
    spelling: Spelling,
}

#[derive(Clone, Debug, PartialEq)]
enum Spelling {
    /// A number with this suffix (`""`, `"f"`, `"i"`, `"u"`).
    Scalar { suffix: String },
    /// A constructor call such as `vec3<f32>(…)`, by its callee text.
    Vector { ctor: String },
}

impl Constant {
    /// Whether the window should offer a color picker.
    pub fn is_color(&self) -> bool {
        match self.hint {
            Hint::Color => matches!(self.kind, Kind::Vec(3 | 4)),
            Hint::Auto => {
                let name = self.name.to_ascii_lowercase();
                matches!(self.kind, Kind::Vec(3 | 4))
                    && ["color", "colour", "col", "tint"]
                        .iter()
                        .any(|w| name.contains(w))
            }
            _ => false,
        }
    }

    /// The slider range for a scalar: the annotation's, else one that
    /// follows from the source value (`default`, so it holds still while
    /// the value is dragged).
    pub fn range(&self, default: f32) -> (f32, f32) {
        if let Hint::Range(lo, hi) = self.hint {
            return (lo, hi);
        }
        let span = match self.kind {
            Kind::I32 | Kind::U32 => (2.0 * default.abs()).max(1.0),
            _ => 2.0 * default.abs(),
        };
        match default {
            0.0 => (if self.kind == Kind::U32 { 0.0 } else { -1.0 }, 1.0),
            v if v > 0.0 => (0.0, span),
            _ => (-span, 0.0),
        }
    }

    /// The initializer for `value`, spelled like the original.
    fn spell(&self, value: [f32; 4]) -> String {
        match &self.spelling {
            Spelling::Scalar { suffix } => format!("{}{suffix}", self.number(value[0])),
            Spelling::Vector { ctor } => {
                let parts: Vec<String> = value[..self.kind.components()]
                    .iter()
                    .map(|v| self.number(*v))
                    .collect();
                format!("{ctor}({})", parts.join(", "))
            }
        }
    }

    fn number(&self, v: f32) -> String {
        match self.kind {
            Kind::I32 => format!("{}", v.round() as i64),
            Kind::U32 => format!("{}", v.round().max(0.0) as u64),
            _ => float_literal(v),
        }
    }

    /// The WGSL expression that reads this constant's slot.
    fn load(&self, slot: usize) -> String {
        let v = format!("package::shadertoy::i_tweak({slot}u)");
        match self.kind {
            Kind::F32 => format!("{v}.x"),
            Kind::I32 => format!("i32(round({v}.x))"),
            Kind::U32 => format!("u32(max(round({v}.x), 0.0))"),
            Kind::Vec(2) => format!("{v}.xy"),
            Kind::Vec(3) => format!("{v}.xyz"),
            Kind::Vec(_) => v,
        }
    }
}

/// A float as a WGSL literal: four significant digits, at least one digit
/// after the point, so it stays a float.
fn float_literal(v: f32) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0.0".into();
    }
    let decimals = (3 - v.abs().log10().floor() as i32).clamp(1, 8) as usize;
    let s = format!("{v:.decimals$}");
    let s = s.trim_end_matches('0');
    if s.ends_with('.') {
        format!("{s}0")
    } else {
        s.to_string()
    }
}

// --- scanning ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    Ident,
    Number,
    Punct(u8),
}

struct Token {
    tok: Tok,
    span: Range<usize>,
}

/// Tokens of `src`, plus the line comments (span of the text after `//`).
fn lex(src: &str) -> (Vec<Token>, Vec<Range<usize>>) {
    let b = src.as_bytes();
    let (mut tokens, mut comments) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c.is_ascii_whitespace() {
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            comments.push(start + 2..i);
        } else if b[i..].starts_with(b"/*") {
            // WGSL block comments nest.
            let mut depth = 0;
            while i < b.len() {
                if b[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let hex = b[i..].starts_with(b"0x") || b[i..].starts_with(b"0X");
            i += 1;
            while i < b.len() {
                let d = b[i];
                let exponent_sign =
                    !hex && (d == b'+' || d == b'-') && matches!(b[i - 1], b'e' | b'E');
                if d.is_ascii_alphanumeric() || d == b'.' || d == b'_' || exponent_sign {
                    i += 1;
                } else {
                    break;
                }
            }
            tokens.push(Token {
                tok: Tok::Number,
                span: start..i,
            });
        } else if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 0x80) {
                i += 1;
            }
            tokens.push(Token {
                tok: Tok::Ident,
                span: start..i,
            });
        } else {
            i += 1;
            tokens.push(Token {
                tok: Tok::Punct(c),
                span: start..i,
            });
        }
    }
    (tokens, comments)
}

/// A number literal's value and suffix, if it is a plain decimal number.
fn parse_number(text: &str) -> Option<(f64, &str, bool)> {
    if text.starts_with("0x") || text.starts_with("0X") {
        return None;
    }
    let (digits, suffix) = match text.as_bytes().last()? {
        b'f' | b'h' | b'i' | b'u' => text.split_at(text.len() - 1),
        _ => (text, ""),
    };
    let float = digits.contains(['.', 'e', 'E']) || matches!(suffix, "f" | "h");
    Some((digits.parse().ok()?, suffix, float))
}

/// Every candidate constant in `src`, including the ones annotated `off`,
/// in declaration order.
pub fn extract(src: &str) -> Vec<Constant> {
    let (tokens, comments) = lex(src);
    let text = |t: &Token| &src[t.span.clone()];
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i].tok {
            Tok::Punct(b'{') => depth += 1,
            Tok::Punct(b'}') => depth -= 1,
            Tok::Ident if depth == 0 && text(&tokens[i]) == "const" => {
                // Not after an attribute (`@if(...) const`), which WESL may
                // compile away.
                let attributed = i > 0 && matches!(tokens[i - 1].tok, Tok::Punct(b')'));
                if let Some((constant, end)) = parse_const(src, &tokens, &comments, i)
                    && !attributed
                {
                    out.push(constant);
                    i = end;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Parse `const NAME [: TYPE] = INIT;` starting at token `at`; returns the
/// constant and the index after its `;`.
fn parse_const(
    src: &str,
    tokens: &[Token],
    comments: &[Range<usize>],
    at: usize,
) -> Option<(Constant, usize)> {
    let text = |i: usize| tokens.get(i).map(|t| &src[t.span.clone()]);
    let name_tok = tokens.get(at + 1).filter(|t| t.tok == Tok::Ident)?;
    let mut i = at + 2;
    let mut declared = String::new();
    if text(i) == Some(":") {
        i += 1;
        while text(i).is_some_and(|t| t != "=") {
            declared.push_str(text(i)?);
            i += 1;
        }
    }
    if text(i) != Some("=") {
        return None;
    }
    i += 1;
    let init_start = i;
    let semi = (i..tokens.len()).find(|&j| text(j) == Some(";"))?;
    let init = &tokens[init_start..semi];
    let (kind, value, spelling) = parse_init(src, init, &declared)?;

    // A `// tweak: …` comment on the same line, after the `;`.
    let line_end = src[tokens[semi].span.end..]
        .find('\n')
        .map_or(src.len(), |n| tokens[semi].span.end + n);
    let hint = comments
        .iter()
        .find(|c| c.start >= tokens[semi].span.end && c.end <= line_end)
        .and_then(|c| parse_hint(&src[c.clone()]))
        .unwrap_or(Hint::Auto);

    let constant = Constant {
        name: src[name_tok.span.clone()].to_string(),
        kind,
        value,
        hint,
        keyword: tokens[at].span.clone(),
        init: init.first()?.span.start..init.last()?.span.end,
        spelling,
    };
    Some((constant, semi + 1))
}

/// A signed number at the front of `toks`: (value, suffix, is_float, used).
fn signed_number<'a>(src: &'a str, toks: &[Token]) -> Option<(f64, &'a str, bool, usize)> {
    let negative = toks.first()?.tok == Tok::Punct(b'-');
    let n = toks.get(usize::from(negative))?;
    if n.tok != Tok::Number {
        return None;
    }
    let (v, suffix, float) = parse_number(&src[n.span.clone()])?;
    Some((
        if negative { -v } else { v },
        suffix,
        float,
        usize::from(negative) + 1,
    ))
}

fn parse_init(src: &str, init: &[Token], declared: &str) -> Option<(Kind, [f32; 4], Spelling)> {
    // A scalar literal.
    if let Some((v, suffix, float, used)) = signed_number(src, init)
        && used == init.len()
    {
        let kind = match (declared, suffix) {
            ("f32", _) => Kind::F32,
            ("i32", _) => Kind::I32,
            ("u32", _) => Kind::U32,
            ("", "i") => Kind::I32,
            ("", "u") => Kind::U32,
            // An untyped integer could become i32, u32 or f32 at each use.
            ("", _) if float => Kind::F32,
            _ => return None,
        };
        let spelling = Spelling::Scalar {
            suffix: suffix.to_string(),
        };
        return Some((kind, [v as f32, 0.0, 0.0, 0.0], spelling));
    }

    // A vector constructor of literals: `vec3<f32>(…)`, `vec3f(…)`, `vec3(…)`.
    let open = init.iter().position(|t| t.tok == Tok::Punct(b'('))?;
    let ctor: String = init[..open].iter().map(|t| &src[t.span.clone()]).collect();
    let (n, typed) = match ctor.as_str() {
        "vec2<f32>" | "vec2f" => (2, true),
        "vec3<f32>" | "vec3f" => (3, true),
        "vec4<f32>" | "vec4f" => (4, true),
        "vec2" => (2, false),
        "vec3" => (3, false),
        "vec4" => (4, false),
        _ => return None,
    };
    if !(declared.is_empty() || declared == ctor || declared == format!("vec{n}<f32>")) {
        return None;
    }
    if init.last()?.tok != Tok::Punct(b')') {
        return None;
    }
    let mut args = &init[open + 1..init.len() - 1];
    let mut values = Vec::new();
    let mut any_float = false;
    while !args.is_empty() {
        let (v, suffix, float, used) = signed_number(src, args)?;
        if matches!(suffix, "i" | "u") {
            return None;
        }
        any_float |= float;
        values.push(v as f32);
        args = &args[used..];
        match args.first() {
            Some(t) if t.tok == Tok::Punct(b',') => args = &args[1..],
            Some(_) => return None,
            None => {}
        }
    }
    // An untyped `vec3(1, 2, 3)` is a vector of abstract integers.
    if !(typed || any_float || declared.contains("f32")) {
        return None;
    }
    let value = match values.len() {
        1 => [values[0]; 4],
        len if len == n => {
            let mut v = [0.0; 4];
            v[..n].copy_from_slice(&values);
            v
        }
        _ => return None,
    };
    let mut value = value;
    value[n..].fill(0.0);
    Some((Kind::Vec(n as u8), value, Spelling::Vector { ctor }))
}

/// `tweak: 0..2`, `tweak: color`, `tweak: off` (the colon is optional).
fn parse_hint(comment: &str) -> Option<Hint> {
    let rest = comment.trim().strip_prefix("tweak")?;
    let rest = rest.trim_start().trim_start_matches(':').trim();
    let word = rest.split_whitespace().next().unwrap_or("");
    Some(match word {
        "" => Hint::Auto,
        "off" => Hint::Off,
        "color" | "colour" => Hint::Color,
        range => {
            let (lo, hi) = range.split_once("..")?;
            let (lo, hi): (f32, f32) = (lo.parse().ok()?, hi.parse().ok()?);
            if lo < hi {
                Hint::Range(lo, hi)
            } else {
                return None;
            }
        }
    })
}

/// Names of the constants that appear where WGSL needs a true constant: in
/// another module-scope `const`'s initializer or as an array size.
/// Rewriting those into `var<private>` would not compile.
pub fn const_context_names(src: &str) -> Vec<String> {
    let (tokens, _) = lex(src);
    let text = |i: usize| tokens.get(i).map(|t| &src[t.span.clone()]);
    let mut names = Vec::new();
    let mut depth = 0i32;
    let mut in_const = false;
    for (i, t) in tokens.iter().enumerate() {
        match t.tok {
            Tok::Punct(b'{') => depth += 1,
            Tok::Punct(b'}') => depth -= 1,
            Tok::Punct(b';') if depth == 0 => in_const = false,
            Tok::Ident => {
                let word = text(i).unwrap_or("");
                if depth == 0 && matches!(word, "const" | "override") {
                    in_const = true;
                    continue;
                }
                // `array<T, NAME>`
                let array_size = text(i.wrapping_sub(1)) == Some(",") && text(i + 1) == Some(">");
                // In a const initializer, after its `=`.
                let in_init = in_const
                    && (0..i)
                        .rev()
                        .take_while(|&j| !matches!(text(j), Some("const" | "override")))
                        .any(|j| text(j) == Some("="));
                if array_size || in_init {
                    names.push(word.to_string());
                }
            }
            _ => {}
        }
    }
    names
}

/// `src` with `constants` (slot = position) turned into `var<private>`s,
/// plus the [`LOADER`] function that fills them from `i_tweaks`. Always
/// appends the loader, empty for no constants, since the entry points call it.
pub fn instrument(src: &str, constants: &[Constant]) -> String {
    let mut out = src.to_string();
    let mut by_position: Vec<&Constant> = constants.iter().collect();
    by_position.sort_by_key(|c| std::cmp::Reverse(c.keyword.start));
    for c in by_position {
        out.replace_range(c.keyword.clone(), "var<private>");
    }
    out.push_str(&format!("\nfn {LOADER}() {{\n"));
    for (slot, c) in constants.iter().enumerate() {
        out.push_str(&format!("    {} = {};\n", c.name, c.load(slot)));
    }
    out.push_str("}\n");
    out
}

/// Write `value` into the source as the new literal of the constant named
/// like `constant`. Looks it up afresh, since the text may have changed
/// since the compile; returns false if it is gone or no longer the same kind.
pub fn write_back(src: &mut String, constant: &Constant, value: [f32; 4]) -> bool {
    let Some(current) = extract(src)
        .into_iter()
        .find(|c| c.name == constant.name && c.kind == constant.kind)
    else {
        return false;
    };
    let new = current.spell(value);
    if src[current.init.clone()] != new {
        src.replace_range(current.init.clone(), &new);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
import package::shadertoy::i_time;
const A: f32 = 0.6;
const B = 4.;
const N: i32 = 8; // tweak: 1..32
const U = 3u;
const COUNT = 8;
const SEA_COLOR: vec3<f32> = vec3<f32>(0.48, 0.54, -0.36);
const SPLAT = vec2f(1.5);
const PI: f32 = 3.14159; // tweak: off
const M: mat2x2<f32> = mat2x2<f32>(1.0, 0.0, 0.0, 1.0);
const LUT: array<f32, 2> = array<f32, 2>(1.0, 2.0);
const TWICE: f32 = A * 2.0;
fn f() {
    const LOCAL: f32 = 1.0;
}
/* const HIDDEN: f32 = 1.0; */
";

    #[test]
    fn finds_literal_constants() {
        let found = extract(SRC);
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["A", "B", "N", "U", "SEA_COLOR", "SPLAT", "PI"],
            "untyped ints, matrices, arrays, expressions, locals and comments are skipped"
        );
        let by = |n: &str| found.iter().find(|c| c.name == n).unwrap();
        assert_eq!(by("A").kind, Kind::F32);
        assert_eq!(by("B").value[0], 4.0);
        assert_eq!(by("N").kind, Kind::I32);
        assert_eq!(by("N").hint, Hint::Range(1.0, 32.0));
        assert_eq!(by("U").kind, Kind::U32);
        assert_eq!(by("SEA_COLOR").kind, Kind::Vec(3));
        assert_eq!(by("SEA_COLOR").value, [0.48, 0.54, -0.36, 0.0]);
        assert!(by("SEA_COLOR").is_color());
        assert_eq!(by("SPLAT").value, [1.5, 1.5, 0.0, 0.0]);
        assert_eq!(by("PI").hint, Hint::Off);
    }

    #[test]
    fn finds_const_contexts() {
        let names = const_context_names(SRC);
        assert!(names.contains(&"A".to_string()), "{names:?}");
        assert!(!names.contains(&"B".to_string()), "{names:?}");
        let sized = const_context_names("const N: i32 = 4;\nvar<private> a: array<f32, N>;");
        assert_eq!(sized, ["N"]);
    }

    #[test]
    fn instrument_keeps_lines() {
        let found = extract(SRC);
        let out = instrument(SRC, &found[..2]);
        for (before, after) in SRC.lines().zip(out.lines()) {
            if before.starts_with("const A") || before.starts_with("const B") {
                assert!(after.starts_with("var<private> "), "{after}");
            } else {
                assert_eq!(before, after);
            }
        }
        assert!(
            out.contains("A = package::shadertoy::i_tweak(0u).x;"),
            "{out}"
        );
        assert!(
            out.contains("B = package::shadertoy::i_tweak(1u).x;"),
            "{out}"
        );
    }

    #[test]
    fn writes_values_back_in_the_same_spelling() {
        let found = extract(SRC);
        let by = |n: &str| found.iter().find(|c| c.name == n).unwrap().clone();
        let mut src = SRC.to_string();
        assert!(write_back(&mut src, &by("A"), [1.25, 0.0, 0.0, 0.0]));
        assert!(write_back(&mut src, &by("N"), [12.4, 0.0, 0.0, 0.0]));
        assert!(write_back(&mut src, &by("U"), [5.0, 0.0, 0.0, 0.0]));
        assert!(write_back(&mut src, &by("SEA_COLOR"), [0.1, 0.2, 0.3, 0.0]));
        assert!(write_back(&mut src, &by("B"), [0.000123, 0.0, 0.0, 0.0]));
        assert!(src.contains("const A: f32 = 1.25;"));
        assert!(src.contains("const N: i32 = 12; // tweak: 1..32"));
        assert!(src.contains("const U = 5u;"));
        assert!(src.contains("vec3<f32>(0.1, 0.2, 0.3);"));
        assert!(src.contains("const B = 0.000123;"), "{src}");
        // Gone from the text since the compile: nothing to write to.
        let mut other = "const Z: f32 = 1.0;".to_string();
        assert!(!write_back(&mut other, &by("A"), [2.0; 4]));
    }

    #[test]
    fn float_literals_stay_floats() {
        assert_eq!(float_literal(4.0), "4.0");
        assert_eq!(float_literal(0.6), "0.6");
        assert_eq!(float_literal(-2.5), "-2.5");
        assert_eq!(float_literal(1234.5678), "1234.6");
        assert_eq!(float_literal(0.0), "0.0");
    }

    #[test]
    fn ranges_follow_the_value() {
        let c = |kind, hint| Constant {
            name: "X".into(),
            kind,
            value: [0.0; 4],
            hint,
            keyword: 0..0,
            init: 0..0,
            spelling: Spelling::Scalar {
                suffix: String::new(),
            },
        };
        assert_eq!(c(Kind::F32, Hint::Auto).range(0.6), (0.0, 1.2));
        assert_eq!(c(Kind::F32, Hint::Auto).range(-2.0), (-4.0, 0.0));
        assert_eq!(c(Kind::F32, Hint::Auto).range(0.0), (-1.0, 1.0));
        assert_eq!(c(Kind::I32, Hint::Auto).range(0.0), (-1.0, 1.0));
        assert_eq!(c(Kind::U32, Hint::Auto).range(0.0), (0.0, 1.0));
        assert_eq!(c(Kind::I32, Hint::Auto).range(3.0), (0.0, 6.0));
        assert_eq!(c(Kind::F32, Hint::Range(1.0, 5.0)).range(0.6), (1.0, 5.0));
    }
}
