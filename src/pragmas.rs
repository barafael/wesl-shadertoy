//! WESL feature flags and `constants::` values, kept in the shader itself
//! as comment lines, so a saved, copied or shared shader compiles the same
//! way anywhere:
//!
//! ```wgsl
//! // wesl-flags: CENTER_2D, MIXOKLAB_SRGB
//! // wesl-const: MARBLE_PERIOD = 90.0
//! ```
//!
//! [`Pragmas`] reads them for the compile ([`crate::compile`]): the flags
//! become WESL features, the constants the source of the `constants`
//! module. A constant also turns on its same-named flag, the pairing LYGIA
//! relies on (`@if(MARBLE_PERIOD)` guards the `import
//! constants::MARBLE_PERIOD;`). The app's Flags window edits them.

/// Starts the comment line that lists the enabled flags.
const FLAGS_PRAGMA: &str = "// wesl-flags:";
/// Starts a comment line that sets one `constants::` value.
const CONST_PRAGMA: &str = "// wesl-const:";

/// The flags and constants a document sets in its pragma lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pragmas {
    /// Enabled flags, from `// wesl-flags:` lines.
    pub flags: Vec<String>,
    /// `(name, WGSL expression)`, one per `// wesl-const:` line.
    pub constants: Vec<(String, String)>,
}

impl Pragmas {
    /// Read the pragma lines of `text` (anywhere in it; later lines add to
    /// earlier ones, a constant set twice keeps the last value).
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if let Some(list) = line.strip_prefix(FLAGS_PRAGMA) {
                for flag in list.split(',').map(str::trim).filter(|f| is_name(f)) {
                    if !out.flags.iter().any(|f| f == flag) {
                        out.flags.push(flag.to_string());
                    }
                }
            } else if let Some(def) = line.strip_prefix(CONST_PRAGMA)
                && let Some((name, value)) = def.split_once('=')
                && is_name(name.trim())
                && !value.trim().is_empty()
            {
                out.set_constant(name.trim(), value.trim());
            }
        }
        out
    }

    /// Whether `name` is on: listed as a flag, or set as a constant.
    pub fn enabled(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name) || self.constant(name).is_some()
    }

    /// The value constant `name` is set to.
    pub fn constant(&self, name: &str) -> Option<&str> {
        self.constants
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_constant(&mut self, name: &str, value: &str) {
        match self.constants.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = value.to_string(),
            None => self.constants.push((name.to_string(), value.to_string())),
        }
    }

    pub fn set_flag(&mut self, name: &str, on: bool) {
        self.flags.retain(|f| f != name);
        if on {
            self.flags.push(name.to_string());
        } else {
            self.constants.retain(|(n, _)| n != name);
        }
    }

    /// The WESL features to compile with: every flag and constant on,
    /// everything else off (WESL's default).
    pub fn features(&self) -> wesl::Features {
        let mut features = wesl::Features::default();
        for name in self
            .flags
            .iter()
            .chain(self.constants.iter().map(|(n, _)| n))
        {
            features.set(name, true);
        }
        features
    }

    /// The source of the `constants` virtual module.
    pub fn constants_module(&self) -> String {
        self.constants
            .iter()
            .map(|(name, value)| format!("const {name} = {value};\n"))
            .collect()
    }

    /// Rewrite the pragma lines of `text` to say this: they replace the
    /// first pragma line (the others go), or, in a text without any, go
    /// after the comments it starts with.
    pub fn write(&self, text: &mut String) {
        let mut lines = Vec::new();
        if !self.flags.is_empty() {
            lines.push(format!("{FLAGS_PRAGMA} {}\n", self.flags.join(", ")));
        }
        for (name, value) in &self.constants {
            lines.push(format!("{CONST_PRAGMA} {name} = {value}\n"));
        }

        let mut kept: Vec<&str> = Vec::new();
        let mut at = None;
        let mut header = 0;
        for line in text.split_inclusive('\n') {
            let t = line.trim();
            if t.starts_with(FLAGS_PRAGMA) || t.starts_with(CONST_PRAGMA) {
                at.get_or_insert(kept.len());
                continue;
            }
            if header == kept.len() && t.starts_with("//") {
                header += 1;
            }
            kept.push(line);
        }
        let at = at.unwrap_or(header);
        let mut out = String::with_capacity(text.len());
        for (i, line) in kept.iter().enumerate() {
            if i == at {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                lines.iter().for_each(|l| out.push_str(l));
            }
            out.push_str(line);
        }
        if at >= kept.len() {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            lines.iter().for_each(|l| out.push_str(l));
        }
        *text = out;
    }
}

fn is_name(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}
