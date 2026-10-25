//! naga IR → dataflow graph.
//!
//! One function of the validated module becomes a [`Graph`]: its arguments
//! are sources, its `return`s are sinks, and in between sit either every
//! expression ([`Mode::All`]) or only the `let` bindings ([`Mode::Lets`]),
//! each labeled with the formula it computes, rebuilt from the IR with the
//! nodes it reads shown by name. Literals and constants are never nodes:
//! they appear inline in their consumer's formula.
//!
//! A `var` is one node: every store into it is an edge in, every read an
//! edge out, so a loop that updates it shows up as a cycle (the layout
//! draws those edges dashed). Control flow is not drawn: nodes computed
//! inside an `if`, `loop` or `switch` carry that nesting as their
//! [`Node::scope`], and conditions are left out.
//!
//! The composed module carries WESL-mangled names
//! (`package_user_map`, `lygia_generative_snoise_snoise2`); [`readable`]
//! maps them back to the item names the user wrote.

use std::collections::HashMap;

use naga::common::wgsl::{TryToWgsl, TypeContext};
use naga::{
    BinaryOperator, Expression, Function, Handle, Literal, LocalVariable, Module, Statement,
    TypeInner, UnaryOperator,
};
use wesl::mangler::{EscapeMangler, Mangler};

/// Formulas longer than this are cut off with an ellipsis (the hover text
/// shows the full one).
const MAX_TEXT: usize = 160;

/// Which expressions become nodes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// Arguments, `let`s, `var`s and returns; everything else is folded
    /// into their formulas.
    #[default]
    Lets,
    /// Every expression that is not a literal, a constant or a plain read.
    All,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A function argument.
    Argument,
    /// An environment accessor like `i_time()` ([`Mode::All`] only): the
    /// same for every pixel.
    Uniform,
    /// A named `let`.
    Let,
    /// A `var`: stores flow in, reads flow out.
    Var,
    /// An unnamed expression ([`Mode::All`] only).
    Op,
    /// A call of another function ([`Mode::All`], or a call whose result
    /// is unused).
    Call,
    Return,
}

/// One box of the graph.
#[derive(Clone, Debug)]
pub struct Node {
    pub kind: Kind,
    /// Name or operator: `uv`, `cos`, `return`.
    pub title: String,
    /// What it computes, e.g. `(frag_coord - 0.5 * res) / res.y`; empty
    /// when the title says it all.
    pub text: String,
    /// WGSL type, e.g. `vec2<f32>`.
    pub ty: String,
    /// Enclosing control flow, e.g. `loop › if`; empty at the top level.
    pub scope: String,
    /// The expression behind a [`Kind::Let`], for its thumbnail.
    pub expr: Option<Handle<Expression>>,
}

/// Data flows `from` → `to` (node indices).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// The item name behind a WESL-mangled name (`package_user_map` → `map`,
/// `lygia_math_const__2TWO_PI` → `TWO_PI`); other names as they are. Only
/// names from `package::` or the `lygia` package are unmangled: any
/// identifier with an underscore would otherwise "unmangle" too.
pub fn readable(name: &str) -> String {
    if (name.starts_with("package_") || name.starts_with("lygia_"))
        && let Some((_, item)) = EscapeMangler.unmangle(name)
    {
        return item;
    }
    name.to_string()
}

/// [`readable`] applied to every identifier in `text` (struct names
/// inside type strings).
fn readable_idents(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut ident = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_alphanumeric() || c == '_' {
            ident.push(c);
        } else {
            if !ident.is_empty() {
                out.push_str(&readable(&ident));
                ident.clear();
            }
            out.push(c);
        }
    }
    out.pop();
    out
}

/// The user's own functions in `module`, by readable name: `main_image`
/// first, then the buffer passes, then the rest alphabetically.
pub fn user_functions(module: &Module) -> Vec<(String, Handle<Function>)> {
    let mut out: Vec<(String, Handle<Function>)> = module
        .functions
        .iter()
        .filter_map(|(h, f)| {
            let name = f.name.as_deref()?;
            let (path, item) = EscapeMangler.unmangle(name)?;
            (name.starts_with("package_user_") && path.components == ["user"]).then_some((item, h))
        })
        .filter(|(name, _)| name != "st_load_tweaks")
        .collect();
    let rank = |name: &str| match name {
        "main_image" => 0,
        n if crate::compile::BUFFER_NAMES.contains(&n) => 1,
        _ => 2,
    };
    out.sort_by(|a, b| (rank(&a.0), &a.0).cmp(&(rank(&b.0), &b.0)));
    out
}

/// The graph of `function` (validated as part of `module`, with `info`).
pub fn extract(
    module: &Module,
    info: &naga::valid::ModuleInfo,
    function: Handle<Function>,
    mode: Mode,
) -> Graph {
    let mut x = Extractor {
        module,
        func: &module.functions[function],
        info: &info[function],
        mode,
        graph: Graph::default(),
        nodes: HashMap::new(),
        deps: HashMap::new(),
        vars: HashMap::new(),
        stores: HashMap::new(),
        scopes: HashMap::new(),
        calls: HashMap::new(),
    };
    // Arguments first, so they lead the layout's initial order.
    for i in 0..x.func.arguments.len() {
        if let Some((h, _)) = x
            .func
            .expressions
            .iter()
            .find(|(_, e)| matches!(e, Expression::FunctionArgument(a) if *a as usize == i))
        {
            x.node(h);
        }
    }
    x.block(&x.func.body, &[]);
    x.graph.edges.sort_by_key(|e| (e.from, e.to));
    x.graph.edges.dedup();
    prune_lone_vars(&mut x.graph);
    x.graph
}

/// Drop `var`s connected only to themselves: loop counters whose reads
/// are all in conditions, which the graph does not show.
fn prune_lone_vars(graph: &mut Graph) {
    let lone: Vec<bool> = (0..graph.nodes.len())
        .map(|n| {
            graph.nodes[n].kind == Kind::Var
                && graph.edges.iter().all(|e| (e.from == n) == (e.to == n))
        })
        .collect();
    let mut index = Vec::with_capacity(lone.len());
    let mut kept = 0;
    for &l in &lone {
        index.push(kept);
        kept += usize::from(!l);
    }
    let mut n = 0;
    graph.nodes.retain(|_| {
        n += 1;
        !lone[n - 1]
    });
    graph.edges.retain(|e| !lone[e.from]);
    for e in &mut graph.edges {
        e.from = index[e.from];
        e.to = index[e.to];
    }
}

struct Extractor<'a> {
    module: &'a Module,
    func: &'a Function,
    info: &'a naga::valid::FunctionInfo,
    mode: Mode,
    graph: Graph,
    /// Expression → its node, or `None` when it is folded into others.
    nodes: HashMap<Handle<Expression>, Option<usize>>,
    /// Expression → the nodes it reads (itself, if it is one).
    deps: HashMap<Handle<Expression>, Vec<usize>>,
    vars: HashMap<Handle<LocalVariable>, usize>,
    /// Var node → stores into it so far (its initializer counts as one).
    stores: HashMap<usize, usize>,
    /// Expression → enclosing control flow, from its `Emit`.
    scopes: HashMap<Handle<Expression>, String>,
    /// Call result → the call's arguments.
    calls: HashMap<Handle<Expression>, Vec<Handle<Expression>>>,
}

impl Extractor<'_> {
    fn block(&mut self, block: &naga::Block, scope: &[&str]) {
        fn nested<'s>(scope: &[&'s str], what: &'s str) -> Vec<&'s str> {
            [scope, &[what]].concat()
        }
        for statement in block.iter() {
            match statement {
                Statement::Emit(range) => {
                    for h in range.clone() {
                        self.scopes.insert(h, scope.join(" › "));
                    }
                }
                Statement::Block(inner) => self.block(inner, scope),
                Statement::If { accept, reject, .. } => {
                    self.block(accept, &nested(scope, "if"));
                    self.block(reject, &nested(scope, "else"));
                }
                Statement::Switch { cases, .. } => {
                    for case in cases {
                        self.block(&case.body, &nested(scope, "switch"));
                    }
                }
                Statement::Loop {
                    body, continuing, ..
                } => {
                    self.block(body, &nested(scope, "loop"));
                    self.block(continuing, &nested(scope, "loop"));
                }
                Statement::Store { pointer, value } => {
                    let Some(var) = self.var_of(*pointer) else {
                        continue;
                    };
                    let mut from = self.deps(*value);
                    from.extend(self.pointer_index_deps(*pointer));
                    for from in from {
                        self.edge(from, var);
                    }
                    // One store reads like an initializer; more, as a count.
                    let stores = self.stores.entry(var).or_default();
                    *stores += 1;
                    let text = match *stores {
                        1 => format!("= {}", self.text(*value, true)),
                        n => format!("{n} stores"),
                    };
                    self.graph.nodes[var].text = text;
                }
                Statement::Call {
                    function,
                    arguments,
                    result,
                } => {
                    match result {
                        Some(result) => {
                            self.calls.insert(*result, arguments.clone());
                            self.scopes.insert(*result, scope.join(" › "));
                        }
                        // Only side effects: a sink of its own.
                        None => {
                            // Inputs first: nodes are numbered in source order.
                            let deps: Vec<usize> =
                                arguments.iter().flat_map(|a| self.deps(*a)).collect();
                            let title = self.function_name(*function);
                            let node = self.add(Kind::Call, title, String::new(), "()", scope);
                            for from in deps {
                                self.edge(from, node);
                            }
                        }
                    }
                }
                Statement::Return { value: Some(value) } => {
                    let deps = self.deps(*value);
                    let text = self.text(*value, true);
                    let ty = self.ty(*value);
                    let node = self.add(Kind::Return, "return".into(), text, &ty, scope);
                    for from in deps {
                        self.edge(from, node);
                    }
                }
                _ => {}
            }
        }
    }

    fn add(&mut self, kind: Kind, title: String, text: String, ty: &str, scope: &[&str]) -> usize {
        self.graph.nodes.push(Node {
            kind,
            title,
            text,
            ty: ty.to_string(),
            scope: scope.join(" › "),
            expr: None,
        });
        self.graph.nodes.len() - 1
    }

    fn edge(&mut self, from: usize, to: usize) {
        self.graph.edges.push(Edge { from, to });
    }

    /// The node of `var`, created on first use.
    fn var_node(&mut self, var: Handle<LocalVariable>) -> usize {
        if let Some(&n) = self.vars.get(&var) {
            return n;
        }
        let local = &self.func.local_variables[var];
        let ty = self.type_name(local.ty);
        let name = local.name.clone().unwrap_or_else(|| "var".into());
        let deps = local.init.map(|init| self.deps(init)).unwrap_or_default();
        let n = self.add(Kind::Var, name, String::new(), &ty, &[]);
        self.vars.insert(var, n);
        if let Some(init) = local.init {
            self.graph.nodes[n].text = format!("= {}", self.text(init, true));
            self.stores.insert(n, 1);
        }
        for from in deps {
            self.edge(from, n);
        }
        n
    }

    /// The `var` a pointer expression points into (through any field and
    /// index accesses).
    fn var_of(&mut self, pointer: Handle<Expression>) -> Option<usize> {
        match self.func.expressions[pointer] {
            Expression::LocalVariable(var) => Some(self.var_node(var)),
            Expression::Access { base, .. } | Expression::AccessIndex { base, .. } => {
                self.var_of(base)
            }
            _ => None,
        }
    }

    /// The nodes the dynamic indices of a pointer chain read (`a[i]`).
    fn pointer_index_deps(&mut self, pointer: Handle<Expression>) -> Vec<usize> {
        match self.func.expressions[pointer] {
            Expression::Access { base, index } => {
                let mut deps = self.pointer_index_deps(base);
                deps.extend(self.deps(index));
                deps
            }
            Expression::AccessIndex { base, .. } => self.pointer_index_deps(base),
            _ => Vec::new(),
        }
    }

    /// Whether `h` gets a node of its own in this mode, and of which kind.
    fn kind_of(&self, h: Handle<Expression>) -> Option<Kind> {
        let e = &self.func.expressions[h];
        if matches!(e, Expression::FunctionArgument(_)) {
            return Some(Kind::Argument);
        }
        if self.func.named_expressions.contains_key(&h) {
            return Some(Kind::Let);
        }
        if self.mode == Mode::Lets || self.is_constant(h) {
            return None;
        }
        match e {
            Expression::LocalVariable(_)
            | Expression::GlobalVariable(_)
            | Expression::Load { .. } => None,
            Expression::Splat { .. } if self.is_implicit_splat(h) => None,
            // Pointer arithmetic is part of the read it feeds.
            Expression::Access { .. } | Expression::AccessIndex { .. } if self.is_pointer(h) => {
                None
            }
            Expression::CallResult(f) if self.is_environment(*f) => Some(Kind::Uniform),
            Expression::CallResult(_) => Some(Kind::Call),
            _ => Some(Kind::Op),
        }
    }

    /// The node of `h`, created (with its incoming edges) on first use.
    fn node(&mut self, h: Handle<Expression>) -> Option<usize> {
        if let Some(&n) = self.nodes.get(&h) {
            return n;
        }
        let Some(kind) = self.kind_of(h) else {
            self.nodes.insert(h, None);
            return None;
        };
        // Inputs first: nodes are numbered in source order.
        let deps: Vec<usize> = self
            .operands(h)
            .into_iter()
            .flat_map(|operand| self.deps(operand))
            .collect();
        let (title, text) = match kind {
            Kind::Argument | Kind::Uniform => (self.text(h, true), String::new()),
            Kind::Let => (self.func.named_expressions[&h].clone(), self.text(h, true)),
            _ => (self.op_name(h), self.text(h, true)),
        };
        let ty = self.ty(h);
        let scope = self.scopes.get(&h).cloned().unwrap_or_default();
        self.graph.nodes.push(Node {
            kind,
            title,
            text,
            ty,
            scope,
            expr: (kind == Kind::Let).then_some(h),
        });
        let n = self.graph.nodes.len() - 1;
        self.nodes.insert(h, Some(n));
        for from in deps {
            self.edge(from, n);
        }
        Some(n)
    }

    /// The nodes `h` reads: itself if it is a node, else whatever its
    /// operands read.
    fn deps(&mut self, h: Handle<Expression>) -> Vec<usize> {
        if let Some(deps) = self.deps.get(&h) {
            return deps.clone();
        }
        let deps = if let Some(n) = self.node(h) {
            vec![n]
        } else if let Expression::LocalVariable(var) = self.func.expressions[h] {
            vec![self.var_node(var)]
        } else {
            let mut deps = Vec::new();
            for operand in self.operands(h) {
                for d in self.deps(operand) {
                    if !deps.contains(&d) {
                        deps.push(d);
                    }
                }
            }
            deps
        };
        self.deps.insert(h, deps.clone());
        deps
    }

    /// The expressions `h` reads directly.
    fn operands(&self, h: Handle<Expression>) -> Vec<Handle<Expression>> {
        use Expression as E;
        match &self.func.expressions[h] {
            E::Compose { components, .. } => components.clone(),
            E::Access { base, index } => vec![*base, *index],
            E::AccessIndex { base, .. } => vec![*base],
            E::Splat { value, .. } => vec![*value],
            E::Swizzle { vector, .. } => vec![*vector],
            E::Load { pointer } => vec![*pointer],
            E::ImageSample {
                image,
                sampler,
                coordinate,
                level,
                depth_ref,
                ..
            } => {
                let mut v = vec![*image, *sampler, *coordinate];
                v.extend(*depth_ref);
                match level {
                    naga::SampleLevel::Exact(l) | naga::SampleLevel::Bias(l) => v.push(*l),
                    naga::SampleLevel::Gradient { x, y } => v.extend([*x, *y]),
                    _ => {}
                }
                v
            }
            E::ImageLoad {
                image,
                coordinate,
                level,
                sample,
                array_index,
            } => [
                Some(*image),
                Some(*coordinate),
                *level,
                *sample,
                *array_index,
            ]
            .into_iter()
            .flatten()
            .collect(),
            E::ImageQuery { image, .. } => vec![*image],
            E::Unary { expr, .. } => vec![*expr],
            E::Binary { left, right, .. } => vec![*left, *right],
            E::Select {
                condition,
                accept,
                reject,
            } => vec![*condition, *accept, *reject],
            E::Derivative { expr, .. } => vec![*expr],
            E::Relational { argument, .. } => vec![*argument],
            E::Math {
                arg,
                arg1,
                arg2,
                arg3,
                ..
            } => [Some(*arg), *arg1, *arg2, *arg3]
                .into_iter()
                .flatten()
                .collect(),
            E::As { expr, .. } => vec![*expr],
            E::CallResult(_) => self.calls.get(&h).cloned().unwrap_or_default(),
            E::ArrayLength(e) => vec![*e],
            _ => Vec::new(),
        }
    }

    /// Literals, constants, and vectors built from them.
    fn is_constant(&self, h: Handle<Expression>) -> bool {
        match &self.func.expressions[h] {
            Expression::Literal(_)
            | Expression::Constant(_)
            | Expression::Override(_)
            | Expression::ZeroValue(_) => true,
            Expression::Compose { components, .. } => {
                components.iter().all(|&c| self.is_constant(c))
            }
            Expression::Splat { value, .. } => self.is_constant(*value),
            _ => false,
        }
    }

    /// A splat naga inserted for a vector-scalar operation (`v / s`): it
    /// shares its operand's span, unlike a written `vec3<f32>(s)`.
    fn is_implicit_splat(&self, h: Handle<Expression>) -> bool {
        let spans = &self.func.expressions;
        match self.func.expressions[h] {
            Expression::Splat { value, .. } => spans.get_span(h) == spans.get_span(value),
            _ => false,
        }
    }

    fn is_pointer(&self, h: Handle<Expression>) -> bool {
        matches!(
            self.info[h].ty.inner_with(&self.module.types),
            TypeInner::Pointer { .. } | TypeInner::ValuePointer { .. }
        )
    }

    /// A `package::shadertoy` accessor (`i_time()` and friends).
    fn is_environment(&self, f: Handle<Function>) -> bool {
        self.module.functions[f]
            .name
            .as_deref()
            .is_some_and(|n| n.starts_with("package_shadertoy_"))
    }

    fn function_name(&self, f: Handle<Function>) -> String {
        readable(self.module.functions[f].name.as_deref().unwrap_or("fn"))
    }

    fn type_name(&self, ty: Handle<naga::Type>) -> String {
        readable_idents(&self.module.to_ctx().type_to_string(ty))
    }

    /// The type of `h`, as WGSL.
    fn ty(&self, h: Handle<Expression>) -> String {
        readable_idents(
            &self
                .module
                .to_ctx()
                .type_resolution_to_string(&self.info[h].ty),
        )
    }

    /// Short name of an unnamed node: its operator or callee.
    fn op_name(&self, h: Handle<Expression>) -> String {
        use Expression as E;
        match &self.func.expressions[h] {
            E::Binary { op, .. } => binary_op(*op).0.to_string(),
            E::Unary { op, .. } => unary_op(*op).to_string(),
            E::Math { fun, .. } => fun.try_to_wgsl().unwrap_or("math").to_string(),
            E::CallResult(f) => self.function_name(*f),
            E::Compose { .. } => "construct".into(),
            E::Splat { .. } => "splat".into(),
            E::Swizzle { .. } => "swizzle".into(),
            E::Access { .. } | E::AccessIndex { .. } => "index".into(),
            E::As { .. } => "convert".into(),
            E::Select { .. } => "select".into(),
            E::ImageSample { .. } => "textureSample".into(),
            E::ImageLoad { .. } => "textureLoad".into(),
            E::ImageQuery { .. } => "textureQuery".into(),
            E::Derivative { .. } => "derivative".into(),
            E::Relational { .. } => "relational".into(),
            _ => "expr".into(),
        }
    }

    /// `h` as source text. Nodes other than `h` itself (and `var` reads)
    /// appear by name, unnamed nodes as `·`.
    fn text(&self, h: Handle<Expression>, root: bool) -> String {
        let mut text = self.text_prec(h, root).0;
        if text.chars().count() > MAX_TEXT {
            text = text.chars().take(MAX_TEXT - 1).collect::<String>() + "…";
        }
        text
    }

    /// [`Self::text`] plus the binding strength of its outermost operator
    /// (for parentheses; atoms bind tightest).
    fn text_prec(&self, h: Handle<Expression>, root: bool) -> (String, u8) {
        use Expression as E;
        const ATOM: u8 = 10;
        if !root && let Some(kind) = self.kind_of(h) {
            let name = match kind {
                Kind::Let => self.func.named_expressions[&h].clone(),
                Kind::Argument | Kind::Uniform => self.text_prec(h, true).0,
                _ => "·".into(),
            };
            return (name, ATOM);
        }
        let t = |e: Handle<Expression>| self.text_prec(e, false).0;
        let list =
            |es: &[Handle<Expression>]| es.iter().map(|&e| t(e)).collect::<Vec<_>>().join(", ");
        let text = match &self.func.expressions[h] {
            E::Literal(l) => literal(l),
            E::Constant(c) => match &self.module.constants[*c].name {
                Some(name) => readable(name),
                None => "const".into(),
            },
            E::Override(o) => readable(
                self.module.overrides[*o]
                    .name
                    .as_deref()
                    .unwrap_or("override"),
            ),
            E::ZeroValue(ty) => format!("{}()", self.type_name(*ty)),
            E::Compose { ty, components } => {
                format!("{}({})", self.type_name(*ty), list(components))
            }
            E::Splat { value, .. } if self.is_implicit_splat(h) => {
                return self.text_prec(*value, false);
            }
            E::Splat { size, value } => format!("vec{}({})", *size as u8, t(*value)),
            E::Swizzle {
                size,
                vector,
                pattern,
            } => {
                let swizzle: String = pattern[..*size as usize]
                    .iter()
                    .map(|c| ['x', 'y', 'z', 'w'][*c as usize])
                    .collect();
                format!("{}.{swizzle}", self.postfix_base(*vector))
            }
            E::AccessIndex { base, index } => {
                format!("{}{}", self.postfix_base(*base), self.member(*base, *index))
            }
            E::Access { base, index } => format!("{}[{}]", self.postfix_base(*base), t(*index)),
            E::FunctionArgument(i) => self.func.arguments[*i as usize]
                .name
                .clone()
                .unwrap_or_else(|| format!("arg{i}")),
            E::GlobalVariable(g) => readable(
                self.module.global_variables[*g]
                    .name
                    .as_deref()
                    .unwrap_or("global"),
            ),
            E::LocalVariable(v) => self.func.local_variables[*v]
                .name
                .clone()
                .unwrap_or_else(|| "var".into()),
            E::Load { pointer } => return self.text_prec(*pointer, false),
            E::Unary { op, expr } => {
                let (inner, prec) = self.text_prec(*expr, false);
                let inner = if prec < ATOM {
                    format!("({inner})")
                } else {
                    inner
                };
                return (format!("{}{inner}", unary_op(*op)), ATOM - 1);
            }
            E::Binary { op, left, right } => {
                let (symbol, prec) = binary_op(*op);
                let (l, lp) = self.text_prec(*left, false);
                let (r, rp) = self.text_prec(*right, false);
                let l = if lp < prec { format!("({l})") } else { l };
                // Left-associative: an equal-strength right operand needs
                // parentheses (`a - (b - c)`).
                let r = if rp <= prec { format!("({r})") } else { r };
                return (format!("{l} {symbol} {r}"), prec);
            }
            E::Select {
                condition,
                accept,
                reject,
            } => format!("select({}, {}, {})", t(*reject), t(*accept), t(*condition)),
            E::Math {
                fun,
                arg,
                arg1,
                arg2,
                arg3,
            } => {
                let args: Vec<_> = [Some(*arg), *arg1, *arg2, *arg3]
                    .into_iter()
                    .flatten()
                    .collect();
                format!("{}({})", fun.try_to_wgsl().unwrap_or("math"), list(&args))
            }
            E::As {
                expr,
                kind,
                convert,
            } => {
                let scalar = naga::Scalar {
                    kind: *kind,
                    width: convert.unwrap_or(4),
                };
                let ty = scalar.try_to_wgsl().unwrap_or("scalar");
                match self.info[h].ty.inner_with(&self.module.types) {
                    TypeInner::Vector { size, .. } => {
                        format!("vec{}<{ty}>({})", *size as u8, t(*expr))
                    }
                    _ if convert.is_none() => format!("bitcast<{ty}>({})", t(*expr)),
                    _ => format!("{ty}({})", t(*expr)),
                }
            }
            E::CallResult(f) => {
                let args = self
                    .calls
                    .get(&h)
                    .map(|args| list(args))
                    .unwrap_or_default();
                format!("{}({args})", self.function_name(*f))
            }
            E::ImageSample {
                image, coordinate, ..
            } => format!("textureSample({}, {})", t(*image), t(*coordinate)),
            E::ImageLoad {
                image, coordinate, ..
            } => format!("textureLoad({}, {})", t(*image), t(*coordinate)),
            E::ImageQuery { image, .. } => format!("textureDimensions({})", t(*image)),
            E::Derivative { expr, .. } => format!("dpdx/dpdy({})", t(*expr)),
            E::Relational { fun, argument } => {
                let name = match fun {
                    naga::RelationalFunction::All => "all",
                    naga::RelationalFunction::Any => "any",
                    naga::RelationalFunction::IsNan => "isNan",
                    naga::RelationalFunction::IsInf => "isInf",
                };
                format!("{name}({})", t(*argument))
            }
            E::ArrayLength(e) => format!("arrayLength({})", t(*e)),
            _ => "…".into(),
        };
        (text, ATOM)
    }

    /// The base of a `.x` / `[i]` access, parenthesized unless it is an atom.
    fn postfix_base(&self, base: Handle<Expression>) -> String {
        let (text, prec) = self.text_prec(base, false);
        if prec < 9 { format!("({text})") } else { text }
    }

    /// `.x`, `.field` or `[i]` for constant index `index` into `base`.
    fn member(&self, base: Handle<Expression>, index: u32) -> String {
        let types = &self.module.types;
        let mut inner = self.info[base].ty.inner_with(types);
        if let TypeInner::Pointer { base, .. } = inner {
            inner = &types[*base].inner;
        }
        match inner {
            TypeInner::Vector { .. } | TypeInner::ValuePointer { size: Some(_), .. } => {
                format!(".{}", ['x', 'y', 'z', 'w'][index as usize & 3])
            }
            TypeInner::Struct { members, .. } => match &members[index as usize].name {
                Some(name) => format!(".{name}"),
                None => format!(".{index}"),
            },
            _ => format!("[{index}]"),
        }
    }
}

/// WGSL text of a literal: floats always with a decimal point.
fn literal(l: &Literal) -> String {
    // Each float at its own precision: `0.4f32` must not print as the
    // f64 `0.4000000059604645`.
    let float = |s: String| {
        if s.contains(['.', 'e', 'i', 'N']) {
            s
        } else {
            format!("{s}.0")
        }
    };
    match *l {
        Literal::F64(v) | Literal::AbstractFloat(v) => float(v.to_string()),
        Literal::F32(v) => float(v.to_string()),
        Literal::F16(v) => float(v.to_string()),
        Literal::U32(v) => format!("{v}u"),
        Literal::I32(v) => v.to_string(),
        Literal::U64(v) => format!("{v}lu"),
        Literal::I64(v) | Literal::AbstractInt(v) => v.to_string(),
        Literal::U16(v) => v.to_string(),
        Literal::I16(v) => v.to_string(),
        Literal::Bool(v) => v.to_string(),
    }
}

/// Symbol and binding strength (higher binds tighter) of a binary operator.
fn binary_op(op: BinaryOperator) -> (&'static str, u8) {
    use BinaryOperator as B;
    match op {
        B::Multiply => ("*", 8),
        B::Divide => ("/", 8),
        B::Modulo => ("%", 8),
        B::Add => ("+", 7),
        B::Subtract => ("-", 7),
        B::ShiftLeft => ("<<", 6),
        B::ShiftRight => (">>", 6),
        B::Less => ("<", 5),
        B::LessEqual => ("<=", 5),
        B::Greater => (">", 5),
        B::GreaterEqual => (">=", 5),
        B::Equal => ("==", 5),
        B::NotEqual => ("!=", 5),
        B::And => ("&", 4),
        B::ExclusiveOr => ("^", 4),
        B::InclusiveOr => ("|", 4),
        B::LogicalAnd => ("&&", 3),
        B::LogicalOr => ("||", 2),
    }
}

fn unary_op(op: UnaryOperator) -> &'static str {
    match op {
        UnaryOperator::Negate => "-",
        UnaryOperator::LogicalNot => "!",
        UnaryOperator::BitwiseNot => "~",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The composed module of `src` (as the app compiles it), validated.
    pub(crate) fn compiled(src: &str) -> (Module, naga::valid::ModuleInfo) {
        let wgsl = crate::compile::compile_user_shader(src)
            .expect("compiles")
            .wgsl;
        super::super::parse(&wgsl).expect("parses")
    }

    fn main_image(module: &Module) -> Handle<Function> {
        user_functions(module)
            .into_iter()
            .find(|(n, _)| n == "main_image")
            .expect("main_image")
            .1
    }

    const PLASMA: &str = r#"
import package::shadertoy::i_resolution;
import package::shadertoy::i_time;

fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    let res = i_resolution().xy;
    let uv = (frag_coord - 0.5 * res) / res.y;
    let t = i_time();
    let col = 0.5 + 0.5 * cos(t + uv.xyx + vec3<f32>(0.0, 2.0, 4.0));
    return vec4<f32>(col, 1.0);
}
"#;

    #[test]
    fn named_lets_of_a_tiny_shader() {
        let (module, info) = compiled(PLASMA);
        let g = extract(&module, &info, main_image(&module), Mode::Lets);
        let titles: Vec<&str> = g.nodes.iter().map(|n| n.title.as_str()).collect();
        // Dependencies before their readers.
        assert_eq!(titles, ["frag_coord", "t", "res", "uv", "col", "return"]);
        let at = |t: &str| titles.iter().position(|n| *n == t).unwrap();
        let edge = |a, b| {
            g.edges.contains(&Edge {
                from: at(a),
                to: at(b),
            })
        };
        assert!(edge("frag_coord", "uv") && edge("res", "uv"));
        assert!(edge("t", "col") && edge("uv", "col") && edge("col", "return"));
        assert_eq!(g.edges.len(), 5, "{:?}", g.edges);
        let uv = &g.nodes[at("uv")];
        assert_eq!(uv.text, "(frag_coord - 0.5 * res) / res.y");
        assert_eq!(uv.ty, "vec2<f32>");
        assert_eq!(g.nodes[at("res")].text, "i_resolution().xy");
        assert_eq!(
            g.nodes[at("col")].text,
            "0.5 + 0.5 * cos(t + uv.xyx + vec3<f32>(0.0, 2.0, 4.0))"
        );
        assert!(g.nodes.iter().all(|n| n.scope.is_empty()));
    }

    #[test]
    fn all_expressions_of_a_tiny_shader() {
        let (module, info) = compiled(PLASMA);
        let g = extract(&module, &info, main_image(&module), Mode::All);
        let lets = extract(&module, &info, main_image(&module), Mode::Lets);
        assert!(g.nodes.len() > lets.nodes.len() + 5, "{:#?}", g.nodes);
        // Environment reads are sources, and nothing is a literal.
        assert!(
            g.nodes
                .iter()
                .any(|n| n.kind == Kind::Uniform && n.title == "i_resolution()")
        );
        assert!(
            g.nodes
                .iter()
                .any(|n| n.kind == Kind::Op && n.title == "cos")
        );
        assert!(!g.nodes.iter().any(|n| n.title == "0.5"));
        for e in &g.edges {
            assert!(e.from < g.nodes.len() && e.to < g.nodes.len());
        }
    }

    #[test]
    fn vars_and_loops() {
        let src = r#"
fn main_image(frag_coord: vec2<f32>) -> vec4<f32> {
    var acc = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        let w = f32(i) * frag_coord.x;
        acc = acc + w;
    }
    return vec4<f32>(acc);
}
"#;
        let (module, info) = compiled(src);
        let g = extract(&module, &info, main_image(&module), Mode::Lets);
        let at = |t: &str| g.nodes.iter().position(|n| n.title == t).unwrap();
        assert_eq!(g.nodes[at("w")].scope, "loop");
        assert_eq!(g.nodes[at("acc")].kind, Kind::Var);
        assert_eq!(g.nodes[at("acc")].text, "2 stores");
        let edge = |a, b| {
            g.edges.contains(&Edge {
                from: at(a),
                to: at(b),
            })
        };
        // The update is a cycle through the var.
        assert!(edge("acc", "acc") && edge("w", "acc") && edge("i", "w"));
        assert!(edge("acc", "return"));
    }

    #[test]
    fn mangled_names_read_back() {
        assert_eq!(readable("package_user_map"), "map");
        assert_eq!(readable("frag_coord"), "frag_coord");
        assert_eq!(literal(&Literal::F32(0.4)), "0.4");
        assert_eq!(literal(&Literal::F32(2.0)), "2.0");
        assert_eq!(
            readable_idents("array<package_user_Hit, 4>"),
            "array<Hit, 4>"
        );
    }

    /// Every example's functions extract in both modes.
    #[test]
    fn every_example_extracts() {
        for example in crate::examples::ALL {
            let (module, info) = compiled(example.code);
            let functions = user_functions(&module);
            assert_eq!(functions[0].0, "main_image", "{}", example.name);
            for (_, f) in functions {
                for mode in [Mode::Lets, Mode::All] {
                    let g = extract(&module, &info, f, mode);
                    assert!(
                        g.nodes.iter().any(|n| n.kind == Kind::Return),
                        "{}: no return",
                        example.name
                    );
                }
            }
        }
    }
}
