//! Export → Bevy: the document as a tiny standalone crate for Bevy 0.20, so
//! a look prototyped here drops into a game.
//!
//! The crate holds the linked WGSL (compiled by
//! [`compile::compile_for_export`], so `fragment` is its only entry point),
//! a `Plugin` that draws it with Bevy's stock
//! `bevy::core_pipeline::fullscreen_material` (whose bindings 0..=2 are the
//! ones `package::shadertoy` uses), the shadertoy uniforms as the material's
//! component, updated every frame from `Time`, the window and the mouse,
//! and the live constants as a `#[derive(Resource, Reflect)]` struct with a
//! field per constant: the WGSL reads them from `i_tweaks` exactly like the
//! Constants window drives them, so editing the resource (by hand or in an
//! inspector) changes the shader live. `examples/demo.rs` opens a window
//! with it.
//!
//! Shaders with buffer passes are refused for now: they need the app's own
//! ping-pong renderer ([`crate::render::buffers`]), not the stock material.
//!
//! Native: the crate is written to `./<name>-bevy/`. Web: it downloads as
//! `<name>-bevy.zip` (a store-only zip, written by [`zip`]).

use crate::compile;
use crate::tweak::{Constant, Hint, Kind, MAX_TWEAKS};

/// An exported crate: its directory name and `(relative path, contents)`.
pub struct Crate {
    pub name: String,
    pub files: Vec<(String, String)>,
}

/// Build the crate for the editor `text`; `title` names it (the example's
/// name, else the source's first-line title, else "shader").
pub fn export(text: &str, title: Option<&str>) -> Result<Crate, String> {
    let compiled = compile::compile_for_export(text).map_err(|e| {
        let first = e.to_string();
        let first = first.lines().take(2).collect::<Vec<_>>().join(" ");
        format!("export needs a shader that compiles ({first})")
    })?;
    if compiled.buffers != 0 {
        return Err(
            "export does not support buffer passes (buffer_a..buffer_d) yet; \
             only single-pass shaders"
                .into(),
        );
    }
    let title = title
        .map(str::to_string)
        .or_else(|| source_title(text))
        .unwrap_or_else(|| "shader".into());
    let names = Names::new(&title);
    let fields = fields(&compiled.tweaks);

    let files = vec![
        ("Cargo.toml".to_string(), cargo_toml(&names, &title)),
        ("src/lib.rs".into(), lib_rs(&names, &title, &fields)),
        ("src/shader.wgsl".into(), compiled.wgsl),
        ("src/shader.wesl".into(), text.to_string()),
        ("examples/demo.rs".into(), demo_rs(&names, &title)),
        ("README.md".into(), readme(&names, &title, text)),
        (".gitignore".into(), "/target\n".into()),
    ];
    Ok(Crate {
        name: names.dir.clone(),
        files,
    })
}

/// The title in an example-style first line, `// Name — summary`.
fn source_title(text: &str) -> Option<String> {
    let first = text.lines().next()?.strip_prefix("//")?.trim();
    let name = first
        .split(" \u{2014} ")
        .next()?
        .trim()
        .trim_end_matches('.');
    (name.len() <= 40 && name.chars().any(|c| c.is_ascii_alphanumeric())).then(|| name.to_string())
}

/// The identifiers derived from the title.
struct Names {
    /// `star-nest-bevy`: the crate and its directory.
    dir: String,
    /// `star_nest_bevy`: the crate as a Rust path.
    krate: String,
    /// `StarNest`: prefix of the generated types.
    ty: String,
}

impl Names {
    fn new(title: &str) -> Self {
        let words: Vec<String> = title
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        let mut words = if words.is_empty() {
            vec!["shader".to_string()]
        } else {
            words
        };
        // Crate names and type names must not start with a digit.
        if words[0].starts_with(|c: char| c.is_ascii_digit()) {
            words.insert(0, "shader".into());
        }
        let ty = words
            .iter()
            .map(|w| w[..1].to_ascii_uppercase() + &w[1..])
            .collect();
        Self {
            dir: format!("{}-bevy", words.join("-")),
            krate: format!("{}_bevy", words.join("_")),
            ty,
        }
    }
}

/// One field of the generated tweaks resource.
struct Field {
    /// The field's Rust name, `sea_height` for `SEA_HEIGHT`.
    name: String,
    constant: Constant,
}

impl Field {
    fn rust_type(&self) -> &'static str {
        match self.constant.kind {
            Kind::F32 => "f32",
            Kind::I32 => "i32",
            Kind::U32 => "u32",
            Kind::Vec(2) => "Vec2",
            Kind::Vec(3) => "Vec3",
            Kind::Vec(_) => "Vec4",
        }
    }

    /// The source value as a Rust expression of [`Self::rust_type`].
    fn default_value(&self) -> String {
        let v = self.constant.value;
        let f = |x: f32| format!("{x:?}");
        match self.constant.kind {
            Kind::F32 => f(v[0]),
            Kind::I32 => format!("{}", v[0].round() as i64),
            Kind::U32 => format!("{}", v[0].round().max(0.0) as u64),
            Kind::Vec(n) => {
                let parts: Vec<String> = v[..n as usize].iter().map(|x| f(*x)).collect();
                format!("{}::new({})", self.rust_type(), parts.join(", "))
            }
        }
    }

    /// The field packed into its `i_tweaks` slot (a `Vec4`).
    fn packed(&self) -> String {
        let name = &self.name;
        match self.constant.kind {
            Kind::F32 => format!("Vec4::new(self.{name}, 0.0, 0.0, 0.0)"),
            Kind::I32 | Kind::U32 => format!("Vec4::new(self.{name} as f32, 0.0, 0.0, 0.0)"),
            Kind::Vec(2) => format!("self.{name}.extend(0.0).extend(0.0)"),
            Kind::Vec(3) => format!("self.{name}.extend(0.0)"),
            Kind::Vec(_) => format!("self.{name}"),
        }
    }

    /// The doc comment: the constant's name in the shader, plus its hint.
    fn doc(&self) -> String {
        let hint = match self.constant.hint {
            Hint::Range(lo, hi) => format!(" Meant for {lo:?}..={hi:?}."),
            _ if self.constant.is_color() => " A color (linear RGB).".into(),
            _ => String::new(),
        };
        format!("`{}` in the shader.{hint}", self.constant.name)
    }
}

/// The resource fields for `tweaks` (slot = index), with unique snake_case
/// names that are not Rust keywords.
fn fields(tweaks: &[Constant]) -> Vec<Field> {
    let mut out: Vec<Field> = Vec::new();
    for c in tweaks {
        let mut name = snake_case(&c.name);
        if RUST_KEYWORDS.contains(&name.as_str()) || name.starts_with(|c: char| c.is_ascii_digit())
        {
            name.push('_');
        }
        while out.iter().any(|f| f.name == name) {
            name.push('_');
        }
        out.push(Field {
            name,
            constant: c.clone(),
        });
    }
    out
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "continue", "crate", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod",
    "move", "mut", "priv", "pub", "ref", "return", "self", "static", "struct", "super", "trait",
    "true", "try", "type", "unsafe", "use", "where", "while", "yield", "_",
];

/// `SEA_HEIGHT` → `sea_height`, `seaHeight` → `sea_height`.
fn snake_case(name: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let after_lower =
            i > 0 && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit());
        if c.is_ascii_uppercase() && after_lower {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// A string as a Rust string literal (for titles in generated code).
fn rust_str(s: &str) -> String {
    format!("{s:?}")
}

fn cargo_toml(names: &Names, title: &str) -> String {
    let description = format!("{title}, exported from wesl-shadertoy as a Bevy 0.20 plugin.");
    format!(
        r#"[package]
name = "{dir}"
version = "0.1.0"
edition = "2024"
description = {description}
publish = false

[dependencies]
# The feature set wesl-shadertoy itself builds with: a fullscreen render pass
# needs the renderer and the core pipeline, not the 2D/3D scene stacks.
bevy = {{ version = "0.20", default-features = false, features = [
    "default_app",
    "default_platform",
    "bevy_render",
    "bevy_core_pipeline",
] }}

# A workspace of its own, so the crate builds wherever it is unpacked.
[workspace]
"#,
        dir = names.dir,
        description = rust_str(&description),
    )
}

fn lib_rs(names: &Names, title: &str, fields: &[Field]) -> String {
    let Names { ty, .. } = names;
    let uuid = uuid_for(&names.dir);
    let mut tweak_fields = String::new();
    let mut tweak_defaults = String::new();
    let mut tweak_packing = String::new();
    for (slot, f) in fields.iter().enumerate() {
        tweak_fields.push_str(&format!(
            "    /// {}\n    pub {}: {},\n",
            f.doc(),
            f.name,
            f.rust_type()
        ));
        tweak_defaults.push_str(&format!("            {}: {},\n", f.name, f.default_value()));
        tweak_packing.push_str(&format!("        slots[{slot}] = {};\n", f.packed()));
    }
    let slots_binding = if fields.is_empty() {
        "slots"
    } else {
        "mut slots"
    };
    format!(
        r#"//! {title}, exported from [wesl-shadertoy](https://github.com/barafael/wesl-shadertoy)
//! as a Bevy 0.20 plugin: a fullscreen shader with shadertoy's uniforms.
//!
//! Add [`{ty}Plugin`] and put a [`{ty}Shader`] on a 3D camera; the shader
//! then draws over that camera's whole view (as a post-process pass, before
//! tonemapping: use `Tonemapping::None` to see its colors untouched). Its
//! constants live in the [`{ty}Tweaks`] resource: change a field, by hand or
//! in an inspector (it derives `Reflect`), and the shader follows live.
//!
//! `src/shader.wgsl` is the linked WGSL; `src/shader.wesl` is the source it
//! was compiled from, for reference (it is wesl-shadertoy's dialect and not
//! built here).

use bevy::asset::{{load_internal_asset, uuid_handle}};
use bevy::core_pipeline::fullscreen_material::{{FullscreenMaterial, FullscreenMaterialPlugin}};
use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::render_resource::ShaderType;
use bevy::shader::ShaderRef;
use bevy::window::PrimaryWindow;

/// The linked WGSL, embedded in the binary.
const SHADER: Handle<Shader> = uuid_handle!("{uuid}");

/// How many constants the shader can read live (the length of `i_tweaks`).
const MAX_TWEAKS: usize = {MAX_TWEAKS};

/// Renders the shader on every camera with a [`{ty}Shader`].
pub struct {ty}Plugin;

impl Plugin for {ty}Plugin {{
    fn build(&self, app: &mut App) {{
        load_internal_asset!(app, SHADER, "shader.wgsl", Shader::from_wgsl);
        app.add_plugins(FullscreenMaterialPlugin::<{ty}Shader>::default())
            .register_type::<{ty}Tweaks>()
            .init_resource::<{ty}Tweaks>()
            .add_systems(Update, update_uniforms);
    }}
}}

/// The shader's live constants; edit them to change it while it runs.
#[derive(Resource, Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, Default)]
pub struct {ty}Tweaks {{
{tweak_fields}}}

impl Default for {ty}Tweaks {{
    /// The values the shader was exported with.
    fn default() -> Self {{
        Self {{
{tweak_defaults}        }}
    }}
}}

impl {ty}Tweaks {{
    /// The fields packed into the `i_tweaks` slots the shader reads.
    fn slots(&self) -> [Vec4; MAX_TWEAKS] {{
        let {slots_binding} = [Vec4::ZERO; MAX_TWEAKS];
{tweak_packing}        slots
    }}
}}

/// The shader's uniforms (shadertoy's `iTime`, `iResolution`, `iMouse`, …).
/// Put it on a 3D camera to draw the shader there; [`{ty}Plugin`] fills it
/// in every frame. The layout must match `ShaderToyUniforms` in
/// `shader.wgsl`.
#[derive(Component, ExtractComponent, Clone, Copy, ShaderType, Default, Debug)]
#[extract_app(RenderApp)]
pub struct {ty}Shader {{
    pub time: f32,
    pub time_delta: f32,
    pub frame: u32,
    pub _pad: f32,
    /// xy: viewport size in physical pixels, z: 1.0.
    pub resolution: Vec4,
    /// xy: pointer while the left button is held, zw: where it was
    /// pressed, negated once released. Origin bottom-left.
    pub mouse: Vec4,
    /// x: year, y: month (1-12), z: day of month, w: seconds since midnight.
    pub date: Vec4,
    pub sample_rate: f32,
    pub frame_rate: f32,
    pub _overlay_top: f32,
    pub _overlay_bottom: f32,
    /// Buffer channels are not exported; these stay zero.
    pub channel_time: Vec4,
    pub channel_resolution: [Vec4; 4],
    /// The [`{ty}Tweaks`] values, one slot per constant.
    pub tweaks: [Vec4; MAX_TWEAKS],
}}

impl FullscreenMaterial for {ty}Shader {{
    fn fragment_shader() -> ShaderRef {{
        SHADER.into()
    }}
}}

/// Advance the clock and copy the window, mouse and tweaks into every
/// [`{ty}Shader`].
fn update_uniforms(
    time: Res<Time>,
    tweaks: Res<{ty}Tweaks>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    mut shaders: Query<&mut {ty}Shader>,
    mut mouse: Local<Vec4>,
    mut dragging: Local<bool>,
) {{
    let size = window
        .as_ref()
        .map(|w| Vec2::new(w.physical_width() as f32, w.physical_height() as f32))
        .unwrap_or(Vec2::ONE);
    // Physical pixels with the origin bottom-left, like shadertoy's iMouse.
    let cursor = window
        .as_ref()
        .and_then(|w| w.physical_cursor_position())
        .map(|p| Vec2::new(p.x, size.y - p.y));
    let pressed = buttons.as_ref().is_some_and(|b| b.just_pressed(MouseButton::Left));
    let held = buttons.as_ref().is_some_and(|b| b.pressed(MouseButton::Left));
    if pressed && let Some(at) = cursor {{
        *mouse = Vec4::new(at.x, at.y, at.x, at.y);
        *dragging = true;
    }} else if *dragging {{
        if let Some(at) = cursor {{
            mouse.x = at.x;
            mouse.y = at.y;
        }}
        if !held {{
            *dragging = false;
            mouse.z = -mouse.z;
            mouse.w = -mouse.w;
        }}
    }}
    let dt = time.delta_secs();
    let slots = tweaks.slots();
    for mut shader in &mut shaders {{
        shader.mouse = *mouse;
        shader.time = time.elapsed_secs();
        shader.time_delta = dt;
        shader.frame = shader.frame.wrapping_add(1);
        shader.resolution = size.extend(1.0).extend(0.0);
        shader.date = date();
        shader.sample_rate = 44100.0;
        if dt > 0.0 {{
            shader.frame_rate = 1.0 / dt;
        }}
        shader.tweaks = slots;
    }}
}}

/// shadertoy's `iDate` from the system clock (UTC).
fn date() -> Vec4 {{
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let days = (secs / 86400.0).floor() as i64;
    // Howard Hinnant's `civil_from_days`.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 {{ mp + 3 }} else {{ mp - 9 }};
    let year = yoe + era * 400 + i64::from(month <= 2);
    let seconds = secs - days as f64 * 86400.0;
    Vec4::new(year as f32, month as f32, day as f32, seconds as f32)
}}
"#,
        title = title,
        ty = ty,
        uuid = uuid,
        MAX_TWEAKS = MAX_TWEAKS,
    )
}

fn demo_rs(names: &Names, title: &str) -> String {
    let Names { ty, krate, .. } = names;
    format!(
        r#"//! {title} in a window: `cargo run --example demo`.

use bevy::core_pipeline::tonemapping::{{DebandDither, Tonemapping}};
use bevy::prelude::*;
use {krate}::{{{ty}Plugin, {ty}Shader}};

fn main() {{
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {{
                primary_window: Some(Window {{
                    title: {title_str}.into(),
                    ..default()
                }}),
                ..default()
            }}),
            {ty}Plugin,
        ))
        .add_systems(Startup, setup)
        .run();
}}

fn setup(mut commands: Commands) {{
    commands.spawn((
        Camera3d::default(),
        // Show the shader's colors untouched, like shadertoy.
        Tonemapping::None,
        DebandDither::Disabled,
        Msaa::Off,
        {ty}Shader::default(),
    ));
}}
"#,
        title_str = rust_str(title),
    )
}

fn readme(names: &Names, title: &str, source: &str) -> String {
    let Names { ty, krate, dir } = names;
    let lygia = if source.contains("lygia::") {
        "\nIt links parts of [LYGIA](https://lygia.xyz), under the Prosperity \
         License 3.0.0 or the Patron License.\n"
    } else {
        ""
    };
    format!(
        r#"# {dir}

{title}, exported from [wesl-shadertoy](https://github.com/barafael/wesl-shadertoy)
as a Bevy 0.20 plugin.

```sh
cargo run --example demo
```

```rust
app.add_plugins({krate}::{ty}Plugin);
commands.spawn((Camera3d::default(), {krate}::{ty}Shader::default()));
```

The shader's constants are the `{ty}Tweaks` resource (`Reflect`, so it shows
up in inspectors); change a field and the shader follows live.

## License

See the header of `src/shader.wesl` for the shader's author and license.{lygia}
"#
    )
}

/// A stable UUID for the shader handle, from a hash of the crate name.
fn uuid_for(name: &str) -> String {
    // FNV-1a, twice with different seeds for 128 bits.
    let hash = |seed: u64| {
        name.bytes().fold(seed, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
        })
    };
    let (a, b) = (hash(0xcbf2_9ce4_8422_2325), hash(0x6c62_272e_07bb_0142));
    let hex = format!("{a:016x}{b:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

// --- delivery ---------------------------------------------------------------------

/// Export `text` and deliver it; returns the status-bar note.
pub fn export_now(text: &str, title: Option<&str>) -> (String, bool) {
    match export(text, title).and_then(|c| deliver(&c)) {
        Ok(note) => (note, true),
        Err(e) => (e, false),
    }
}

/// Native: write the crate to `./<name>/`.
#[cfg(not(target_arch = "wasm32"))]
fn deliver(krate: &Crate) -> Result<String, String> {
    let dir = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .join(&krate.name);
    write_to(krate, &dir)?;
    Ok(format!("exported the Bevy crate to {}", dir.display()))
}

/// Write the crate's files under `dir`, replacing earlier exports.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_to(krate: &Crate, dir: &std::path::Path) -> Result<(), String> {
    for (path, contents) in &krate.files {
        let path = dir.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Web: download the crate as `<name>.zip`.
#[cfg(target_arch = "wasm32")]
fn deliver(krate: &Crate) -> Result<String, String> {
    use wasm_bindgen::JsCast;

    let files: Vec<(String, &[u8])> = krate
        .files
        .iter()
        .map(|(path, contents)| (format!("{}/{path}", krate.name), contents.as_bytes()))
        .collect();
    let bytes = zip(&files);
    let fail = |_| "the browser refused the download".to_string();
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes.as_slice()));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/zip");
    let blob =
        web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).map_err(fail)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(fail)?;
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let anchor: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .map_err(fail)?
        .dyn_into()
        .map_err(|_| "no anchor element".to_string())?;
    anchor.set_href(&url);
    let file = format!("{}.zip", krate.name);
    anchor.set_download(&file);
    anchor.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(format!("downloaded the Bevy crate as {file}"))
}

/// A store-only (uncompressed) zip archive of `files`: small text files,
/// so compression would not buy much and this needs no dependency.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn zip(files: &[(String, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        // Version 2.0, UTF-8 names, stored, no timestamp (1980-01-01).
        let common = |out: &mut Vec<u8>| {
            for half in [20u16, 0x0800, 0, 0, 0x21] {
                out.extend_from_slice(&half.to_le_bytes());
            }
            for word in [crc, size, size] {
                out.extend_from_slice(&word.to_le_bytes());
            }
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra field
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        common(&mut central);
        // Comment length, disk, internal and external attributes.
        central.extend_from_slice(&[0; 2 + 2 + 2 + 4]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = out.len() as u32;
    let count = files.len() as u16;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // disk numbers
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment
    out
}

/// CRC-32 (IEEE), bit by bit: the files are small.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::examples;

    /// Every kind of constant, a name that is a Rust keyword and a camelCase one.
    const KINDS: &str = "\
// Kinds — every kind of tweakable constant.
import package::shadertoy::{i_time, i_mouse, i_date, i_resolution};
const SPEED: f32 = 1.5; // tweak: 0..4
const COUNT: i32 = -3;
const STEPS = 7u;
const OFFSET: vec2<f32> = vec2<f32>(0.25, -0.5);
const TINT: vec3<f32> = vec3<f32>(1.0, 0.5, 0.0);
const TYPE: vec4<f32> = vec4<f32>(0.1, 0.2, 0.3, 0.4);
const glowAmount: f32 = 2.0;
fn main_image(p: vec2<f32>) -> vec4<f32> {
    let uv = p / i_resolution().xy + OFFSET + i_mouse().xy * 0.0 + i_date().w * 0.0;
    let k = SPEED * f32(COUNT) * f32(STEPS) * glowAmount * i_time();
    return vec4<f32>(TINT * uv.x * k, 1.0) * TYPE;
}
";

    fn file<'a>(krate: &'a Crate, path: &str) -> &'a str {
        &krate.files.iter().find(|(p, _)| p == path).unwrap().1
    }

    #[test]
    fn names_follow_the_title() {
        let n = Names::new("Star Nest");
        assert_eq!(
            (n.dir.as_str(), n.krate.as_str(), n.ty.as_str()),
            ("star-nest-bevy", "star_nest_bevy", "StarNest")
        );
        let n = Names::new("2D noise / zoo!");
        assert_eq!(
            (n.dir.as_str(), n.ty.as_str()),
            ("shader-2d-noise-zoo-bevy", "Shader2dNoiseZoo")
        );
        assert_eq!(Names::new("\u{2014}").ty, "Shader");
        assert_eq!(source_title(KINDS).as_deref(), Some("Kinds"));
        assert_eq!(source_title("fn main_image"), None);
        assert_eq!(snake_case("SEA_HEIGHT"), "sea_height");
        assert_eq!(snake_case("glowAmount"), "glow_amount");
    }

    #[test]
    fn constants_become_resource_fields() {
        let krate = export(KINDS, None).expect("exports");
        assert_eq!(krate.name, "kinds-bevy");
        let lib = file(&krate, "src/lib.rs");
        for field in [
            "pub speed: f32,",
            "pub count: i32,",
            "pub steps: u32,",
            "pub offset: Vec2,",
            "pub tint: Vec3,",
            "pub type_: Vec4,",
            "pub glow_amount: f32,",
            "speed: 1.5,",
            "count: -3,",
            "offset: Vec2::new(0.25, -0.5),",
            "slots[1] = Vec4::new(self.count as f32, 0.0, 0.0, 0.0);",
            "Meant for 0.0..=4.0.",
            "pub struct KindsPlugin;",
        ] {
            assert!(lib.contains(field), "missing `{field}` in\n{lib}");
        }
        let wgsl = file(&krate, "src/shader.wgsl");
        assert_eq!(wgsl.matches("@fragment").count(), 1, "{wgsl}");
        let demo = file(&krate, "examples/demo.rs");
        assert!(demo.contains("use kinds_bevy::{KindsPlugin, KindsShader};"));
        assert!(file(&krate, "Cargo.toml").contains("name = \"kinds-bevy\""));
    }

    #[test]
    fn refuses_what_it_cannot_export() {
        let fluid = examples::by_name("Fluid").unwrap();
        let why = export(fluid.code, Some(fluid.name)).err().unwrap();
        assert!(why.contains("buffer"), "{why}");
        let broken = export("fn main_image(", None).err().unwrap();
        assert!(
            broken.starts_with("export needs a shader that compiles"),
            "{broken}"
        );
    }

    #[test]
    fn zip_is_well_formed() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let files = [
            ("a/x.txt".to_string(), &b"hello"[..]),
            ("a/y.txt".into(), b""),
        ];
        let zip = zip(&files);
        assert_eq!(&zip[..4], &0x0403_4b50u32.to_le_bytes());
        // The end record points back at the central directory.
        let end = &zip[zip.len() - 22..];
        assert_eq!(&end[..4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(u16::from_le_bytes([end[10], end[11]]), 2);
        let at = u32::from_le_bytes(end[16..20].try_into().unwrap()) as usize;
        assert_eq!(&zip[at..at + 4], &0x0201_4b50u32.to_le_bytes());
        let crc = u32::from_le_bytes(zip[14..18].try_into().unwrap());
        assert_eq!(crc, crc32(b"hello"));
    }

    /// The exported crates build against Bevy 0.20: Plasma, Seascape (int
    /// and color constants) and every kind of constant, warnings denied.
    /// Offline, with this crate's lockfile, in `target/export-check`; the
    /// first run checks Bevy and takes minutes, so it is opt-in:
    /// `cargo test -- --ignored exported_crates_build`.
    #[test]
    #[ignore]
    fn exported_crates_build() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let base = root.join("target/export-check");
        let plasma = examples::by_name("Plasma").unwrap();
        let seascape = examples::by_name("Seascape").unwrap();
        for (text, title) in [
            (plasma.code, Some(plasma.name)),
            (seascape.code, Some(seascape.name)),
            (KINDS, None),
        ] {
            let krate = export(text, title).expect("exports");
            let dir = base.join(&krate.name);
            write_to(&krate, &dir).expect("writes");
            std::fs::copy(root.join("Cargo.lock"), dir.join("Cargo.lock")).expect("lockfile");
            let status = std::process::Command::new(env!("CARGO"))
                .args(["check", "--offline", "--all-targets", "--quiet"])
                .current_dir(&dir)
                .env("CARGO_TARGET_DIR", base.join("target"))
                .env("RUSTFLAGS", "-Dwarnings")
                .status()
                .expect("runs cargo");
            assert!(status.success(), "{} does not build", krate.name);
        }
    }
}
