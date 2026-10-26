//! A random generator of small, well-typed `main_image` shaders.
//!
//! Each shader binds a few `let`s of type `f32`, `vec2<f32>` and `vec3<f32>`,
//! each a random expression tree over `frag_coord`, `i_time()`,
//! `i_resolution()`, literals, the earlier `let`s, arithmetic and a handful
//! of builtins, and maps the last `vec3` to a colour. Inputs to builtins with
//! a restricted domain are kept inside it (`pow` of a clamped base, division
//! by `1 + |x|`), so every pixel is finite and well defined, and a backend
//! that disagrees is wrong rather than differently undefined. The same seed
//! always yields the same text.
//!
//! Literals carry an `f` suffix. Spelled as abstract floats, a subtree made
//! of literals only is a constant expression, and naga 30 cannot
//! const-evaluate `mix` or `smoothstep`: it rejects valid WGSL such as
//! `let v: f32 = smoothstep(0.0, 1.0, 0.5);` (found by this generator, seed
//! 1 before the suffix; `tests/naga_regressions.rs` keeps the reproduction).

use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ty {
    F32,
    Vec2,
    Vec3,
}

impl Ty {
    const ALL: [Ty; 3] = [Ty::F32, Ty::Vec2, Ty::Vec3];

    fn wgsl(self) -> &'static str {
        match self {
            Ty::F32 => "f32",
            Ty::Vec2 => "vec2<f32>",
            Ty::Vec3 => "vec3<f32>",
        }
    }

    /// `x` (an `f32` expression) splatted to this type.
    fn splat(self, x: &str) -> String {
        match self {
            Ty::F32 => x.to_string(),
            _ => format!("{}({x})", self.wgsl()),
        }
    }
}

/// SplitMix64: tiny, seedable, and good enough to pick tree shapes.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n)) as u32
    }

    /// An `f32` literal in `lo..hi`, two decimals.
    fn literal(&mut self, lo: f32, hi: f32) -> String {
        let x = lo + (hi - lo) * self.below(10_000) as f32 / 10_000.0;
        format!("{x:.2}f")
    }
}

struct Generator {
    rng: Rng,
    /// The `let`s bound so far, by type.
    bound: Vec<(String, Ty)>,
}

impl Generator {
    fn expr(&mut self, ty: Ty, depth: u32) -> String {
        if depth == 0 || self.rng.below(5) == 0 {
            return self.leaf(ty);
        }
        let d = depth - 1;
        match self.rng.below(14) {
            0 => format!("sin({})", self.expr(ty, d)),
            1 => format!("cos({})", self.expr(ty, d)),
            2 => format!("fract({})", self.expr(ty, d)),
            3 => format!("abs({})", self.expr(ty, d)),
            4 => {
                let op = ["+", "-", "*"][self.rng.below(3) as usize];
                format!("({} {op} {})", self.expr(ty, d), self.expr(ty, d))
            }
            5 => format!("({} * {})", self.expr(ty, d), self.expr(Ty::F32, d)),
            6 => format!("({} / (1.0 + abs({})))", self.expr(ty, d), self.expr(ty, d)),
            7 => format!(
                "mix({}, {}, clamp({}, 0.0, 1.0))",
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(Ty::F32, d)
            ),
            8 => {
                let lo = self.rng.literal(-1.0, 0.5);
                let hi = self.rng.literal(0.6, 2.0);
                format!(
                    "smoothstep({}, {}, {})",
                    ty.splat(&lo),
                    ty.splat(&hi),
                    self.expr(ty, d)
                )
            }
            9 => {
                let lo = self.rng.literal(-2.0, 0.0);
                let hi = self.rng.literal(0.1, 2.0);
                format!(
                    "clamp({}, {}, {})",
                    self.expr(ty, d),
                    ty.splat(&lo),
                    ty.splat(&hi)
                )
            }
            10 => {
                let k = self.rng.literal(0.5, 3.0);
                format!(
                    "pow(clamp(abs({}), {}, {}), {})",
                    self.expr(ty, d),
                    ty.splat("0.01"),
                    ty.splat("4.0"),
                    ty.splat(&k)
                )
            }
            11 => format!(
                "select({}, {}, {} < {})",
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(Ty::F32, d),
                self.expr(Ty::F32, d)
            ),
            // Changes of type: reductions, swizzles, constructors.
            _ => self.convert(ty, d),
        }
    }

    fn convert(&mut self, ty: Ty, d: u32) -> String {
        match ty {
            Ty::F32 => match self.rng.below(4) {
                0 => format!("length({})", self.expr(Ty::Vec2, d)),
                1 => format!("length({})", self.expr(Ty::Vec3, d)),
                2 => format!(
                    "dot({}, {})",
                    self.expr(Ty::Vec3, d),
                    self.expr(Ty::Vec3, d)
                ),
                _ => format!("({}).y", self.expr(Ty::Vec2, d)),
            },
            Ty::Vec2 => match self.rng.below(2) {
                0 => format!("({}).zx", self.expr(Ty::Vec3, d)),
                _ => format!(
                    "vec2<f32>({}, {})",
                    self.expr(Ty::F32, d),
                    self.expr(Ty::F32, d)
                ),
            },
            Ty::Vec3 => match self.rng.below(2) {
                0 => format!(
                    "vec3<f32>({}, {})",
                    self.expr(Ty::Vec2, d),
                    self.expr(Ty::F32, d)
                ),
                _ => format!("({}).xyx", self.expr(Ty::Vec2, d)),
            },
        }
    }

    fn leaf(&mut self, ty: Ty) -> String {
        let bound: Vec<String> = self
            .bound
            .iter()
            .filter(|(_, t)| *t == ty)
            .map(|(name, _)| name.clone())
            .collect();
        if !bound.is_empty() && self.rng.below(2) == 0 {
            return bound[self.rng.below(bound.len() as u32) as usize].clone();
        }
        match (ty, self.rng.below(3)) {
            (Ty::F32, 0) => "t".into(),
            (Ty::F32, 1) => "uv.x".into(),
            (Ty::F32, _) => self.rng.literal(-2.0, 2.0),
            (Ty::Vec2, 0 | 1) => "uv".into(),
            (Ty::Vec2, _) => {
                let (a, b) = (self.rng.literal(-2.0, 2.0), self.rng.literal(-2.0, 2.0));
                format!("vec2<f32>({a}, {b})")
            }
            (Ty::Vec3, 0) => "vec3<f32>(uv, t)".into(),
            (Ty::Vec3, 1) => "vec3<f32>(uv.y, t, uv.x)".into(),
            (Ty::Vec3, _) => {
                let c: [String; 3] = std::array::from_fn(|_| self.rng.literal(-2.0, 2.0));
                format!("vec3<f32>({}, {}, {})", c[0], c[1], c[2])
            }
        }
    }
}

/// The user module for `seed`: a `main_image` as the editor would hold it.
pub fn shader(seed: u64) -> String {
    let mut g = Generator {
        rng: Rng(seed),
        bound: Vec::new(),
    };
    let mut body = String::new();
    let lets = 2 + g.rng.below(4);
    for i in 0..lets {
        // The last binding is the colour, so it is always a vec3.
        let ty = if i + 1 == lets {
            Ty::Vec3
        } else {
            Ty::ALL[g.rng.below(3) as usize]
        };
        let name = format!("v{i}");
        let value = g.expr(ty, 4);
        let _ = writeln!(body, "    let {name}: {} = {value};", ty.wgsl());
        g.bound.push((name, ty));
    }
    format!(
        "// backend-diff fuzz shader, seed {seed}\n\
         import package::shadertoy::{{i_resolution, i_time}};\n\n\
         fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {{\n\
         \x20   let uv = (frag_coord - 0.5 * i_resolution().xy) / i_resolution().y;\n\
         \x20   let t = i_time();\n\
         {body}\
         \x20   return vec4<f32>(0.5 + 0.5 * cos(v{}), 1.0);\n\
         }}\n",
        lets - 1
    )
}

/// Types of the integer generator: the two integers, and `f32` for the
/// conversions between them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ITy {
    I32,
    U32,
    F32,
}

impl ITy {
    const ALL: [ITy; 3] = [ITy::I32, ITy::U32, ITy::F32];

    fn wgsl(self) -> &'static str {
        match self {
            ITy::I32 => "i32",
            ITy::U32 => "u32",
            ITy::F32 => "f32",
        }
    }

    /// The edge-value table function for this type.
    fn table(self) -> &'static str {
        match self {
            ITy::I32 => "ei",
            ITy::U32 => "eu",
            ITy::F32 => "ef",
        }
    }
}

/// Edge-value tables and the colour hash, shared by every integer shader.
/// The tables are indexed at run time, so naga cannot fold the edge cases
/// away; the hash turns any bit of difference into a different colour.
const INT_PRELUDE: &str = "\
fn show(v: u32) -> vec4<f32> {
    var h = v * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    return vec4<f32>(f32(h & 255u), f32((h >> 8u) & 255u), f32((h >> 16u) & 255u), 255.0) / 255.0;
}

fn ei(k: u32) -> i32 {
    var t = array<i32, 16>(0, 1, -1, 2, -2, 7, -7, 3, 31, 32, 33, 2147483647, -2147483647 - 1, 65536, -65536, 1000000);
    return t[k & 15u];
}

fn eu(k: u32) -> u32 {
    var t = array<u32, 16>(0u, 1u, 2u, 3u, 7u, 31u, 32u, 33u, 63u, 0x80000000u, 0xffffffffu, 0xfffffffeu, 0x7fffffffu, 0x10000u, 0xdeadbeefu, 5u);
    return t[k & 15u];
}

fn ef(k: u32) -> f32 {
    var t = array<f32, 16>(0.0, -0.5, 0.5, 1.5, -1.5, 2.5, -2.5, 3e9, -3e9, 5e9, 16777217.0, 4294967040.0, 2147483520.0, -2147483904.0, 1e20, -1e20);
    return t[k & 15u];
}
";

/// The integer generator: like [`Generator`], over [`ITy`]. Every value is
/// defined by WGSL for every input (division by zero, `INT_MIN / -1`,
/// shifts past the width, out-of-range float conversions all have a
/// specified result), and the floats stay finite: they are only added,
/// halved and rounded, never multiplied or divided by run-time values.
struct IntGenerator {
    rng: Rng,
    bound: Vec<(String, ITy)>,
}

impl IntGenerator {
    fn expr(&mut self, ty: ITy, depth: u32) -> String {
        if depth == 0 || self.rng.below(5) == 0 {
            return self.leaf(ty);
        }
        let d = depth - 1;
        match ty {
            ITy::F32 => self.float(d),
            _ => self.int(ty, d),
        }
    }

    fn int(&mut self, ty: ITy, d: u32) -> String {
        let w = ty.wgsl();
        match self.rng.below(16) {
            0..=3 => {
                let op = ["+", "-", "*", "/", "%", "&", "|", "^"][self.rng.below(8) as usize];
                format!("({} {op} {})", self.expr(ty, d), self.expr(ty, d))
            }
            4 => {
                let op = ["/", "%"][self.rng.below(2) as usize];
                format!("({} {op} {})", self.expr(ty, d), self.expr(ty, d))
            }
            5 => {
                let op = ["<<", ">>"][self.rng.below(2) as usize];
                format!("({} {op} {})", self.expr(ty, d), self.expr(ITy::U32, d))
            }
            6 => match ty {
                ITy::I32 => format!("(-{})", self.expr(ty, d)),
                _ => format!("(~{})", self.expr(ty, d)),
            },
            7 => {
                let f = ["min", "max"][self.rng.below(2) as usize];
                format!("{f}({}, {})", self.expr(ty, d), self.expr(ty, d))
            }
            8 => format!(
                "clamp({}, {}, {})",
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(ty, d)
            ),
            9 => format!(
                "select({}, {}, {} < {})",
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(ty, d)
            ),
            // A vector select, which GLSL spells as `mix` with a bool vector.
            10 => format!(
                "select(vec2<{w}>({}, {}), vec2<{w}>({}, {}), vec2<bool>({} < {}, {} == {})).{}",
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(ty, d),
                ["x", "y"][self.rng.below(2) as usize]
            ),
            11 => {
                let f = [
                    "countOneBits",
                    "countLeadingZeros",
                    "countTrailingZeros",
                    "firstLeadingBit",
                    "firstTrailingBit",
                    "reverseBits",
                ][self.rng.below(6) as usize];
                format!("{f}({})", self.expr(ty, d))
            }
            12 => format!(
                "extractBits({}, {}, {})",
                self.expr(ty, d),
                self.expr(ITy::U32, d),
                self.expr(ITy::U32, d)
            ),
            13 => format!(
                "insertBits({}, {}, {}, {})",
                self.expr(ty, d),
                self.expr(ty, d),
                self.expr(ITy::U32, d),
                self.expr(ITy::U32, d)
            ),
            14 => match ty {
                ITy::I32 => format!("abs({})", self.expr(ty, d)),
                _ => format!(
                    "({} {} {})",
                    self.expr(ty, d),
                    ["/", "%"][self.rng.below(2) as usize],
                    self.expr(ty, d)
                ),
            },
            // Changes of type.
            _ => {
                let other = if ty == ITy::I32 { ITy::U32 } else { ITy::I32 };
                match self.rng.below(3) {
                    0 => format!("{w}({})", self.expr(other, d)),
                    1 => format!("bitcast<{w}>({})", self.expr(other, d)),
                    _ => format!("{w}({})", self.expr(ITy::F32, d)),
                }
            }
        }
    }

    fn float(&mut self, d: u32) -> String {
        match self.rng.below(8) {
            0 => format!("({} + {})", self.expr(ITy::F32, d), self.expr(ITy::F32, d)),
            1 => format!("({} - {})", self.expr(ITy::F32, d), self.expr(ITy::F32, d)),
            2 => {
                let f =
                    ["trunc", "floor", "ceil", "round", "abs", "fract"][self.rng.below(6) as usize];
                format!("{f}({})", self.expr(ITy::F32, d))
            }
            3 => format!(
                "({} * {})",
                self.expr(ITy::F32, d),
                ["0.5", "1.5", "-0.25"][self.rng.below(3) as usize]
            ),
            4 => format!(
                "({} % {})",
                self.expr(ITy::F32, d),
                ["1.25", "-0.75", "3.0"][self.rng.below(3) as usize]
            ),
            5 => format!("f32({})", self.expr(ITy::I32, d)),
            6 => format!("f32({})", self.expr(ITy::U32, d)),
            _ => format!(
                "select({}, {}, {} < {})",
                self.expr(ITy::F32, d),
                self.expr(ITy::F32, d),
                self.expr(ITy::F32, d),
                self.expr(ITy::F32, d)
            ),
        }
    }

    fn leaf(&mut self, ty: ITy) -> String {
        let bound: Vec<String> = self
            .bound
            .iter()
            .filter(|(_, t)| *t == ty)
            .map(|(name, _)| name.clone())
            .collect();
        if !bound.is_empty() && self.rng.below(3) == 0 {
            return bound[self.rng.below(bound.len() as u32) as usize].clone();
        }
        let k = self.rng.below(16);
        let axis = ["px", "py", "pk"][self.rng.below(3) as usize];
        match (ty, self.rng.below(4)) {
            (ITy::I32, 0) => ["ix", "iy"][self.rng.below(2) as usize].into(),
            (ITy::U32, 0) => axis.into(),
            _ => format!("{}({axis} + {k}u)", ty.table()),
        }
    }
}

/// The integer fuzz shader for `seed`: integer arithmetic over edge values,
/// conversions and bit builtins, hashed to a colour so that any difference
/// in any bit shows.
pub fn int_shader(seed: u64) -> String {
    let mut g = IntGenerator {
        rng: Rng(seed),
        bound: Vec::new(),
    };
    let mut body = String::new();
    let lets = 2 + g.rng.below(4);
    let mut last = ITy::U32;
    for i in 0..lets {
        let ty = ITy::ALL[g.rng.below(3) as usize];
        let name = format!("v{i}");
        let value = g.expr(ty, 4);
        let _ = writeln!(body, "    let {name}: {} = {value};", ty.wgsl());
        g.bound.push((name, ty));
        last = ty;
    }
    let shown = match last {
        ITy::U32 => format!("v{}", lets - 1),
        _ => format!("bitcast<u32>(v{})", lets - 1),
    };
    format!(
        "// backend-diff integer fuzz shader, seed {seed}\n\n\
         {INT_PRELUDE}\n\
         fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {{\n\
         \x20   let px = u32(frag_coord.x);\n\
         \x20   let py = u32(frag_coord.y);\n\
         \x20   let pk = px * 7u + py * 3u;\n\
         \x20   let ix = i32(px) - 128;\n\
         \x20   let iy = i32(py) - 72;\n\
         {body}\
         \x20   return show({shown});\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_shader() {
        assert_eq!(shader(7), shader(7));
        assert_ne!(shader(7), shader(8));
    }

    /// Well-typed by construction: every generated shader gets through the
    /// app's compiler and naga validation.
    #[test]
    fn generated_shaders_compile() {
        for seed in 0..300 {
            let src = shader(seed);
            if let Err(e) = wesl_shadertoy::compile::compile_user_shader(&src) {
                panic!("seed {seed}: {e}\n{src}");
            }
        }
    }

    #[test]
    fn generated_int_shaders_compile() {
        for seed in 0..300 {
            let src = int_shader(seed);
            if let Err(e) = wesl_shadertoy::compile::compile_user_shader(&src) {
                panic!("seed {seed}: {e}\n{src}");
            }
        }
    }
}
