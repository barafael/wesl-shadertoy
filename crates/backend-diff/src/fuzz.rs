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
}
