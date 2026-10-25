//! Lexical scans of WESL source, shared by the build script (which includes
//! this file to index LYGIA) and the app (which scans the editor text).
//!
//! There is no parser here: comments are blanked out and what is left is
//! matched token by token. That is enough to find the modules a source
//! imports, the feature flags its `@if` attributes test and the names it
//! reads from the `constants::` virtual module, and it also works on text
//! that does not compile (yet).

/// `src` with every comment replaced by spaces (newlines kept), so byte
/// offsets and line numbers still match the original. Block comments nest,
/// as in WGSL.
pub fn blank_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut depth = 0usize;
    let mut line_comment = false;
    while i < bytes.len() {
        let pair = &bytes[i..(i + 2).min(bytes.len())];
        if line_comment {
            if bytes[i] == b'\n' {
                line_comment = false;
            }
        } else if pair == b"/*" {
            depth += 1;
            out.push_str("  ");
            i += 2;
            continue;
        } else if depth > 0 && pair == b"*/" {
            depth -= 1;
            out.push_str("  ");
            i += 2;
            continue;
        } else if depth == 0 && pair == b"//" {
            line_comment = true;
        }
        let ch = src[i..].chars().next().expect("on a char boundary");
        if (line_comment || depth > 0) && ch != '\n' {
            // One space per byte keeps the offsets.
            out.extend(std::iter::repeat_n(' ', ch.len_utf8()));
        } else {
            out.push(ch);
        }
        i += ch.len_utf8();
    }
    out
}

/// The tokens of comment-free `src`: identifiers (and numbers), `::`, and
/// every other non-blank character on its own.
fn tokens(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut chars = src.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        let mut end = start + ch.len_utf8();
        if ch.is_alphanumeric() || ch == '_' {
            while let Some(&(i, c)) = chars.peek() {
                if !(c.is_alphanumeric() || c == '_') {
                    break;
                }
                end = i + c.len_utf8();
                chars.next();
            }
        } else if ch == ':' && chars.peek().is_some_and(|&(_, c)| c == ':') {
            chars.next();
            end += 1;
        }
        out.push(&src[start..end]);
    }
    out
}

fn is_ident(token: &str) -> bool {
    token
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
}

/// Every path `src` names in the package `package`, without the package
/// itself: each leaf of its `import` trees
/// (`import lygia::color::{blend::darken::blendDarken, luma::luma};` names
/// `color::blend::darken::blendDarken` and `color::luma::luma`) and each
/// inline reference (`lygia::math::consts::PI`). In order of appearance,
/// with repeats.
pub fn package_paths(src: &str, package: &str) -> Vec<Vec<String>> {
    let blanked = blank_comments(src);
    let toks = tokens(&blanked);
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if toks[i] == "import" {
            let mut leaves = Vec::new();
            i = import_tree(&toks, i + 1, Vec::new(), &mut leaves);
            out.extend(
                leaves
                    .into_iter()
                    .filter(|path| path.first().is_some_and(|p| p == package))
                    .map(|mut path| {
                        path.remove(0);
                        path
                    }),
            );
        } else if toks[i] == package
            && toks.get(i + 1) == Some(&"::")
            && (i == 0 || toks[i - 1] != "::")
        {
            let mut path = Vec::new();
            i += 1;
            while toks.get(i) == Some(&"::") && toks.get(i + 1).is_some_and(|t| is_ident(t)) {
                path.push(toks[i + 1].to_string());
                i += 2;
            }
            out.push(path);
        } else {
            i += 1;
        }
    }
    out
}

/// Parse the import tree starting at `toks[i]` under `prefix`, pushing each
/// leaf path to `out`; returns the index after the tree (past the `;` for a
/// whole statement). Stops quietly at anything malformed.
fn import_tree(
    toks: &[&str],
    mut i: usize,
    mut prefix: Vec<String>,
    out: &mut Vec<Vec<String>>,
) -> usize {
    loop {
        match toks.get(i).copied() {
            Some("{") => {
                i += 1;
                while toks.get(i).is_some_and(|t| *t != "}") {
                    i = import_tree(toks, i, prefix.clone(), out);
                    if toks.get(i) == Some(&",") {
                        i += 1;
                    }
                }
                i += 1; // the `}`
                break;
            }
            Some(t) if is_ident(t) => {
                prefix.push(t.to_string());
                i += 1;
                if toks.get(i) == Some(&"::") {
                    i += 1;
                    continue;
                }
                if toks.get(i) == Some(&"as") {
                    i += 2;
                }
                out.push(prefix);
                break;
            }
            _ => return i,
        }
    }
    if toks.get(i) == Some(&";") {
        i += 1;
    }
    i
}

/// Unique feature flags tested by the `@if(…)` / `@elif(…)` attributes of
/// `src`, in order of first appearance.
// Only build.rs scans flags so far.
#[allow(dead_code)]
pub fn flag_names(src: &str) -> Vec<String> {
    let blanked = blank_comments(src);
    let toks = tokens(&blanked);
    let mut out: Vec<String> = Vec::new();
    for (i, w) in toks.windows(3).enumerate() {
        if !(w[0] == "@" && (w[1] == "if" || w[1] == "elif") && w[2] == "(") {
            continue;
        }
        let mut depth = 0;
        for &t in &toks[i + 2..] {
            match t {
                "(" => depth += 1,
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ if is_ident(t) && t != "true" && t != "false" && !out.iter().any(|f| f == t) => {
                    out.push(t.to_string());
                }
                _ => {}
            }
        }
    }
    out
}

/// Unique names `src` reads from the `constants::` virtual module, in order
/// of first appearance.
// Only build.rs scans constants so far.
#[allow(dead_code)]
pub fn constant_names(src: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for path in package_paths(src, "constants") {
        if let Some(name) = path.into_iter().next()
            && !out.contains(&name)
        {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_blanked_in_place() {
        let src = "a // b\n/* c /* d */ e */f\n";
        let blanked = blank_comments(src);
        assert_eq!(blanked.len(), src.len());
        assert_eq!(blanked, "a     \n                 f\n");
    }

    #[test]
    fn package_paths_expand_import_trees() {
        let src = "@if(X)\nimport lygia::color::{blend::darken::blendDarken, luma::luma as l};\n\
                   import package::shadertoy::i_time;\n\
                   import lygia::math::consts::{\n    PI,\n    TAU,\n};\n\
                   // import lygia::commented::out;\n\
                   fn f() -> f32 { return lygia::math::consts::PI + x::lygia::no; }\n";
        let paths: Vec<String> = package_paths(src, "lygia")
            .iter()
            .map(|p| p.join("::"))
            .collect();
        assert_eq!(
            paths,
            [
                "color::blend::darken::blendDarken",
                "color::luma::luma",
                "math::consts::PI",
                "math::consts::TAU",
                "math::consts::PI",
            ]
        );
    }

    #[test]
    fn flags_and_constants() {
        let src = "@if(MARBLE_PERIOD)\nimport constants::MARBLE_PERIOD;\n\
                   @if(A && !B) fn f() {}\n@elif((C || true))\nfn f() {}\n\
                   // @if(COMMENTED)\n\
                   fn g() -> vec2f { return constants::CENTER_2D; }\n";
        assert_eq!(flag_names(src), ["MARBLE_PERIOD", "A", "B", "C"]);
        assert_eq!(constant_names(src), ["MARBLE_PERIOD", "CENTER_2D"]);
    }
}
