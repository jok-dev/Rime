//! rime-syntax: parse with full-moon and lower to the parser-agnostic HIR.
//!
//! This is the only crate that names a parser type. Swapping the parser
//! means reimplementing `lower_module` (and `parse`), nothing else.
//!
//! Lowering preserves evaluation order exactly as the CFG consumes it:
//! initializers before their names bind, assignment values before targets,
//! loop bound expressions before the loop variable.

use full_moon::ast;
use full_moon::node::Node;
use full_moon::tokenizer::TokenReference;
use rime_hir as hir;

/// A parse failure, already parser-neutral. `message` is the short reason
/// ("unexpected token `local`", "unclosed string"); position noise stays out
/// of it — `span` carries the offending token's byte range and `note` any
/// extra parser detail, so the diagnostic layer can place and phrase them.
#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub span: Option<hir::Span>,
    pub note: Option<String>,
    /// The innermost block still open where parsing stopped — what a
    /// "missing `end`" is actually missing. See `unclosed_block`.
    pub opener: Option<Opener>,
    /// Where the pairing probably went wrong, by indentation. Only ever
    /// set alongside `opener`. See `unclosed_block`.
    pub hint: Option<IndentHint>,
    /// The parser read the file; the lowering met syntax the HIR has no
    /// model for (see `Lowerer::condition_binding`). `message` then names
    /// that syntax rather than a token the parser tripped on, and `span`
    /// is where it starts.
    pub unsupported: bool,
}

/// A block keyword: its byte span and its kind (`function`, `if`, `do`,
/// `for`, `while`, `repeat` for openers; `end`, `until` for closers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opener {
    pub span: hir::Span,
    pub kind: &'static str,
}

/// Indentation's opinion on a missing `end`. `closer` is the first `end`/
/// `until` written shallower than the block it closed; `culprit` is that
/// block — the one most likely missing its own closer — and
/// `lines_up_with` is the enclosing block whose indentation the closer
/// matches, when one does (what the closer was written for).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndentHint {
    pub culprit: Opener,
    pub closer: Opener,
    pub lines_up_with: Option<Opener>,
}

/// Parse + lower. The single entry point.
pub fn parse(src: &str) -> Result<hir::Module, Vec<ParseError>> {
    let ast = full_moon::parse(src).map_err(|errors| {
        errors
            .into_iter()
            .map(|e| {
                let (start, end) = e.range();
                let span = Some(start.bytes()..end.bytes());
                match e {
                    full_moon::Error::AstError(e) => {
                        let at_eof = matches!(
                            e.token().token_type(),
                            full_moon::tokenizer::TokenType::Eof
                        );
                        // Scan up to the offending TOKEN, not the error's
                        // node range: for a missing `end` that range is the
                        // whole trailing statement, which would hide every
                        // closer inside it from the matcher.
                        let stop = e.token().start_position().bytes();
                        let (opener, hint) = unclosed_block(src, stop);
                        ParseError {
                            message: if at_eof {
                                "unexpected end of file".to_string()
                            } else {
                                format!("unexpected token `{}`", e.token())
                            },
                            span,
                            note: Some(e.error_message().to_string())
                                .filter(|n| !n.is_empty()),
                            opener,
                            hint,
                            unsupported: false,
                        }
                    }
                    full_moon::Error::TokenizerError(e) => ParseError {
                        message: e.error().to_string(),
                        span,
                        note: None,
                        opener: None,
                        hint: None,
                        unsupported: false,
                    },
                }
            })
            .collect::<Vec<_>>()
    })?;
    let (module, unsupported) = lower_module(&ast, src);
    if unsupported.is_empty() {
        Ok(module)
    } else {
        Err(unsupported)
    }
}

/// The innermost block still open at byte offset `stop`, by a token-level
/// bracket match over the source (the lexer works on files the parser
/// rejects). Openers: `function`, `if`, `do`, `for`/`while` (whose own `do`
/// is part of the header, not a second opener), `repeat`; `end` closes the
/// first five, `until` closes `repeat`. Pairing is innermost-first, exactly
/// as the parser would pair them, so whatever is left on the stack is what
/// a missing `end` would have closed. An extra `end` (empty stack) blames
/// nothing.
///
/// Luau's `if c then a else b` EXPRESSION has no `end`. It is told apart
/// from the statement by position: an `if` right after a token that must
/// be followed by an expression (`=`, `(`, `,`, `return`, an operator, or
/// the `then`/`else` of an enclosing if-expression) is an expression. Its
/// frame is closed by its own `else`, never by `end`. Getting this wrong
/// eats a real `end` and blames a function hundreds of lines away.
///
/// The second result is indentation's opinion (`IndentHint`): innermost-
/// first pairing blames the OUTERMOST survivor, but the block actually
/// missing its `end` is usually deep inside it. The first closer written
/// shallower than the block it pops is where the pairing went wrong — the
/// popped block is the culprit, and the enclosing frame whose indentation
/// the closer matches is what it was written for. Only whitespace prefixes
/// that nest (one is a prefix of the other) are compared; a tabs-versus-
/// spaces clash says nothing. Same idea as rustc's "this delimiter might
/// not be properly closed… it has different indentation".
fn unclosed_block<'s>(src: &'s str, stop: usize) -> (Option<Opener>, Option<IndentHint>) {
    use full_moon::tokenizer::{Lexer, LexerResult, Symbol, TokenType};
    enum Frame<'s> {
        /// A real block and the leading whitespace of its opener's line.
        Block(Opener, &'s str),
        IfExpr,
    }
    /// The leading whitespace of the line containing `offset`.
    fn line_indent(src: &str, offset: usize) -> &str {
        let line_start = src[..offset].rfind('\n').map_or(0, |i| i + 1);
        let rest = &src[line_start..];
        let len = rest.bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
        &rest[..len]
    }
    // `new_lazy`, not `new`: `new` pre-reads two tokens into its
    // current/peek slots, and `process_next` would start AFTER them —
    // silently dropping `local function` at the top of a file.
    let mut lexer = Lexer::new_lazy(src, full_moon::LuaVersion::luau());
    let mut stack: Vec<Frame<'s>> = Vec::new();
    let mut hint: Option<IndentHint> = None;
    // Set by `for`/`while`: the next `do` belongs to that header.
    let mut header_do = false;
    // The previous significant token demands an expression next, so an
    // `if` here is the expression form.
    let mut expr_next = false;
    while let Some(next) = lexer.process_next() {
        let token = match next {
            LexerResult::Ok(t) | LexerResult::Recovered(t, _) => t,
            LexerResult::Fatal(_) => break,
        };
        let start = token.start_position().bytes();
        if start >= stop || matches!(token.token_type(), TokenType::Eof) {
            break;
        }
        let symbol = match token.token_type() {
            TokenType::Symbol { symbol } => *symbol,
            TokenType::Whitespace { .. }
            | TokenType::SingleLineComment { .. }
            | TokenType::MultiLineComment { .. }
            | TokenType::Shebang { .. } => continue,
            // Identifiers, literals, interpolated strings: a value, so
            // whatever follows is not an expression start.
            _ => {
                expr_next = false;
                continue;
            }
        };
        let span = start..token.end_position().bytes();
        let in_if_expr = matches!(stack.last(), Some(Frame::IfExpr));
        let indent = line_indent(src, start);
        let open = |stack: &mut Vec<Frame<'s>>, kind| {
            stack.push(Frame::Block(Opener { span: span.clone(), kind }, indent))
        };
        // Pop the nearest real block, discarding if-expression frames a
        // malformed `if` expression may have left above it. Records the
        // first closer written shallower than what it closes.
        let mut close_block = |stack: &mut Vec<Frame<'s>>, kind: &'static str| {
            while let Some(frame) = stack.pop() {
                let Frame::Block(popped, popped_indent) = frame else { continue };
                let shallower = indent.len() < popped_indent.len()
                    && popped_indent.starts_with(indent);
                if shallower && hint.is_none() {
                    let lines_up_with = stack.iter().rev().find_map(|f| match f {
                        Frame::Block(o, i) if *i == indent => Some(o.clone()),
                        _ => None,
                    });
                    hint = Some(IndentHint {
                        culprit: popped,
                        closer: Opener { span: span.clone(), kind },
                        lines_up_with,
                    });
                }
                break;
            }
        };
        expr_next = match symbol {
            Symbol::Function => {
                open(&mut stack, "function");
                false
            }
            Symbol::If => {
                if expr_next {
                    stack.push(Frame::IfExpr);
                } else {
                    open(&mut stack, "if");
                }
                true
            }
            Symbol::Repeat => {
                open(&mut stack, "repeat");
                false
            }
            Symbol::For => {
                open(&mut stack, "for");
                header_do = true;
                false
            }
            Symbol::While => {
                open(&mut stack, "while");
                header_do = true;
                true
            }
            Symbol::Do => {
                if header_do {
                    header_do = false;
                } else {
                    open(&mut stack, "do");
                }
                false
            }
            Symbol::End => {
                close_block(&mut stack, "end");
                false
            }
            Symbol::Until => {
                close_block(&mut stack, "until");
                true
            }
            // Inside an if-expression, `then`/`else` are followed by the
            // branch expressions and `else` ends it; in a statement they
            // start a block of statements.
            Symbol::Then => in_if_expr,
            Symbol::Else => {
                if in_if_expr {
                    stack.pop();
                }
                in_if_expr
            }
            Symbol::ElseIf => true,
            Symbol::Equal
            | Symbol::LeftParen
            | Symbol::Comma
            | Symbol::LeftBracket
            | Symbol::LeftBrace
            | Symbol::Return
            | Symbol::In
            | Symbol::And
            | Symbol::Or
            | Symbol::Not
            | Symbol::Hash
            | Symbol::Plus
            | Symbol::Minus
            | Symbol::Star
            | Symbol::Slash
            | Symbol::DoubleSlash
            | Symbol::Percent
            | Symbol::Caret
            | Symbol::TwoDots
            | Symbol::TwoEqual
            | Symbol::TildeEqual
            | Symbol::LessThan
            | Symbol::LessThanEqual
            | Symbol::GreaterThan
            | Symbol::GreaterThanEqual
            | Symbol::PlusEqual
            | Symbol::MinusEqual
            | Symbol::StarEqual
            | Symbol::SlashEqual
            | Symbol::DoubleSlashEqual
            | Symbol::PercentEqual
            | Symbol::CaretEqual
            | Symbol::TwoDotsEqual => true,
            _ => false,
        };
    }
    let opener = stack.into_iter().rev().find_map(|f| match f {
        Frame::Block(o, _) => Some(o),
        Frame::IfExpr => None,
    });
    // A hint without a missing closer is noise (an extra `end` is its own
    // error, at its own site).
    let hint = hint.filter(|_| opener.is_some());
    (opener, hint)
}

/// The lowered module, and every construct the lowering refused (in source
/// order). A non-empty second half means the module is not to be checked:
/// it was lowered without those constructs.
fn lower_module<'a>(ast: &'a ast::Ast, src: &'a str) -> (hir::Module, Vec<ParseError>) {
    let mut lowerer = Lowerer {
        bodies: Vec::new(),
        src,
        type_refs: std::collections::HashSet::new(),
        aliases: top_level_aliases(ast.nodes()),
        unknown_members: std::collections::HashSet::new(),
        unsupported: Vec::new(),
        notes: vec![hir::SyntaxNotes::default()],
    };
    // Reserve the root slot first so nested bodies get later ids.
    lowerer.bodies.push(placeholder_body());
    let block = lowerer.block(ast.nodes());
    lowerer.bodies[0] = hir::Body {
        params: Vec::new(),
        has_varargs: true, // a module body sees `...` (its require args)
        return_ty: None,
        end_span: None,
        block,
        attributes: Vec::new(),
        notes: lowerer.notes.pop().unwrap_or_default(),
    };
    let module = hir::Module {
        bodies: lowerer.bodies,
        root: 0,
        type_refs: lowerer.type_refs,
        rebound_require: std::collections::HashSet::new(),
        unknown_members: lowerer.unknown_members,
    };
    (module, lowerer.unsupported)
}

fn placeholder_body() -> hir::Body {
    hir::Body {
        params: Vec::new(),
        has_varargs: false,
        return_ty: None,
        end_span: None,
        block: hir::Block {
            stmts: Vec::new(),
            terminator: None,
        },
        attributes: Vec::new(),
        notes: hir::SyntaxNotes::default(),
    }
}

fn span_of(node: &impl Node) -> hir::Span {
    node.range()
        .map(|(s, e)| s.bytes()..e.bytes())
        .unwrap_or(0..0)
}

/// full_moon quirk, 2.x and 3.0 (upstream-reportable, same family as the
/// TypeInfo::Tuple workaround in `lower_ty`): a node whose final written
/// token lives in an EARLIER struct field reports a `range()` that stops
/// short of it. `Index::Brackets` declares its `ContainedSpan` before the
/// key expression, so `t[k]` at the end of any node loses its `]`; the
/// parenthesis and brace pairs have the same field shape. Rebalance
/// against the source: while delimiters inside the span are unbalanced,
/// absorb whitespace up to — and only when followed by — the missing
/// closer. Delimiters inside string literals are not modelled; a string
/// containing a bare closer leaves the span as short as it was before.
fn rebalance(mut span: hir::Span, src: &str) -> hir::Span {
    let bytes = src.as_bytes();
    let (mut parens, mut brackets, mut braces) = (0i32, 0i32, 0i32);
    let end = span.end.min(bytes.len());
    for b in &bytes[span.start.min(end)..end] {
        match b {
            b'(' => parens += 1,
            b')' => parens -= 1,
            b'[' => brackets += 1,
            b']' => brackets -= 1,
            b'{' => braces += 1,
            b'}' => braces -= 1,
            _ => {}
        }
    }
    while parens > 0 || brackets > 0 || braces > 0 {
        let mut i = span.end;
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
        }
        match bytes.get(i) {
            Some(b')') if parens > 0 => {
                parens -= 1;
                span.end = i + 1;
            }
            Some(b']') if brackets > 0 => {
                brackets -= 1;
                span.end = i + 1;
            }
            Some(b'}') if braces > 0 => {
                braces -= 1;
                span.end = i + 1;
            }
            _ => break,
        }
    }
    span
}

fn name_of(token: &TokenReference) -> String {
    token.token().to_string()
}

/// The attributes written before a `function`, as written: `@name` kept
/// whether or not Luau knows the name.
fn attributes<'a>(
    attrs: impl Iterator<Item = &'a ast::luau::LuauAttribute>,
) -> Vec<hir::Attribute> {
    attrs
        .map(|a| hir::Attribute {
            name: name_of(a.name()),
            span: span_of(a),
        })
        .collect()
}

/// A string literal's content with quotes stripped. Long-bracket strings
/// (`[[x]]`) keep their raw form minus brackets; escape sequences are NOT
/// decoded — require-path segments never contain them in practice.
fn string_content(tok: &TokenReference) -> String {
    let raw = tok.token().to_string();
    let b = raw.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        return raw[1..raw.len() - 1].to_string();
    }
    if let Some(rest) = raw.strip_prefix("[[") {
        if let Some(inner) = rest.strip_suffix("]]") {
            return inner.to_string();
        }
    }
    raw
}

/// Whether a written type admits nil — `T?`, `nil`, `any`, unions containing
/// one, and the empty tuple `()`.
fn admits_nil(t: &ast::luau::TypeInfo) -> bool {
    use ast::luau::TypeInfo as T;
    match t {
        T::Optional { .. } => true,
        T::Basic(tok) => {
            let s = tok.token().to_string();
            s == "nil" || s == "any"
        }
        T::Union(u) => u.types().iter().any(admits_nil),
        T::Tuple { types, .. } => types.is_empty() || types.iter().all(admits_nil),
        _ => false,
    }
}

/// Whether a written type is `any` or wraps/unions one — the user opted
/// out of typing, which is not a claim that nil is expected.
fn is_any(t: &ast::luau::TypeInfo) -> bool {
    use ast::luau::TypeInfo as T;
    match t {
        T::Basic(tok) => tok.token().to_string() == "any",
        T::Optional { base, .. } => is_any(base),
        T::Union(u) => u.types().iter().any(is_any),
        T::Tuple { types, .. } => types.iter().any(is_any),
        _ => false,
    }
}

/// The primitive an annotation plainly names, unwrapping `T?`. Anything
/// structured (tables, functions, unions, generics) is None — argument-type
/// checks only fire when both sides classify.
fn prim_of(t: &ast::luau::TypeInfo) -> Option<hir::Prim> {
    use ast::luau::TypeInfo as T;
    match t {
        T::Basic(tok) => match tok.token().to_string().as_str() {
            "number" => Some(hir::Prim::Number),
            "string" => Some(hir::Prim::String),
            "boolean" => Some(hir::Prim::Boolean),
            "nil" => Some(hir::Prim::Nil),
            _ => None,
        },
        T::Optional { base, .. } => prim_of(base),
        _ => None,
    }
}

/// Map full-moon's operator token to the HIR operator. A `None` (future
/// grammar growth) falls back to `Composite` — structure lost, reads kept.
fn lower_binop(op: &ast::BinOp) -> Option<hir::BinOp> {
    use ast::BinOp as B;
    Some(match op {
        B::Plus(_) => hir::BinOp::Add,
        B::Minus(_) => hir::BinOp::Sub,
        B::Star(_) => hir::BinOp::Mul,
        B::Slash(_) => hir::BinOp::Div,
        B::DoubleSlash(_) => hir::BinOp::FloorDiv,
        B::Percent(_) => hir::BinOp::Mod,
        B::Caret(_) => hir::BinOp::Pow,
        B::TwoDots(_) => hir::BinOp::Concat,
        B::TwoEqual(_) => hir::BinOp::Eq,
        B::TildeEqual(_) => hir::BinOp::Ne,
        B::LessThan(_) => hir::BinOp::Lt,
        B::LessThanEqual(_) => hir::BinOp::Le,
        B::GreaterThan(_) => hir::BinOp::Gt,
        B::GreaterThanEqual(_) => hir::BinOp::Ge,
        B::And(_) => hir::BinOp::And,
        B::Or(_) => hir::BinOp::Or,
        _ => return None,
    })
}

/// The file's top-level `type X = ...` and `export type X = ...`, by name.
/// Aliases are not ordered in Luau (one may be used above its definition),
/// so they are collected before anything is lowered. A later definition of
/// the same name wins; Luau rejects the duplicate anyway.
fn top_level_aliases(block: &ast::Block) -> std::collections::HashMap<String, &ast::luau::TypeInfo> {
    let mut aliases = std::collections::HashMap::new();
    for stmt in block.stmts() {
        let decl = match stmt {
            ast::Stmt::TypeDeclaration(decl) => decl,
            ast::Stmt::ExportedTypeDeclaration(exported) => exported.type_declaration(),
            _ => continue,
        };
        aliases.insert(name_of(decl.type_name()), decl.type_definition());
    }
    aliases
}

/// `e` with any enclosing parentheses taken off.
fn unparenthesized(mut e: &ast::Expression) -> &ast::Expression {
    while let ast::Expression::Parentheses { expression, .. } = e {
        e = expression;
    }
    e
}

fn lower_ty(ts: &ast::luau::TypeSpecifier, src: &str) -> hir::Ty {
    let info = ts.type_info();
    // The span covers the type itself, not the `:` (an annotation label
    // should select `(CFrame, CFrame)`, not `: (CFrame, CFrame`).
    let mut span = span_of(info);
    // full_moon quirk, 2.x and 3.0: TypeInfo::Tuple's range()/tokens() miss the
    // closing parenthesis token (upstream-reportable). Rebalance against
    // the source so the underline covers the whole written type.
    let mut opens: i32 = src[span.clone()]
        .bytes()
        .map(|b| match b {
            b'(' => 1,
            b')' => -1,
            _ => 0,
        })
        .sum();
    let bytes = src.as_bytes();
    while opens > 0 && span.end < bytes.len() {
        match bytes[span.end] {
            b')' => {
                opens -= 1;
                span.end += 1;
            }
            b' ' | b'\t' | b'\r' | b'\n' | b'?' => span.end += 1,
            _ => break,
        }
    }
    hir::Ty {
        admits_nil: admits_nil(info),
        any: is_any(info),
        prim: prim_of(info),
        rendered: info.to_string().trim().to_string(),
        span,
    }
}

struct Lowerer<'s> {
    bodies: Vec<hir::Body>,
    src: &'s str,
    type_refs: std::collections::HashSet<String>,
    /// See `top_level_aliases`.
    aliases: std::collections::HashMap<String, &'s ast::luau::TypeInfo>,
    /// Becomes `hir::Module::unknown_members`.
    unknown_members: std::collections::HashSet<usize>,
    /// Constructs the parser accepted and the HIR cannot express.
    unsupported: Vec<ParseError>,
    /// The notes of each body being lowered, innermost last: the file's,
    /// then one per enclosing function. A body's notes are about its own
    /// statements, so a nested function pushes a fresh entry.
    notes: Vec<hir::SyntaxNotes>,
}

impl Lowerer<'_> {
    /// Refuse a condition binding: `if local x = e then`, `if const x = e
    /// then`, either on an `elseif`, and the same four in if-expressions.
    ///
    /// full_moon 3.0 parses them. In Luau they are experimental — the RFC
    /// is still open and the compiler reads them only behind a flag. The
    /// name is declared INSIDE the `if`, and the HIR's `If` and `IfExpr`
    /// have a condition and nothing that can declare a name there.
    /// Lowering the condition alone would drop the binding and report
    /// every use of the name as undefined (R0022); lowering it as a
    /// `local` before the `if` would leak the name past `end` and make two
    /// `elseif local m = …` arms shadow each other (R0011). Either is a
    /// wrong report on valid code, so the file is refused instead, with
    /// the reason — which is also what happened before the parser could
    /// read this syntax, minus the reason.
    fn condition_binding(
        &mut self,
        keyword: &str,
        binding: Option<&ast::luau::IfConditionBinding>,
    ) {
        let Some(binding) = binding else {
            return;
        };
        let introducer = binding.local_token();
        let name = name_of(binding.name());
        self.unsupported.push(ParseError {
            message: format!(
                "`{keyword} {}` bindings are experimental Luau syntax that Rime does not understand yet",
                introducer.token()
            ),
            span: Some(span_of(introducer)),
            note: Some(format!(
                "`{name}` is declared inside the condition, which Rime's model of an `if` \
                 has no place for; until it does, declare `{name}` on the line before the \
                 `if` and test it there"
            )),
            opener: None,
            hint: None,
            unsupported: true,
        });
    }

    /// Whether the written type `t` says the value's members cannot be
    /// listed: it is a table type with an indexer that takes string keys,
    /// or an alias of this file that comes to one. `T?`, unions and
    /// intersections count when any part does: a part that admits every
    /// key makes the whole admit it.
    ///
    /// `any` does NOT count. It is written to turn type checking off for a
    /// name (a cyclic require, a table the checker cannot follow), not to
    /// say the table holds keys the file never gives it; the members the
    /// file defines are still all there are. An indexer is the statement
    /// about keys, and `{ [string]: any }` counts for its indexer.
    ///
    /// An indexer counts unless its key is plainly `number` (`{ T }` is
    /// that too): a generic key (`{ [K]: V }`) may be a string, and saying
    /// "unknown" there only costs a check. A name that is not an alias of
    /// this file (a type from another module, a builtin) is not followed
    /// and counts as known, which is what every such annotation was
    /// before. `seen` is the aliases already being expanded, so a
    /// recursive alias ends.
    fn unknown_members(&self, t: &ast::luau::TypeInfo, seen: &mut Vec<String>) -> bool {
        use ast::luau::TypeFieldKey as K;
        use ast::luau::TypeInfo as T;
        let alias = |name: String, seen: &mut Vec<String>| -> bool {
            if seen.contains(&name) {
                return false;
            }
            let Some(definition) = self.aliases.get(&name).copied() else {
                return false;
            };
            seen.push(name);
            let unknown = self.unknown_members(definition, seen);
            seen.pop();
            unknown
        };
        match t {
            T::Basic(tok) => alias(tok.token().to_string(), seen),
            T::Generic { base, .. } => alias(base.token().to_string(), seen),
            T::Table { fields, .. } => fields.iter().any(|field| match field.key() {
                K::IndexSignature { inner, .. } => {
                    !matches!(&**inner, T::Basic(tok) if tok.token().to_string() == "number")
                }
                _ => false,
            }),
            T::Optional { base, .. } => self.unknown_members(base, seen),
            T::Union(union) => union.types().iter().any(|part| self.unknown_members(part, seen)),
            T::Intersection(both) => both.types().iter().any(|part| self.unknown_members(part, seen)),
            T::Tuple { types, .. } if types.len() == 1 => {
                types.iter().next().is_some_and(|only| self.unknown_members(only, seen))
            }
            _ => false,
        }
    }

    /// Whether `e` is a cast (`value :: T`, parentheses aside) to a type
    /// with unknown members. Only the outermost cast is read: in
    /// `(v :: T) :: U` the value's type is `U`.
    fn cast_to_unknown_members(&self, e: &ast::Expression) -> bool {
        match unparenthesized(e) {
            ast::Expression::TypeAssertion { type_assertion, .. } => {
                self.unknown_members(type_assertion.cast_to(), &mut Vec::new())
            }
            _ => false,
        }
    }

    /// Notes which of a `local` / `const` statement's names are given a
    /// type with unknown members, by annotation or by a cast on the
    /// initializer in the same position (see `hir::Module::unknown_members`).
    fn note_unknown_members<'n>(
        &mut self,
        names: impl Iterator<Item = &'n TokenReference>,
        specifiers: impl Iterator<Item = Option<&'n ast::luau::TypeSpecifier>>,
        initializers: &ast::punctuated::Punctuated<ast::Expression>,
    ) {
        for (i, (name, specifier)) in names.zip(specifiers).enumerate() {
            let unknown = match specifier {
                Some(specifier) => self.unknown_members(specifier.type_info(), &mut Vec::new()),
                None => initializers.iter().nth(i).is_some_and(|e| self.cast_to_unknown_members(e)),
            };
            if unknown {
                self.unknown_members.insert(span_of(name).start);
            }
        }
    }

    /// Record every identifier inside a type-position node (an annotation,
    /// a `type X = …` declaration, a `::` assertion, a generic
    /// instantiation). Feeds `hir::Module::type_refs` — see its doc for the
    /// over-approximation contract.
    fn type_names(&mut self, node: &impl Node) {
        for tok in node.tokens() {
            match tok.token().token_type() {
                full_moon::tokenizer::TokenType::Identifier { identifier } => {
                    self.type_refs.insert(identifier.to_string());
                }
                // A string in type position (`"a" | "b"`) never reaches
                // `expr`; this is the one place it is seen.
                full_moon::tokenizer::TokenType::StringLiteral { .. } => self.note_string(tok),
                _ => {}
            }
        }
    }

    /// Records one string token on the body being lowered (see
    /// `hir::SyntaxNotes::string_tokens`).
    fn note_string(&mut self, tok: &full_moon::tokenizer::TokenReference) {
        let span = tok.token().start_position().bytes()..tok.token().end_position().bytes();
        if let Some(notes) = self.notes.last_mut() {
            notes.string_tokens.push(span);
        }
    }

    /// Lower a written annotation AND record its type-position names.
    fn ty(&mut self, ts: &ast::luau::TypeSpecifier) -> hir::Ty {
        self.type_names(ts);
        lower_ty(ts, self.src)
    }

    fn function_body(
        &mut self,
        body: &ast::FunctionBody,
        attributes: Vec<hir::Attribute>,
    ) -> hir::BodyId {
        let params = body
            .parameters()
            .iter()
            .zip(body.type_specifiers())
            .filter_map(|(p, ts)| match p {
                ast::Parameter::Name(t) => Some(hir::LocalDecl {
                    name: name_of(t),
                    span: span_of(t),
                    ty: ts.map(|t| self.ty(t)),
                    is_const: false,
                }),
                _ => None, // `...` — not a named local
            })
            .collect();
        let has_varargs = body
            .parameters()
            .iter()
            .any(|p| !matches!(p, ast::Parameter::Name(_)));
        let return_ty = body.return_type().map(|t| self.ty(t));
        let id = self.bodies.len();
        self.bodies.push(placeholder_body());
        self.notes.push(hir::SyntaxNotes::default());
        let block = self.block(body.block());
        self.bodies[id] = hir::Body {
            params,
            has_varargs,
            return_ty,
            end_span: Some(span_of(body.end_token())),
            block,
            attributes,
            notes: self.notes.pop().unwrap_or_default(),
        };
        id
    }

    fn block(&mut self, block: &ast::Block) -> hir::Block {
        let mut stmts = Vec::new();
        for (stmt, semicolon) in block.stmts_with_semicolon() {
            let stmt = self.stmt(stmt);
            if let (Some(semicolon), Some(notes)) = (semicolon, self.notes.last_mut()) {
                let token = semicolon.token();
                notes.semicolons.push(hir::StmtSemicolon {
                    stmt: stmt.span.clone(),
                    semicolon: token.start_position().bytes()..token.end_position().bytes(),
                });
            }
            stmts.push(stmt);
        }
        let terminator = block.last_stmt().and_then(|ls| match ls {
            ast::LastStmt::Return(r) => {
                let span = rebalance(span_of(r), self.src);
                if r.returns().iter().next().is_some_and(|e| self.cast_to_unknown_members(e)) {
                    self.unknown_members.insert(span.start);
                }
                Some(hir::Terminator::Return {
                    exprs: r.returns().iter().map(|e| self.expr(e)).collect(),
                    span,
                })
            }
            ast::LastStmt::Break(_) => Some(hir::Terminator::Break),
            ast::LastStmt::Continue(_) => Some(hir::Terminator::Continue),
            _ => None,
        });
        hir::Block { stmts, terminator }
    }

    /// Records a statement condition written as `(expr)` (see
    /// `hir::ParenCondition`). The spans come from the parenthesis tokens
    /// themselves: `Expression::Parentheses` is one of the nodes whose
    /// `range()` stops short of its closer (see `rebalance`).
    fn note_condition(&mut self, keyword: &'static str, cond: &ast::Expression) {
        let ast::Expression::Parentheses { contained, expression } = cond else {
            return;
        };
        let (open, close) = contained.tokens();
        let span = open.token().start_position().bytes()..close.token().end_position().bytes();
        let (mut open, mut close, mut held) = (open, close, &**expression);
        while let ast::Expression::Parentheses { contained, expression } = held {
            (open, close) = contained.tokens();
            held = &**expression;
        }
        let inner = open.token().end_position().bytes()..close.token().start_position().bytes();
        if let Some(notes) = self.notes.last_mut() {
            notes.paren_conditions.push(hir::ParenCondition { keyword, span, inner });
        }
    }

    fn stmt(&mut self, stmt: &ast::Stmt) -> hir::Stmt {
        use ast::Stmt as S;
        let span = rebalance(span_of(stmt), self.src);
        let kind = match stmt {
            S::LocalAssignment(la) => {
                self.note_unknown_members(la.names().iter(), la.type_specifiers(), la.expressions());
                let init = la.expressions().iter().map(|e| self.expr(e)).collect();
                let decls = la
                    .names()
                    .iter()
                    .zip(la.type_specifiers())
                    .map(|(name, ts)| hir::LocalDecl {
                        name: name_of(name),
                        span: span_of(name),
                        ty: ts.map(|t| self.ty(t)),
                        is_const: false,
                    })
                    .collect();
                hir::StmtKind::Local { decls, init }
            }
            // `const a, b = e1, e2` — a `local` whose names can never be
            // assigned again. Same statement in the HIR, so scope, unused,
            // shadowing and narrowing treat it as the local it is; the
            // flag is what the const-reassign rule (R0043) reads.
            S::ConstAssignment(ca) => {
                self.note_unknown_members(ca.names().iter(), ca.type_specifiers(), ca.expressions());
                let init = ca.expressions().iter().map(|e| self.expr(e)).collect();
                let decls = ca
                    .names()
                    .iter()
                    .zip(ca.type_specifiers())
                    .map(|(name, ts)| hir::LocalDecl {
                        name: name_of(name),
                        span: span_of(name),
                        ty: ts.map(|t| self.ty(t)),
                        is_const: true,
                    })
                    .collect();
                hir::StmtKind::Local { decls, init }
            }
            S::Assignment(a) => hir::StmtKind::Assign {
                values: a.expressions().iter().map(|e| self.expr(e)).collect(),
                targets: a.variables().iter().map(|v| self.target(v)).collect(),
            },
            S::CompoundAssignment(ca) => hir::StmtKind::CompoundAssign {
                value: self.expr(ca.rhs()),
                target: self.target(ca.lhs()),
            },
            S::FunctionCall(fc) => hir::StmtKind::Call(self.chain(fc.prefix(), fc.suffixes())),
            S::Do(d) => hir::StmtKind::Do(self.block(d.block())),
            S::If(i) => {
                self.condition_binding("if", i.binding());
                self.note_condition("if", i.condition());
                hir::StmtKind::If {
                    cond: self.expr(i.condition()),
                    then: self.block(i.block()),
                    else_ifs: i
                        .else_if()
                        .map(|eis| {
                            eis.iter()
                                .map(|ei| {
                                    self.condition_binding("elseif", ei.binding());
                                    self.note_condition("elseif", ei.condition());
                                    (self.expr(ei.condition()), self.block(ei.block()))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    or_else: i.else_block().map(|b| self.block(b)),
                }
            }
            S::While(w) => {
                self.note_condition("while", w.condition());
                hir::StmtKind::While {
                    cond: self.expr(w.condition()),
                    body: self.block(w.block()),
                }
            }
            S::Repeat(r) => {
                let body = self.block(r.block());
                self.note_condition("until", r.until());
                hir::StmtKind::Repeat {
                    body,
                    until: self.expr(r.until()),
                }
            }
            S::NumericFor(nf) => hir::StmtKind::NumericFor {
                start: self.expr(nf.start()),
                end: self.expr(nf.end()),
                step: nf.step().map(|e| self.expr(e)),
                var: hir::LocalDecl {
                    name: name_of(nf.index_variable()),
                    span: span_of(nf.index_variable()),
                    ty: nf.type_specifier().map(|t| self.ty(t)),
                    is_const: false,
                },
                body: self.block(nf.block()),
            },
            S::GenericFor(gf) => hir::StmtKind::GenericFor {
                exprs: gf.expressions().iter().map(|e| self.expr(e)).collect(),
                vars: gf
                    .names()
                    .iter()
                    .zip(gf.type_specifiers())
                    .map(|(name, ts)| hir::LocalDecl {
                        name: name_of(name),
                        span: span_of(name),
                        ty: ts.map(|t| self.ty(t)),
                        is_const: false,
                    })
                    .collect(),
                body: self.block(gf.block()),
            },
            S::LocalFunction(lf) => hir::StmtKind::LocalFunction {
                decl: hir::LocalDecl {
                    name: name_of(lf.name()),
                    span: span_of(lf.name()),
                    ty: None,
                    is_const: false,
                },
                body: self.function_body(lf.body(), attributes(lf.attributes())),
            },
            // `const function f() end` — `local function` with a const name.
            S::ConstFunction(cf) => hir::StmtKind::LocalFunction {
                decl: hir::LocalDecl {
                    name: name_of(cf.name()),
                    span: span_of(cf.name()),
                    ty: None,
                    is_const: true,
                },
                body: self.function_body(cf.body(), attributes(cf.attributes())),
            },
            S::FunctionDeclaration(fd) => {
                let mut names = fd.name().names().iter();
                let first = names.next();
                let mut path: Vec<String> = names.map(name_of).collect();
                let is_method = fd.name().method_name().is_some();
                if let Some(m) = fd.name().method_name() {
                    path.push(name_of(m));
                }
                let single = path.is_empty();
                hir::StmtKind::FunctionDecl {
                    root: first.map(name_of).unwrap_or_default(),
                    path,
                    is_method,
                    assigns_root: single,
                    body: self.function_body(fd.body(), attributes(fd.attributes())),
                }
            }
            // Type declarations: no control flow, no values — but the RHS
            // names are type reads (`type Item = Types.Item` keeps `Types`
            // alive). The token scan grabs the alias name too; harmless,
            // over-approximation only ever silences.
            S::TypeDeclaration(_) | S::ExportedTypeDeclaration(_) => {
                self.type_names(stmt);
                hir::StmtKind::Do(hir::Block {
                    stmts: Vec::new(),
                    terminator: None,
                })
            }
            _ => hir::StmtKind::Do(hir::Block {
                stmts: Vec::new(),
                terminator: None,
            }),
        };
        hir::Stmt { kind, span }
    }

    fn target(&mut self, var: &ast::Var) -> hir::Target {
        match var {
            ast::Var::Name(token) => hir::Target::Name {
                name: name_of(token),
                span: span_of(token),
            },
            ast::Var::Expression(ve) => {
                hir::Target::Projection(self.chain(ve.prefix(), ve.suffixes()))
            }
            _ => hir::Target::Projection(hir::Expr::Leaf),
        }
    }

    fn expr(&mut self, expr: &ast::Expression) -> hir::Expr {
        use ast::Expression as E;
        match expr {
            E::BinaryOperator { lhs, binop, rhs } => match lower_binop(binop) {
                Some(op) => hir::Expr::BinOp {
                    op,
                    lhs: Box::new(self.expr(lhs)),
                    rhs: Box::new(self.expr(rhs)),
                },
                None => hir::Expr::Composite(vec![self.expr(lhs), self.expr(rhs)]),
            },
            E::Parentheses { expression, .. } => self.expr(expression),
            E::UnaryOperator { unop, expression } => {
                let inner = Box::new(self.expr(expression));
                let span = rebalance(span_of(expr), self.src);
                match unop {
                    ast::UnOp::Not(_) => hir::Expr::Not(inner),
                    ast::UnOp::Minus(_) => hir::Expr::UnOp {
                        op: hir::UnOp::Neg,
                        operand: inner,
                        span,
                    },
                    ast::UnOp::Hash(_) => hir::Expr::UnOp {
                        op: hir::UnOp::Len,
                        operand: inner,
                        span,
                    },
                    _ => hir::Expr::Composite(vec![*inner]),
                }
            }
            E::TypeAssertion {
                expression,
                type_assertion,
            } => {
                self.type_names(type_assertion);
                self.expr(expression)
            }
            E::Function(f) => {
                hir::Expr::Function(self.function_body(f.body(), attributes(f.attributes())))
            }
            E::FunctionCall(fc) => self.chain(fc.prefix(), fc.suffixes()),
            E::IfExpression(ie) => {
                self.condition_binding("if", ie.binding());
                let mut arms = vec![(
                    Some(self.expr(ie.condition())),
                    self.expr(ie.if_expression()),
                )];
                if let Some(elseifs) = ie.else_if_expressions() {
                    for ei in elseifs {
                        self.condition_binding("elseif", ei.binding());
                        arms.push((Some(self.expr(ei.condition())), self.expr(ei.expression())));
                    }
                }
                arms.push((None, self.expr(ie.else_expression())));
                hir::Expr::IfExpr(arms)
            }
            E::InterpolatedString(is) => {
                for segment in is.segments() {
                    self.note_string(&segment.literal);
                }
                self.note_string(is.last_string());
                hir::Expr::Composite(is.expressions().map(|e| self.expr(e)).collect())
            }
            E::TableConstructor(tc) => self.table(tc),
            E::Var(ast::Var::Name(token)) => hir::Expr::Name {
                name: name_of(token),
                span: span_of(token),
            },
            E::Var(ast::Var::Expression(ve)) => self.chain(ve.prefix(), ve.suffixes()),
            E::Number(tok) => hir::Expr::Literal {
                prim: hir::Prim::Number,
                span: span_of(tok),
                text: Some(tok.token().to_string()),
            },
            E::String(tok) => {
                self.note_string(tok);
                hir::Expr::Literal {
                    prim: hir::Prim::String,
                    span: span_of(tok),
                    text: Some(string_content(tok)),
                }
            }
            E::Symbol(tok) => {
                let span = span_of(tok);
                match tok.token().to_string().as_str() {
                    s @ ("true" | "false") => hir::Expr::Literal {
                        prim: hir::Prim::Boolean,
                        span,
                        text: Some(s.to_string()),
                    },
                    "nil" => hir::Expr::Literal {
                        prim: hir::Prim::Nil,
                        span,
                        text: None,
                    },
                    "..." => {
                        // Nothing for flow analysis to read, but the read
                        // itself is a fact (see `SyntaxNotes`).
                        if let Some(notes) = self.notes.last_mut() {
                            notes.vararg_reads.push(span);
                        }
                        hir::Expr::Leaf
                    }
                    _ => hir::Expr::Leaf,
                }
            }
            _ => hir::Expr::Leaf,
        }
    }

    fn table(&mut self, tc: &ast::TableConstructor) -> hir::Expr {
        let mut fields = Vec::new();
        for pair in tc.fields().pairs() {
            let field = pair.value();
            let key = match field {
                ast::Field::ExpressionKey { key, .. } => Some(rebalance(span_of(key), self.src)),
                ast::Field::NameKey { key, .. } => Some(span_of(key)),
                _ => None,
            };
            if let Some(key) = key {
                // Noted before the value is lowered, so a nested table's
                // fields follow their parent's: source order.
                let mut extent = rebalance(span_of(field), self.src);
                if let Some(sep) = pair.punctuation() {
                    extent.end = sep.token().end_position().bytes();
                }
                if let Some(notes) = self.notes.last_mut() {
                    notes.table_fields.push(hir::TableFieldSpan { key, extent });
                }
            }
            match field {
                ast::Field::ExpressionKey { key, value, .. } => {
                    fields.push(hir::TableField::Keyed {
                        key: self.expr(key),
                        value: self.expr(value),
                    });
                }
                ast::Field::NameKey { key, value, .. } => {
                    fields.push(hir::TableField::Named {
                        name: name_of(key),
                        span: span_of(key),
                        value: self.expr(value),
                    });
                }
                ast::Field::NoKey(e) => fields.push(hir::TableField::Item(self.expr(e))),
                _ => {}
            }
        }
        hir::Expr::Table(fields)
    }

    /// Lower a prefix + suffix chain (`a.b(c)[d]:m(e)`) into nested
    /// Index/Call nodes, left to right.
    fn chain<'b>(
        &mut self,
        prefix: &ast::Prefix,
        suffixes: impl Iterator<Item = &'b ast::Suffix>,
    ) -> hir::Expr {
        let mut acc = match prefix {
            ast::Prefix::Name(token) => hir::Expr::Name {
                name: name_of(token),
                span: span_of(token),
            },
            ast::Prefix::Expression(e) => self.expr(e),
            _ => hir::Expr::Leaf,
        };
        for suffix in suffixes {
            acc = match suffix {
                ast::Suffix::Call(call) => {
                    let (args, arg_spans, method) = match call {
                        ast::Call::AnonymousCall(a) => {
                            let (args, spans) = self.args(a);
                            (args, spans, None)
                        }
                        ast::Call::MethodCall(mc) => {
                            let (args, spans) = self.args(mc.args());
                            (args, spans, Some(name_of(mc.name())))
                        }
                        _ => (Vec::new(), Vec::new(), None),
                    };
                    hir::Expr::Call {
                        base: Box::new(acc),
                        args,
                        method,
                        span: rebalance(span_of(suffix), self.src),
                        arg_spans,
                    }
                }
                ast::Suffix::Index(ast::Index::Brackets { expression, .. }) => hir::Expr::Index {
                    base: Box::new(acc),
                    key: Some(Box::new(self.expr(expression))),
                    name: None,
                    name_span: None,
                },
                ast::Suffix::Index(ast::Index::Dot { name, .. }) => hir::Expr::Index {
                    base: Box::new(acc),
                    key: None,
                    name: Some(name_of(name)),
                    name_span: Some(span_of(name)),
                },
                ast::Suffix::Index(_) => hir::Expr::Index {
                    base: Box::new(acc),
                    key: None,
                    name: None,
                    name_span: None,
                },
                // TypeInstantiation (`f<Types.Foo>()`) — no value reads,
                // but the generic arguments are type reads.
                _ => {
                    self.type_names(suffix);
                    acc
                }
            };
        }
        acc
    }

    /// Lower call arguments, keeping a span per argument. The sugar forms
    /// `f "str"` and `f {t}` are one-argument calls and must count as such
    /// for arity checking.
    fn args(&mut self, args: &ast::FunctionArgs) -> (Vec<hir::Expr>, Vec<hir::Span>) {
        match args {
            ast::FunctionArgs::Parentheses { arguments, .. } => {
                arguments
                    .iter()
                    .map(|e| (self.expr(e), rebalance(span_of(e), self.src)))
                    .unzip()
            }
            ast::FunctionArgs::TableConstructor(tc) => {
                let span = rebalance(span_of(tc), self.src);
                (vec![self.table(tc)], vec![span])
            }
            ast::FunctionArgs::String(tok) => {
                self.note_string(tok);
                let span = span_of(tok);
                (
                    vec![hir::Expr::Literal {
                        prim: hir::Prim::String,
                        span: span.clone(),
                        text: Some(string_content(tok)),
                    }],
                    vec![span],
                )
            }
            _ => (Vec::new(), Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    // full_moon range() quirk, 2.x and 3.0 (see `rebalance`): a node whose final
    // token lives in an EARLIER struct field — Index::Brackets declares its
    // ContainedSpan before the key expression — reports a range that stops
    // short of the closing delimiter. These pin the lowered spans covering
    // the full written form.

    #[test]
    fn parse_error_carries_span_and_note() {
        let src = "local local = 1\n";
        let errs = crate::parse(src).unwrap_err();
        let e = &errs[0];
        assert!(
            e.message.contains("unexpected token `local`"),
            "message: {}",
            e.message
        );
        let span = e.span.clone().expect("parse error should carry a span");
        assert_eq!(&src[span], "local");
        assert!(e.note.as_deref().is_some_and(|n| !n.is_empty()));
        assert!(e.opener.is_none(), "no block is open here");
    }

    /// (kind, keyword text) of the block the matcher blames, if any.
    fn opener_of(src: &str) -> Option<(&'static str, String)> {
        let errs = crate::parse(src).unwrap_err();
        let e = &errs[0];
        e.opener.as_ref().map(|o| (o.kind, src[o.span.clone()].to_string()))
    }

    #[test]
    fn missing_end_names_the_innermost_open_block() {
        // The inner `if` is closed; its `end` must not be mistaken for the
        // function's. The function is what stays open.
        let src = "local function f()\n\tif x then\n\t\treturn 1\n\tend\nreturn f\n";
        assert_eq!(opener_of(src), Some(("function", "function".to_string())));
        let errs = crate::parse(src).unwrap_err();
        assert_eq!(errs[0].message, "unexpected end of file");
        assert_eq!(errs[0].opener.as_ref().unwrap().span, 6..14);
    }

    #[test]
    fn loop_headers_own_their_do() {
        // `for ... do` and `while ... do` are ONE opener each, closed by one
        // `end`; the `do` must not be pushed a second time.
        let src = "for i = 1, 2 do\nend\nwhile true do\nend\nif x then\n";
        assert_eq!(opener_of(src), Some(("if", "if".to_string())));
        let src = "do\n\tlocal a = 1\nend\nfor i = 1, 2 do\n";
        assert_eq!(opener_of(src), Some(("for", "for".to_string())));
    }

    #[test]
    fn repeat_pairs_with_until() {
        let src = "repeat\n\tx = x + 1\nuntil x > 3\nwhile x do\n";
        assert_eq!(opener_of(src), Some(("while", "while".to_string())));
        let src = "local x = 0\nrepeat\n\tx = x + 1\n";
        assert_eq!(opener_of(src), Some(("repeat", "repeat".to_string())));
    }

    #[test]
    fn if_expressions_are_not_blocks() {
        // `if c then a else b` in expression position has no `end`. Counted
        // as a block, an `if` expression in a table field inside a function
        // makes the matcher eat that function's `end` and blame the wrong
        // function far above it.
        let src = "local x = if a then 1 else 2\n\
                   local function closed()\n\
                   \tlocal y = if b then 3 elseif c then 4 else 5\n\
                   \tlocal z = f(if d then if e then 6 else 7 else 8, if g then 9 else 10)\n\
                   \tlocal t = { k = if h then 11 else 12 }\n\
                   \treturn if y then y else z + t.k\n\
                   end\n\
                   local function open()\n\
                   \tlocal w = if a then 1 else 2\n\
                   \tif w then\n\
                   \t\treturn w\n\
                   \tend\n\
                   return open\n";
        assert_eq!(opener_of(src), Some(("function", "function".to_string())));
        let errs = crate::parse(src).unwrap_err();
        let span = errs[0].opener.as_ref().unwrap().span.clone();
        assert!(src[span.start..].starts_with("function open"), "blamed the wrong function");
    }

    #[test]
    fn indent_hint_names_the_block_the_end_was_not_meant_for() {
        // An `else` branch left open. The
        // `end` at indent 1 closes the inner `if` (indent 2) syntactically,
        // but lines up with the outer `if` — so the inner `if` is the one
        // missing its `end`.
        let src = "local function migrate(data)\n\
                   \tif data.version < 3 then\n\
                   \t\tif not data.ok then\n\
                   \t\t\treturn\n\
                   \t\telse\n\
                   \t\tdata.version = 3\n\
                   \tend\n\
                   \treturn data\n\
                   end\n";
        let errs = crate::parse(src).unwrap_err();
        assert!(src[errs[0].opener.as_ref().unwrap().span.start..].starts_with("function"));
        let h = errs[0].hint.as_ref().expect("an indentation hint");
        assert_eq!(h.culprit.kind, "if");
        assert!(src[h.culprit.span.start..].starts_with("if not data.ok"));
        assert_eq!(h.closer.kind, "end");
        assert_eq!(h.closer.span.start, src.find("\tend\n").unwrap() + 1);
        let up = h.lines_up_with.as_ref().expect("the outer if lines up");
        assert!(src[up.span.start..].starts_with("if data.version"));
    }

    #[test]
    fn no_hint_when_indentation_agrees_with_the_pairing() {
        let src = "local function f()\n\tif x then\n\t\treturn 1\n\tend\nreturn f\n";
        let errs = crate::parse(src).unwrap_err();
        assert!(errs[0].opener.is_some());
        assert!(errs[0].hint.is_none());
    }

    #[test]
    fn hint_can_lack_a_lining_up_block() {
        let src = "local function f()\n\t\tif x then\n\t\t\treturn 1\n\tend\nreturn f\n";
        let errs = crate::parse(src).unwrap_err();
        let h = errs[0].hint.as_ref().expect("shallower end still hints");
        assert_eq!(h.culprit.kind, "if");
        assert!(h.lines_up_with.is_none());
    }

    #[test]
    fn tabs_versus_spaces_never_hint() {
        // The closer's whitespace is not a prefix of the opener's: nothing
        // to compare, so no claim.
        let src = "local function f()\n\tif x then\n  end\nreturn f\n";
        let errs = crate::parse(src).unwrap_err();
        assert!(errs[0].hint.is_none());
    }

    #[test]
    fn extra_end_has_no_opener() {
        let src = "local x = 1\nend\n";
        assert_eq!(opener_of(src), None);
    }

    #[test]
    fn ty_any_flag_marks_any_and_unions_containing_it() {
        let m = crate::parse(
            "local a: any = 1\nlocal b: number? = 1\nlocal c: any? = 1\nlocal d: string | any = 1\nlocal e: string | nil = 1\nlocal f: (any) = 1\n",
        )
        .unwrap();
        let tys: Vec<(bool, bool)> = m.bodies[0]
            .block
            .stmts
            .iter()
            .map(|s| match &s.kind {
                rime_hir::StmtKind::Local { decls, .. } => {
                    let t = decls[0].ty.as_ref().unwrap();
                    (t.admits_nil, t.any)
                }
                _ => panic!("expected a local"),
            })
            .collect();
        // (admits_nil, any) per declaration, in source order.
        assert_eq!(
            tys,
            vec![
                (true, true),
                (true, false),
                (true, true),
                (true, true),
                (true, false),
                (true, true),
            ]
        );
    }

    /// The names of the `local`s a file's written types leave with unknown
    /// members, and whether its final `return` casts to such a type.
    fn unknown_members_of(src: &str) -> (Vec<String>, bool) {
        let m = crate::parse(src).unwrap();
        let block = &m.bodies[0].block;
        let mut names = Vec::new();
        for s in &block.stmts {
            if let rime_hir::StmtKind::Local { decls, .. } = &s.kind {
                names.extend(
                    decls.iter().filter(|d| m.declares_unknown_members(d)).map(|d| d.name.clone()),
                );
            }
        }
        let returned = matches!(
            &block.terminator,
            Some(rime_hir::Terminator::Return { span, .. }) if m.returns_unknown_members(span)
        );
        (names, returned)
    }

    #[test]
    fn unknown_members_from_annotations_and_casts() {
        let (names, returned) = unknown_members_of(
            "type Late = Alias\n\
             local a: { [string]: number } = {}\n\
             local b: { [string]: any } = {}\n\
             local c: Late = {}\n\
             local d: Map<number>? = {}\n\
             local e = {} :: Alias\n\
             local f = ({} :: Map<string>)\n\
             local g: { name: string, [string]: number } & Closed = {}\n\
             local h, i = {}, {} :: Alias | any\n\
             type Alias = { [string]: number }\n\
             type Map<V> = { [string]: V }\n\
             type Closed = { name: string }\n\
             return a :: Alias\n",
        );
        assert_eq!(names, ["a", "b", "c", "d", "e", "f", "g", "i"]);
        assert!(returned);
    }

    #[test]
    fn listed_members_stay_known() {
        let (names, returned) = unknown_members_of(
            "type Closed = { name: string, inner: { [string]: number } }\n\
             type Loop = Loop\n\
             local a: Closed = {}\n\
             local b: { [number]: string } = {}\n\
             local c: { string } = {}\n\
             local d: Loop = {}\n\
             local e: Other.Map = {}\n\
             local f: NotDeclaredHere = {}\n\
             local g = ({} :: Open) :: Closed\n\
             local h = {}\n\
             type Open = { [string]: number }\n\
             return a :: Closed\n",
        );
        assert_eq!(names, Vec::<String>::new());
        assert!(!returned);
    }

    #[test]
    fn any_alone_leaves_members_known() {
        let (names, returned) = unknown_members_of(
            "type Loose = any\n\
             local a: any = {}\n\
             local b = {} :: any\n\
             local c: any? = {}\n\
             local d: Loose | string = {}\n\
             local e: (any) = {}\n\
             return a :: any\n",
        );
        assert_eq!(names, Vec::<String>::new());
        assert!(!returned);
    }

    #[test]
    fn assign_stmt_span_includes_trailing_bracket() {
        let src = "local t = {}\nt.x = t[1]\nreturn t\n";
        let module = crate::parse(src).unwrap();
        let stmt = &module.body(module.root).block.stmts[1];
        assert_eq!(&src[stmt.span.clone()], "t.x = t[1]");
    }

    #[test]
    fn arg_span_includes_trailing_bracket() {
        let src = "local t = {}\nprint(t[1])\nreturn t\n";
        let module = crate::parse(src).unwrap();
        let stmt = &module.body(module.root).block.stmts[1];
        let rime_hir::StmtKind::Call(rime_hir::Expr::Call { arg_spans, .. }) = &stmt.kind
        else {
            panic!("expected call stmt, got {:?}", stmt.kind);
        };
        assert_eq!(&src[arg_spans[0].clone()], "t[1]");
    }

    // --- `const` (Luau RFC "const keyword", parsed by full_moon >= 2.2) -----

    #[test]
    fn const_assignment_lowers_to_a_const_local() {
        let src = "const LIMIT = 10\nconst a: number, b = 1, 2\nlocal c = LIMIT + a + b\nreturn c\n";
        let m = crate::parse(src).unwrap();
        let stmts = &m.body(m.root).block.stmts;
        assert_eq!(stmts.len(), 3);

        let rime_hir::StmtKind::Local { decls, init } = &stmts[0].kind else {
            panic!("`const LIMIT = 10` must lower to a Local, got {:?}", stmts[0].kind);
        };
        assert_eq!(init.len(), 1);
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].name, "LIMIT");
        assert_eq!(&src[decls[0].span.clone()], "LIMIT");
        assert!(decls[0].is_const);
        assert_eq!(&src[stmts[0].span.clone()], "const LIMIT = 10");

        let rime_hir::StmtKind::Local { decls, init } = &stmts[1].kind else {
            panic!("a typed multi-name const must lower to a Local, got {:?}", stmts[1].kind);
        };
        assert_eq!(init.len(), 2);
        let names: Vec<(&str, bool)> = decls.iter().map(|d| (d.name.as_str(), d.is_const)).collect();
        assert_eq!(names, vec![("a", true), ("b", true)]);
        assert_eq!(decls[0].ty.as_ref().map(|t| t.rendered.as_str()), Some("number"));
        assert!(decls[1].ty.is_none());

        let rime_hir::StmtKind::Local { decls, .. } = &stmts[2].kind else {
            panic!("expected a local");
        };
        assert!(!decls[0].is_const, "a plain `local` is not const");
    }

    #[test]
    fn const_function_lowers_to_a_const_local_function() {
        let src = "const function add(a, b)\n\treturn a + b\nend\nreturn add(1, 2)\n";
        let m = crate::parse(src).unwrap();
        let stmts = &m.body(m.root).block.stmts;
        let rime_hir::StmtKind::LocalFunction { decl, body } = &stmts[0].kind else {
            panic!("`const function` must lower to a LocalFunction, got {:?}", stmts[0].kind);
        };
        assert_eq!(decl.name, "add");
        assert_eq!(&src[decl.span.clone()], "add");
        assert!(decl.is_const);
        let body = m.body(*body);
        let params: Vec<(&str, bool)> =
            body.params.iter().map(|p| (p.name.as_str(), p.is_const)).collect();
        assert_eq!(params, vec![("a", false), ("b", false)]);
        assert!(body.block.terminator.is_some(), "the function body is lowered, not dropped");
    }

    #[test]
    fn local_function_and_loop_variables_are_not_const() {
        let src = "local function f() end\nfor i = 1, 2 do f(i) end\nfor k, v in pairs({}) do f(k, v) end\n";
        let m = crate::parse(src).unwrap();
        let stmts = &m.body(m.root).block.stmts;
        let rime_hir::StmtKind::LocalFunction { decl, .. } = &stmts[0].kind else {
            panic!("expected a local function");
        };
        assert!(!decl.is_const);
        let rime_hir::StmtKind::NumericFor { var, .. } = &stmts[1].kind else {
            panic!("expected a numeric for");
        };
        assert!(!var.is_const);
        let rime_hir::StmtKind::GenericFor { vars, .. } = &stmts[2].kind else {
            panic!("expected a generic for");
        };
        assert!(vars.iter().all(|v| !v.is_const));
    }

    #[test]
    fn const_is_still_an_ordinary_name() {
        // `const` is a contextual keyword: code that already uses it as a
        // variable keeps its meaning.
        let src = "local const = { n = 1 }\nconst.n = 2\nconst = { n = 3 }\nprint(const)\nreturn const\n";
        let m = crate::parse(src).unwrap();
        let stmts = &m.body(m.root).block.stmts;
        let rime_hir::StmtKind::Local { decls, .. } = &stmts[0].kind else {
            panic!("expected a local, got {:?}", stmts[0].kind);
        };
        assert_eq!(decls[0].name, "const");
        assert!(!decls[0].is_const);
        assert!(matches!(
            &stmts[1].kind,
            rime_hir::StmtKind::Assign { targets, .. }
                if matches!(targets[0], rime_hir::Target::Projection(_))
        ));
        assert!(matches!(
            &stmts[2].kind,
            rime_hir::StmtKind::Assign { targets, .. }
                if matches!(&targets[0], rime_hir::Target::Name { name, .. } if name == "const")
        ));
        assert!(matches!(&stmts[3].kind, rime_hir::StmtKind::Call(_)));
    }

    /// The message and the flagged source text of the one error a file with
    /// a condition binding must produce.
    fn binding_error(src: &str) -> (String, String) {
        let errs = match crate::parse(src) {
            Err(errs) => errs,
            Ok(_) => panic!("a condition binding must not lower silently:\n{src}"),
        };
        assert_eq!(errs.len(), 1, "one error per binding: {errs:?}");
        let span = errs[0].span.clone().expect("the binding keyword's span");
        (errs[0].message.clone(), src[span].to_string())
    }

    #[test]
    fn condition_bindings_are_reported_not_dropped() {
        // full_moon 3.0 parses `if local x = e then` (experimental Luau,
        // behind a flag upstream). The HIR has no place for a binding that
        // lives in a condition, and dropping it would report every use of
        // `x` as undefined — so the file is refused with a message that
        // says exactly that.
        let (message, at) = binding_error("local t = {}\nif local v = t.x then\n\tprint(v)\nend\n");
        assert!(message.contains("`if local`"), "message: {message}");
        assert_eq!(at, "local");

        let (message, at) = binding_error("local t = {}\nif const v = t.x then\n\tprint(v)\nend\n");
        assert!(message.contains("`if const`"), "message: {message}");
        assert_eq!(at, "const");

        let (message, at) = binding_error(
            "local t = {}\nif t.a then\n\tprint(1)\nelseif local v = t.x then\n\tprint(v)\nend\n",
        );
        assert!(message.contains("`elseif local`"), "message: {message}");
        assert_eq!(at, "local");

        let (message, at) =
            binding_error("local t = {}\nlocal n = if local v = t.x then v else 0\nreturn n\n");
        assert!(message.contains("`if local`"), "message: {message}");
        assert_eq!(at, "local");

        let (message, at) = binding_error(
            "local t = {}\nlocal n = if t.a then 1 elseif const v = t.x then v else 0\nreturn n\n",
        );
        assert!(message.contains("`elseif const`"), "message: {message}");
        assert_eq!(at, "const");
    }

    #[test]
    fn condition_binding_inside_a_function_is_found() {
        let (message, _) = binding_error(
            "local function f(t)\n\treturn function()\n\t\tif local v = t.x then\n\t\t\treturn v\n\t\tend\n\t\treturn nil\n\tend\nend\nreturn f\n",
        );
        assert!(message.contains("`if local`"), "message: {message}");
    }

    /// The `...` reads of each body, as source text, in body order (root
    /// first, then functions in source order).
    fn vararg_reads(src: &str) -> Vec<(bool, Vec<&str>)> {
        let module = crate::parse(src).unwrap();
        module
            .bodies
            .iter()
            .map(|b| {
                (
                    b.has_varargs,
                    b.notes.vararg_reads.iter().map(|s| &src[s.clone()]).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn vararg_reads_are_recorded_on_the_body_that_reads_them() {
        // `...` lowers to a leaf, so the read itself is all that survives:
        // one span per read, on the body it is written in — the file, a
        // function without `...`, a function with it. Whether a read is
        // legal is `has_varargs` on the same body; lowering does not judge.
        let src = "\
local a = ...
local function plain()
\treturn ...
end
local function variadic(...)
\treturn select(\"#\", ...), { ... }
end
return plain, variadic, a
";
        assert_eq!(
            vararg_reads(src),
            vec![
                (true, vec!["..."]),
                (false, vec!["..."]),
                (true, vec!["...", "..."]),
            ]
        );
        let module = crate::parse(src).unwrap();
        let plain = &module.bodies[1].notes.vararg_reads[0];
        assert_eq!(plain.start, src.find("return ...").unwrap() + "return ".len());
    }

    #[test]
    fn a_nested_function_does_not_lend_its_vararg_reads_to_the_outer_one() {
        let src = "\
local function outer(...)
\treturn function()
\t\treturn ...
\tend
end
return outer
";
        assert_eq!(
            vararg_reads(src),
            vec![(true, vec![]), (true, vec![]), (false, vec!["..."])]
        );
    }

    #[test]
    fn function_attributes_are_kept_on_the_body_they_decorate() {
        // Every function form takes attributes: a declaration, a method, a
        // local function, a const function and a function expression. The
        // names are kept as written, known to Luau or not: judging them is
        // the checker's job.
        let src = "\
local M = {}
@native function M.a() end
@checked @deprecated function M:b() end
@native local function c() end
@foo const function d() end
local e = @bar @native function() end
local function plain() end
return M, c, d, e, plain
";
        let module = crate::parse(src).unwrap();
        let attrs: Vec<Vec<(&str, &str)>> = module
            .bodies
            .iter()
            .map(|b| {
                b.attributes
                    .iter()
                    .map(|a| (a.name.as_str(), &src[a.span.clone()]))
                    .collect()
            })
            .collect();
        assert_eq!(
            attrs,
            vec![
                vec![],
                vec![("native", "@native")],
                vec![("checked", "@checked"), ("deprecated", "@deprecated")],
                vec![("native", "@native")],
                vec![("foo", "@foo")],
                vec![("bar", "@bar"), ("native", "@native")],
                vec![],
            ]
        );
    }

    #[test]
    fn a_variadic_parameter_or_type_is_not_a_vararg_read() {
        // `...` in a parameter list, a parameter annotation, a function
        // type or a generic pack is not a value read.
        let src = "\
type Callback = (...any) -> ...any
local function f(...: number): ...string
\treturn
end
local function g<T...>(...: T...) end
return f, g
";
        for (_, reads) in vararg_reads(src) {
            assert!(reads.is_empty(), "{reads:?}");
        }
    }
    /// (keyword, parenthesised text, what the innermost pair holds) of each
    /// recorded condition, per body.
    fn paren_conditions(src: &str) -> Vec<Vec<(&'static str, &str, &str)>> {
        let module = crate::parse(src).unwrap();
        module
            .bodies
            .iter()
            .map(|b| {
                b.notes
                    .paren_conditions
                    .iter()
                    .map(|p| (p.keyword, &src[p.span.clone()], &src[p.inner.clone()]))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_condition_wholly_in_parentheses_is_recorded() {
        // Parentheses lower to the expression they hold, so the fact that a
        // condition was wrapped survives only as a note on the body.
        let src = "\
local x, t = 1, {}
if (x) then elseif (t[x]) then end
while ( x == 1 ) do break end
repeat until ((x))
local function f()
	if (x) then end
end
return f
";
        assert_eq!(
            paren_conditions(src),
            vec![
                vec![
                    ("if", "(x)", "x"),
                    ("elseif", "(t[x])", "t[x]"),
                    ("while", "( x == 1 )", " x == 1 "),
                    ("until", "((x))", "x"),
                ],
                vec![("if", "(x)", "x")],
            ]
        );
    }

    #[test]
    fn a_condition_only_partly_in_parentheses_is_not_recorded() {
        let src = "\
local x, y = 1, 2
if (x) and (y) then end
if not (x) then end
if (print)(x) then end
if (x) :: any then end
while (x) == 1 do break end
local a = if (x) then 1 elseif (y) then 2 else 3
local b = (x)
return a, b
";
        assert_eq!(paren_conditions(src), vec![vec![]]);
    }
    #[test]
    fn every_string_token_is_noted_on_its_body() {
        // The tree keeps what a string says, not how it was escaped, and
        // keeps nothing of a backtick string's text or a type's.
        let src = r#"local a = "one" .. 'two' .. [[three]]
local b = print"four"
local c = `five {a} six {b} seven`
type T = "eight" | 'nine'
local function f()
	return `ten`, ("eleven") :: "twelve"
end
return a, b, c, f
"#;
        let module = crate::parse(src).unwrap();
        let noted: Vec<Vec<&str>> = module
            .bodies
            .iter()
            .map(|b| {
                let mut spans = b.notes.string_tokens.clone();
                spans.sort_by_key(|s| s.start);
                spans.dedup();
                spans.iter().map(|s| &src[s.clone()]).collect()
            })
            .collect();
        assert_eq!(
            noted,
            vec![
                vec![
                    "\"one\"", "'two'", "[[three]]", "\"four\"", "`five {", "} six {", "} seven`",
                    "\"eight\"", "'nine'",
                ],
                vec!["`ten`", "\"eleven\"", "\"twelve\""],
            ]
        );
    }

    /// (key, whole field) of each recorded table field, per body.
    fn table_fields(src: &str) -> Vec<Vec<(&str, &str)>> {
        let module = crate::parse(src).unwrap();
        module
            .bodies
            .iter()
            .map(|b| {
                b.notes
                    .table_fields
                    .iter()
                    .map(|f| (&src[f.key.clone()], &src[f.extent.clone()]))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn every_keyed_table_field_is_noted_with_its_separator() {
        // The tree keeps a field's name and value. Where the field ends —
        // and whether a `,` or `;` follows — survives only as a note.
        let src = "\
local t = {
	first = 1,
	second = f(2, 3) ; -- trailing comment
	[\"third\"] = { inner = true },
	4,
	[5] = t[1],
	last = 6
}
local function g()
	return { only = `x{t}` }
end
return t, g
";
        assert_eq!(
            table_fields(src),
            vec![
                vec![
                    ("first", "first = 1,"),
                    ("second", "second = f(2, 3) ;"),
                    ("\"third\"", "[\"third\"] = { inner = true },"),
                    ("inner", "inner = true"),
                    ("5", "[5] = t[1],"),
                    ("last", "last = 6"),
                ],
                vec![("only", "only = `x{t}`")],
            ]
        );
    }

    #[test]
    fn every_semicolon_that_ends_a_statement_is_noted() {
        // The tree keeps the statement, not the `;` after it.
        let src = "\
local a = 1;
a = 2 ; -- trailing comment
print(a)
do
	a += 1; a += 2;
end
local function g()
	a = 3;
	return a;
end
g() -- far
;
return a
";
        let module = crate::parse(src).unwrap();
        let noted: Vec<Vec<(&str, &str)>> = module
            .bodies
            .iter()
            .map(|b| {
                b.notes
                    .semicolons
                    .iter()
                    .map(|s| (&src[s.stmt.clone()], &src[s.semicolon.clone()]))
                    .collect()
            })
            .collect();
        assert_eq!(
            noted,
            vec![
                vec![
                    ("local a = 1", ";"),
                    ("a = 2", ";"),
                    ("a += 1", ";"),
                    ("a += 2", ";"),
                    ("g()", ";"),
                ],
                vec![("a = 3", ";")],
            ]
        );
        // Each is the statement's own span, so a statement finds its `;`.
        let root = module.body(module.root);
        let last = root.block.stmts.last().unwrap();
        let found = root.notes.semicolons.iter().find(|s| s.stmt == last.span).expect("noted");
        assert_eq!(&src[last.span.start..found.semicolon.end], "g() -- far\n;");
    }

    #[test]
    fn a_named_field_is_found_by_its_name_span() {
        let src = "local t = { alpha = 1, beta = 2 }\nreturn t\n";
        let module = crate::parse(src).unwrap();
        let root = module.body(module.root);
        let rime_hir::StmtKind::Local { init, .. } = &root.block.stmts[0].kind else { panic!() };
        let rime_hir::Expr::Table(fields) = &init[0] else { panic!() };
        let rime_hir::TableField::Named { span, .. } = &fields[1] else { panic!() };
        let noted = root.notes.table_fields.iter().find(|f| f.key == *span).expect("noted");
        assert_eq!(&src[noted.extent.clone()], "beta = 2");
    }
}
