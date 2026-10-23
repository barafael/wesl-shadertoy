//! A small hand-written WGSL/WESL tokenizer that produces an egui
//! [`LayoutJob`] with per-token colors.
//!
//! This is the "syntax highlighting" layer: the editor widget renders this
//! job through `TextEdit`'s layouter, so highlighting and editing share the
//! exact same glyph layout. Token colors come from the active
//! [`crate::theme::Theme`]'s [`Syntax`] palette.

use bevy_egui::egui::Color32;
use bevy_egui::egui::FontId;
use bevy_egui::egui::text::{LayoutJob, TextFormat};

use crate::theme::Syntax;

const KEYWORDS: &[&str] = &[
    // WGSL
    "fn",
    "let",
    "var",
    "const",
    "struct",
    "return",
    "if",
    "else",
    "for",
    "while",
    "loop",
    "switch",
    "case",
    "break",
    "continue",
    "fallthrough",
    "discard",
    "true",
    "false",
    "alias",
    "override",
    "type",
    "enable",
    "requires",
    "diagnostic",
    "const_assert",
    // WESL
    "public",
    "private",
];

const IMPORT_KEYWORDS: &[&str] = &["import", "package", "super", "self"];

const TYPES: &[&str] = &[
    "bool",
    "i32",
    "u32",
    "f32",
    "f16",
    "f64",
    "atomic",
    "array",
    "ptr",
    "mat2x2",
    "mat2x3",
    "mat2x4",
    "mat3x2",
    "mat3x3",
    "mat3x4",
    "mat4x2",
    "mat4x3",
    "mat4x4",
    "sampler",
    "sampler_comparison",
    "texture_1d",
    "texture_2d",
    "texture_2d_array",
    "texture_3d",
    "texture_cube",
    "texture_cube_array",
    "texture_multisampled_2d",
    "texture_depth_2d",
    "texture_depth_2d_array",
    "texture_depth_cube",
    "texture_storage_1d",
    "texture_storage_2d",
    "texture_storage_3d",
];

const BUILTINS: &[&str] = &[
    "abs",
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atan2",
    "atanh",
    "bitcast",
    "ceil",
    "clamp",
    "cos",
    "cosh",
    "countLeadingZeros",
    "countOneBits",
    "countTrailingZeros",
    "cross",
    "degrees",
    "determinant",
    "distance",
    "dot",
    "dot4U8Packed",
    "exp",
    "exp2",
    "extractBits",
    "faceForward",
    "findLSB",
    "findMSB",
    "firstLeadingBit",
    "firstTrailingBit",
    "floor",
    "fma",
    "fract",
    "frexp",
    "insertBits",
    "inverse",
    "inverseSqrt",
    "ldexp",
    "length",
    "log",
    "log2",
    "max",
    "min",
    "mix",
    "modf",
    "normalize",
    "pack4x8snorm",
    "pack4x8unorm",
    "pack2x16snorm",
    "pack2x16unorm",
    "pack2x16float",
    "pow",
    "quantizeToF16",
    "radians",
    "reflect",
    "refract",
    "reverseBits",
    "round",
    "saturate",
    "select",
    "sign",
    "sin",
    "sinh",
    "smoothstep",
    "sqrt",
    "step",
    "storageBarrier",
    "tan",
    "tanh",
    "textureDimensions",
    "textureGather",
    "textureGatherCompare",
    "textureLoad",
    "textureNumLayers",
    "textureNumLevels",
    "textureNumSamples",
    "textureSample",
    "textureSampleBias",
    "textureSampleCompare",
    "textureSampleCompareLevel",
    "textureSampleGrad",
    "textureSampleLevel",
    "textureStore",
    "transpose",
    "trunc",
    "unpack4x8snorm",
    "unpack4x8unorm",
    "unpack2x16snorm",
    "unpack2x16unorm",
    "unpack2x16float",
    "workgroupBarrier",
    "workgroupMemoryBarrier",
    "workgroupUniformLoad",
    // Vector constructors that read like types but are used like calls. (The
    // scalar and matrix constructors are already covered by `TYPES`, which
    // `classify_word` checks first.)
    "vec2",
    "vec3",
    "vec4",
    "bool2",
    "bool3",
    "bool4",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Default,
    Comment,
    String,
    Number,
    Keyword,
    Import,
    Type,
    Builtin,
    Attribute,
}

impl Kind {
    fn color(self, default: Color32, pal: &Syntax) -> Color32 {
        match self {
            Kind::Default => default,
            Kind::Comment => pal.comment,
            Kind::String => pal.string,
            Kind::Number => pal.number,
            Kind::Keyword => pal.keyword,
            Kind::Import => pal.import,
            Kind::Type => pal.ty,
            Kind::Builtin => pal.builtin,
            Kind::Attribute => pal.attribute,
        }
    }
}

struct Token {
    range: std::ops::Range<usize>,
    kind: Kind,
}

fn classify_word(word: &str) -> Kind {
    if KEYWORDS.contains(&word) {
        Kind::Keyword
    } else if IMPORT_KEYWORDS.contains(&word) {
        Kind::Import
    } else if TYPES.contains(&word) {
        Kind::Type
    } else if BUILTINS.contains(&word) {
        Kind::Builtin
    } else {
        Kind::Default
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_digit_start(c: char) -> bool {
    c.is_ascii_digit()
}

/// Tokenize `src` into colored runs.
fn tokenize(src: &str) -> Vec<Token> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i] as char;
        let start = i;

        // Line comment
        if c == '/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            tokens.push(Token {
                range: start..i,
                kind: Kind::Comment,
            });
            continue;
        }

        // Block comment (WGSL block comments nest)
        if c == '/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            tokens.push(Token {
                range: start..i,
                kind: Kind::Comment,
            });
            continue;
        }

        // String (no escapes needed for shader sources in practice)
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            while i < bytes.len() && bytes[i] != quote as u8 {
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            tokens.push(Token {
                range: start..i,
                kind: Kind::String,
            });
            continue;
        }

        // Attribute: @name
        if c == '@' {
            i += 1;
            while i < bytes.len() && is_ident_char(bytes[i] as char) {
                i += 1;
            }
            tokens.push(Token {
                range: start..i,
                kind: Kind::Attribute,
            });
            continue;
        }

        // Number: decimals, hex, floats with exponent / f16-hint suffixes
        if is_digit_start(c) {
            while i < bytes.len() {
                let b = bytes[i] as char;
                if b.is_ascii_alphanumeric() || b == '.' || b == '_' {
                    i += 1;
                } else {
                    break;
                }
            }
            tokens.push(Token {
                range: start..i,
                kind: Kind::Number,
            });
            continue;
        }

        // Identifier / keyword
        if is_ident_start(c) {
            while i < bytes.len() && is_ident_char(bytes[i] as char) {
                i += 1;
            }
            let word = &src[start..i];
            tokens.push(Token {
                range: start..i,
                kind: classify_word(word),
            });
            continue;
        }

        // Anything else (punctuation, whitespace): one byte at a time is fine,
        // runs of the same kind merge visually.
        i += 1;
        while i < bytes.len() {
            let b = bytes[i] as char;
            if b == '/'
                || b == '"'
                || b == '\''
                || b == '@'
                || is_ident_start(b)
                || is_digit_start(b)
            {
                break;
            }
            i += 1;
        }
        tokens.push(Token {
            range: start..i,
            kind: Kind::Default,
        });
    }

    tokens
}

/// Build a [`LayoutJob`] with WESL/WGSL syntax colors.
///
/// Wrapping is disabled (`max_width = INFINITY`): the editor scrolls
/// horizontally instead, which keeps the line-number gutter aligned.
pub fn highlight_job(
    src: &str,
    font_id: FontId,
    default_color: Color32,
    pal: &Syntax,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    for token in tokenize(src) {
        job.append(
            &src[token.range],
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: token.kind.color(default_color, pal),
                ..Default::default()
            },
        );
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_comments_and_keywords() {
        let src = "// hi\nfn main_image(a: vec2<f32>) -> vec4<f32> { return vec4(1.0); }";
        let toks = tokenize(src);
        assert_eq!(toks[0].kind, Kind::Comment);
        assert!(toks.iter().any(|t| t.kind == Kind::Keyword));
        assert!(toks.iter().any(|t| t.kind == Kind::Type));
        assert!(toks.iter().any(|t| t.kind == Kind::Number));
    }

    #[test]
    fn nested_block_comments() {
        let src = "/* a /* b */ c */ fn";
        let toks = tokenize(src);
        assert_eq!(toks[0].range, 0..17);
        assert_eq!(toks[0].kind, Kind::Comment);
    }

    #[test]
    fn attributes() {
        let src = "@group(0) @binding(2) var<uniform> u: S;";
        let toks = tokenize(src);
        assert_eq!(toks[0].kind, Kind::Attribute);
    }
}
