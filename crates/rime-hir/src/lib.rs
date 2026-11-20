//! HIR: the parser-agnostic intermediate representation.
//!
//! This crate is the firewall between the parser and every analysis. It has
//! no dependencies; nothing in it names a parser type. `rime-syntax` lowers
//! the full-moon CST into these structures, and `rime-check` consumes only
//! them — so swapping the parser (e.g. to a `Luau.Ast` C++ shim if grammar
//! fidelity ever forces it) means rewriting one lowering, not the checker.
//!
//! Shape notes:
//!   - Bodies live in a flat arena on `Module`, referenced by `BodyId`.
//!     Nested closures are separate bodies; the checker walks each
//!     independently (outer-local capture is unmodelled until plan §7.1).
//!   - Statements carry their source span so flow diagnostics (unreachable
//!     code) can point somewhere without consulting the parser.
//!   - Types are lowered to the small `Ty` summary the current analyses
//!     need. This grows toward the plan's interned types
//!     when the type checker lands; it does not try to anticipate it.
//!
//! Milestone deliberate omissions (documented, not decided): expression
//! lowering keeps what flow analysis reads — names, calls, projections,
//! nested bodies — plus the condition structure narrowing is made of
//! (binary operators, `not`, table constructor fields). Everything else
//! (if-expressions, interpolated strings) still flattens into `Composite`.

pub type Span = std::ops::Range<usize>;
pub type BodyId = usize;

pub mod visit;

#[derive(Debug)]
pub struct Module {
    pub bodies: Vec<Body>,
    pub root: BodyId,
    /// Names referenced anywhere in TYPE position (annotations, `type X = …`
    /// aliases, `::` assertions, generic instantiations). Lowering drops
    /// type structure; this name-set is what survives, so unused-* can count
    /// a type reference as a read. Over-approximate by construction:
    /// name-based, unscoped — same contract as capture's closure reads.
    pub type_refs: std::collections::HashSet<String>,
    /// Start offsets of the `require` names that reach a local the file
    /// declares (`local require = ...`, a parameter, a `local function`),
    /// not the builtin. Lowering leaves this empty: it needs name
    /// resolution, which the checker runs once after parsing.
    pub rebound_require: std::collections::HashSet<usize>,
    /// Places where the WRITTEN type says a value's members cannot be
    /// listed: a table type with a string indexer (`{ [string]: T }`),
    /// directly or behind type aliases of this file. `any` is not one.
    /// Lowering drops type structure; this is the one fact about it the
    /// member rules need. Each entry is a start offset:
    ///
    ///   - of a declared name, for `local x: T = ...` and for
    ///     `local x = e :: T` (the cast gives the name its type);
    ///   - of a `return` statement, for `return e :: T`.
    ///
    /// See `declares_unknown_members` and `returns_unknown_members`.
    pub unknown_members: std::collections::HashSet<usize>,
}

impl Module {
    /// Whether the local declared at `decl` was given a type that says its
    /// members cannot be listed (see `unknown_members`).
    pub fn declares_unknown_members(&self, decl: &LocalDecl) -> bool {
        self.unknown_members.contains(&decl.span.start)
    }

    /// Whether the `return` statement spanning `span` casts what it
    /// returns to such a type.
    pub fn returns_unknown_members(&self, span: &Span) -> bool {
        self.unknown_members.contains(&span.start)
    }

    pub fn body(&self, id: BodyId) -> &Body {
        &self.bodies[id]
    }

    /// Whether `e` is the name `require` and reaches the builtin. Every
    /// place that recognises a `require(...)` call asks this of the callee,
    /// so a file that rebinds the name is read the way Luau scopes it.
    pub fn is_require(&self, e: &Expr) -> bool {
        matches!(e, Expr::Name { name, span }
            if name == "require" && !self.rebound_require.contains(&span.start))
    }

    /// Whether `e` is the name `require` bound to something else in this
    /// file: a call through it is the file's own function, not a require.
    pub fn is_rebound_require(&self, e: &Expr) -> bool {
        matches!(e, Expr::Name { name, span }
            if name == "require" && self.rebound_require.contains(&span.start))
    }
}

/// One function body (or the module top level, which has no params and no
/// return annotation).
#[derive(Debug)]
pub struct Body {
    pub params: Vec<LocalDecl>,
    /// Whether the parameter list ends in `...` — a variadic callee accepts
    /// any number of extra arguments (milestone 2 arity checks).
    pub has_varargs: bool,
    pub return_ty: Option<Ty>,
    /// Span of the body's `end` token — where a definite-return diagnostic
    /// points. None for the module top level.
    pub end_span: Option<Span>,
    pub block: Block,
    /// The attributes written before `function` (`@native function f()`),
    /// in source order. Empty for the module top level.
    pub attributes: Vec<Attribute>,
    /// What this body's own source says that `block` does not keep. Nested
    /// function bodies carry their own.
    pub notes: SyntaxNotes,
}

/// One function attribute, `@native` in `@native function f() end`.
#[derive(Debug)]
pub struct Attribute {
    /// The name without the `@`.
    pub name: String,
    /// From the `@` to the end of the name.
    pub span: Span,
}

/// Facts about one body's source that the tree drops — lowering records
/// them here, so no rule has to re-scan source text for something the
/// parser already knew.
#[derive(Debug, Default)]
pub struct SyntaxNotes {
    /// Every `...` read as a value in this body (`return ...`,
    /// `select("#", ...)`, `{ ... }`), in source order. The expression
    /// itself lowers to `Expr::Leaf`: flow analysis has nothing to read in
    /// it. Luau refuses to compile one in a body whose `has_varargs` is
    /// false; whether that is so is the checker's call, not lowering's.
    pub vararg_reads: Vec<Span>,
    /// Every `if`, `elseif`, `while` and `until` condition that is one
    /// parenthesised expression (`if (x) then`), in source order. The tree
    /// keeps no trace of parentheses. Conditions of `if` expressions are
    /// not recorded.
    pub paren_conditions: Vec<ParenCondition>,
    /// Every string token written in this body, quotes included: `"…"`,
    /// `'…'`, `[[…]]`, each literal piece of a backtick string (delimiters
    /// included: `` `a{ ``, `}b{`, `` }c` ``), a call's string argument
    /// (`f"…"`), and strings in type position (`type T = "a" | "b"`). The
    /// tree keeps a string's content, not how it was escaped. Not in
    /// source order, and a token may be listed twice.
    pub string_tokens: Vec<Span>,
    /// Every keyed field of every table constructor written in this body
    /// (`name = v`, `[k] = v`), in source order. The tree keeps a field's
    /// name and value, not where the field as a whole starts and ends.
    /// Positional items are not listed.
    pub table_fields: Vec<TableFieldSpan>,
    /// Every `;` that ends a statement of this body (`x = 1;`), in source
    /// order. A statement's span stops at the statement. The `;` after a
    /// `return`, `break` or `continue` is not listed.
    pub semicolons: Vec<StmtSemicolon>,
}

/// The `;` written after one statement.
#[derive(Debug, Clone, PartialEq)]
pub struct StmtSemicolon {
    /// The statement it ends: equal to that `Stmt::span`, which is how a
    /// statement finds its `;`.
    pub stmt: Span,
    /// The `;` itself. Whitespace and comments may sit between the two.
    pub semicolon: Span,
}

/// Where one keyed table-constructor field sits in the source.
#[derive(Debug, Clone, PartialEq)]
pub struct TableFieldSpan {
    /// The key as written: the name in `name = v`, the expression between
    /// the brackets in `[k] = v`. Equal to `TableField::Named::span` for a
    /// named field, which is how a field is looked up here.
    pub key: Span,
    /// The whole field: from the start of the key (the `[` for a bracketed
    /// one) to the end of the value, and on through the `,` or `;` that
    /// follows when there is one. Deleting it leaves a table that still
    /// parses.
    pub extent: Span,
}

/// A condition written as `(expr)`, where the parentheses hold all of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ParenCondition {
    /// The keyword the condition follows: `if`, `elseif`, `while` or
    /// `until`.
    pub keyword: &'static str,
    /// From the outermost `(` to its `)`.
    pub span: Span,
    /// What the innermost pair holds: everything between its `(` and `)`,
    /// surrounding whitespace and comments included. Equal to `span` less
    /// one character each side unless the condition is `((x))`.
    pub inner: Span,
}

#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub terminator: Option<Terminator>,
}

#[derive(Debug)]
pub enum Terminator {
    /// `return` — empty `exprs` means a bare return (returns no values;
    /// for definite-return purposes it violates a non-nil return type,
    /// and `span` is where the diagnostic points).
    Return {
        exprs: Vec<Expr>,
        span: Span,
    },
    Break,
    Continue,
}

#[derive(Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum StmtKind {
    /// `local a, b = e1, e2` — initializers evaluate before names bind
    /// (`local x = x` reads the outer `x`). `const a, b = e1, e2` is the
    /// same statement with every decl marked `is_const`.
    Local {
        decls: Vec<LocalDecl>,
        init: Vec<Expr>,
    },
    /// `a, t.x = e1, e2`
    Assign {
        targets: Vec<Target>,
        values: Vec<Expr>,
    },
    /// `x += e` — reads and writes the target.
    CompoundAssign {
        target: Target,
        value: Expr,
    },
    /// A call in statement position.
    Call(Expr),
    Do(Block),
    If {
        cond: Expr,
        then: Block,
        else_ifs: Vec<(Expr, Block)>,
        or_else: Option<Block>,
    },
    While {
        cond: Expr,
        body: Block,
    },
    /// `repeat body until cond` — the condition sees locals declared in the
    /// body; the lowering keeps them in one scope and the CFG respects it.
    Repeat {
        body: Block,
        until: Expr,
    },
    NumericFor {
        var: LocalDecl,
        start: Expr,
        end: Expr,
        step: Option<Expr>,
        body: Block,
    },
    GenericFor {
        vars: Vec<LocalDecl>,
        exprs: Vec<Expr>,
        body: Block,
    },
    /// `local function f() ... end` — `f` is bound before the body, so
    /// recursion resolves. `const function f() ... end` is the same
    /// statement with the decl marked `is_const`.
    LocalFunction {
        decl: LocalDecl,
        body: BodyId,
    },
    /// `function f() end` / `function a.b:c() end`. Only the plain
    /// single-name, non-method form assigns a local; the checker decides by
    /// `assigns_root`.
    FunctionDecl {
        root: String,
        /// Dot-path segments after the root (`a.b.c` → ["b", "c"]), then the
        /// method name if declared with `:`. The call-conformance registry uses
        /// single-segment paths (`Tbl.f` / `Tbl:m`).
        path: Vec<String>,
        /// Declared with `:` — the callee has an implicit `self` parameter
        /// that `params` does not list.
        is_method: bool,
        assigns_root: bool,
        body: BodyId,
    },
}

#[derive(Debug)]
pub struct LocalDecl {
    pub name: String,
    pub span: Span,
    pub ty: Option<Ty>,
    /// Declared with `const` (`const x = e`, `const function f`). Scoping,
    /// shadowing and reads are those of a `local`; the one difference is
    /// that Luau refuses to compile any later assignment to the binding.
    pub is_const: bool,
}

#[derive(Debug)]
pub enum Target {
    /// Plain name — a local write if it resolves, else a global write.
    Name { name: String, span: Span },
    /// `t.x = v`, `t[k] = v` — mutates the table; reads everything inside.
    Projection(Expr),
}

/// Expressions, reduced to what flow analysis observes.
#[derive(Debug)]
pub enum Expr {
    /// An identifier reference. Local-vs-global resolution happens in the
    /// checker, which owns scope.
    Name { name: String, span: Span },
    /// Any call. `method` is Some for `base:m(args)` — the colon form, whose
    /// implicit self makes it a different arity than `base.m(args)`.
    /// `arg_spans` parallels `args` so a diagnostic can point at one
    /// argument; `span` covers the whole call.
    Call {
        base: Box<Expr>,
        args: Vec<Expr>,
        method: Option<String>,
        span: Span,
        arg_spans: Vec<Span>,
    },
    /// `base.name` (name Some, key None) or `base[expr]` (key Some).
    ///
    /// The dot-field name is kept so the checker can recognize builtin
    /// projections (`table.freeze`, `table.clone`, `table.isfrozen`) without
    /// consulting the parser. Bracket keys carry no name. `name_span` is the
    /// dot-field name token's span (None for bracket form) — member-existence
    /// diagnostics anchor on it, and it is the callee-name span autofixes
    /// need.
    Index {
        base: Box<Expr>,
        key: Option<Box<Expr>>,
        name: Option<String>,
        name_span: Option<Span>,
    },
    /// A closure literal; the body is in the module arena.
    Function(BodyId),
    /// `lhs op rhs`. Kept structured because narrowing conditions are made
    /// of these (`x ~= nil`, `a and b`, `type(x) == "string"`).
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `not e`. Kept separate from `UnOp` because narrowing consumes it as
    /// condition structure (fact negation), not as a value operator.
    Not(Box<Expr>),
    /// `-e` / `#e`. Kept structured so syntactic rules can see the operator
    /// (`for i = #t, 1` reverse-loop detection needs `#`); flow analyses
    /// treat it as a transparent read of the operand. `span` covers the
    /// operator and its operand.
    UnOp {
        op: UnOp,
        operand: Box<Expr>,
        span: Span,
    },
    /// A table constructor with its field structure kept — constructor keys
    /// feed the duplicate-key rule and, later, table literal typing.
    Table(Vec<TableField>),
    /// `if c then a elseif c2 then b else z` — arms in source order, the
    /// final `(None, z)` is the else. Kept structured because the
    /// default-fill idiom (`x = if type(x) == "number" then x else 1`) must
    /// be provable non-nil or narrowing drowns in false positives.
    IfExpr(Vec<(Option<Expr>, Expr)>),
    /// Any composite with subexpressions whose structure flow analysis does
    /// not care about (interpolated strings...).
    Composite(Vec<Expr>),
    /// A literal whose primitive type is knowable at a glance — what
    /// milestone 2 argument-type checks consume. `text` carries a string
    /// literal's content (quotes stripped) — require-path resolution reads
    /// `GetService("X")` / `WaitForChild("X")` arguments from it — a number
    /// literal's raw source text, or `"true"`/`"false"`. Consumers matching
    /// on `text` must also match `prim` (a number's text is not a name).
    Literal {
        prim: Prim,
        span: Span,
        text: Option<String>,
    },
    /// Anything else with nothing to read.
    Leaf,
}

/// The value-producing unary operators (`not` is `Expr::Not`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// `-e`
    Neg,
    /// `#e`
    Len,
}

/// Binary operators, one-to-one with Luau's. Narrowing consumes `Eq`, `Ne`,
/// `And`, `Or`; the rest are kept so no operator loses its identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    /// Whether a nil operand throws at runtime: arithmetic, concat, and the
    /// ordering comparisons. `==`/`~=` accept nil (they ARE the nil checks),
    /// and `and`/`or` are control flow.
    pub fn requires_non_nil_operands(self) -> bool {
        use BinOp::*;
        matches!(
            self,
            Add | Sub | Mul | Div | FloorDiv | Mod | Pow | Concat | Lt | Le | Gt | Ge
        )
    }
}

/// One field of a table constructor. Evaluation order within a field is
/// key before value, matching the VM.
#[derive(Debug)]
pub enum TableField {
    /// `name = value` — the span points at the key name (duplicate-key
    /// diagnostics anchor here).
    Named {
        name: String,
        span: Span,
        value: Expr,
    },
    /// `[key] = value`.
    Keyed { key: Expr, value: Expr },
    /// A positional item.
    Item(Expr),
}

/// The primitive classification both literals and written types reduce to.
/// Deliberately tiny: argument-type checking only fires when both sides
/// classify, so everything unclassifiable stays silent rather than wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prim {
    Number,
    String,
    Boolean,
    Nil,
}

impl Prim {
    pub fn name(self) -> &'static str {
        match self {
            Prim::Number => "number",
            Prim::String => "string",
            Prim::Boolean => "boolean",
            Prim::Nil => "nil",
        }
    }
}

/// The type summary current analyses consume. `rendered` is what the user
/// wrote, for diagnostics; `span` points at the annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Ty {
    /// Whether nil is a legal value of this type (`T?`, `nil`, `any`,
    /// unions containing one of those, `()`). Computed at lowering.
    pub admits_nil: bool,
    /// The annotation is `any`, `any?`, or a union containing `any`. Nil is
    /// admitted, but as a type-system opt-out, not as a claim that nil is
    /// expected — R0201 never lets such an annotation upgrade an
    /// unclassifiable write into a nil claim, so those writes stay opaque
    /// and are reported conservatively. Computed at lowering.
    pub any: bool,
    /// The primitive this annotation names, when it plainly names one
    /// (`number`, `string?`, ...). None for tables, functions, unions,
    /// generics — those wait for the real type system.
    pub prim: Option<Prim>,
    pub rendered: String,
    pub span: Span,
}
