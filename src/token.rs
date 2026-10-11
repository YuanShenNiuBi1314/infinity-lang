//! Token model shared by the lexer, the parser, the macro expander and `infy fmt`.
//!
//! The lexer (§2) produces `Vec<Tok>` plus `Vec<Comment>` plus diagnostics; token trees
//! (§3.9/§6) are built by grouping `()`/`[]`/`{}` runs of `Tok`.

use crate::span::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Punct {
    Dot,        // .
    DotDot,     // ..
    DotDotEq,   // ..=
    Comma,
    Semi,
    Colon,
    ColonColon, // ::
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Bang,       // !
    Eq,         // =
    FatArrow,   // =>
    Arrow,      // ->
    Question,   // ?
    Amp,        // &
    AmpAmp,     // &&
    Pipe,
    PipePipe,   // ||
    Hat,
    Star,
    Percent,
    Plus,
    Minus,
    Lt,
    LtEq,
    Gt,
    GtEq,
    EqEq,       // ==
    BangEq,     // !=
    Shl,
    Shr,
    PlusEq,
    MinusEq,
    StarEq,
    PercentEq,
    AmpEq,
    HatEq,
    PipeEq,
    ShlEq,
    ShrEq,
    Slash,      // /
    SlashEq,    // /=
    At,         // @ (embed prefix marker, only legal before a track lang)
    Hash,       // #
    Dollar,     // $ (macro fragment introducer, §3.9/§6.2)
    Tilde,      // ~ unused; reserved
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kw {
    // declaration
    Module, Package, Import, Pub, Priv, Mod, Event, Fn, Struct, Enum, Union, Trait, Impl, Type,
    Const, Static, Let, Var,
    // link / external
    Extern, Unsafe, Link, Owns, Borrows, Label,
    // types & constants
    U8, U16, U32, U64, U128, I8, I16, I32, I64, I128, F32, F64, BoolTy, Str, Char, Dyn, SelfTy,
    SelfVal, True, False,
    // control flow (plain statements)
    If, Else, While, Loop, For, In, Break, Continue, Return, Match,
    // chain / linkage
    Chain, StartFlag, Goto, Start, Stop, Pause, Resume, Linkage,
    // operation semantics
    As, Is, Where, Move,
    // reserved but unused in v2.0 grammar (§2.4: E-0020 when used as an identifier)
    Use,
    // lexical
    Auto, Mut, Ref, Defer, ConstAssert, MacroRules,
}

impl Kw {
    pub fn text(&self) -> &'static str {
        use Kw::*;
        match self {
            Module => "module", Package => "package", Import => "import", Pub => "pub",
            Priv => "priv", Mod => "mod", Event => "event", Fn => "fn", Struct => "struct",
            Enum => "enum", Union => "union", Trait => "trait", Impl => "impl", Type => "type",
            Const => "const", Static => "static", Let => "let", Var => "var",
            Extern => "extern", Unsafe => "unsafe", Link => "link", Owns => "owns",
            Borrows => "borrows", Label => "label",
            U8 => "u8", U16 => "u16", U32 => "u32", U64 => "u64", U128 => "u128",
            I8 => "i8", I16 => "i16", I32 => "i32", I64 => "i64", I128 => "i128",
            F32 => "f32", F64 => "f64", BoolTy => "bool", Str => "str", Char => "char",
            Dyn => "dyn", SelfTy => "Self", SelfVal => "self", True => "true", False => "false",
            If => "if", Else => "else", While => "while", Loop => "loop", For => "for",
            In => "in", Break => "break", Continue => "continue", Return => "return",
            Match => "match",
            Chain => "chain", StartFlag => "start_flag", Goto => "goto", Start => "start",
            Stop => "stop", Pause => "pause", Resume => "resume", Linkage => "linkage",
            As => "as", Is => "is", Where => "where", Move => "move", Use => "use",
            Auto => "auto", Mut => "mut", Ref => "ref", Defer => "defer",
            ConstAssert => "const_assert", MacroRules => "macro_rules!",
        }
    }

    pub fn from_text(s: &str) -> Option<Kw> {
        // §2.4: reserved words. `macro_rules!` is a single token.
        Some(match s {
            "module" => Module, "package" => Package, "import" => Import, "pub" => Pub,
            "priv" => Priv, "mod" => Mod, "event" => Event, "fn" => Fn, "struct" => Struct,
            "enum" => Enum, "union" => Union, "trait" => Trait, "impl" => Impl, "type" => Type,
            "const" => Const, "static" => Static, "let" => Let, "var" => Var,
            "extern" => Extern, "unsafe" => Unsafe, "link" => Link, "owns" => Owns,
            "borrows" => Borrows, "label" => Label,
            "u8" => U8, "u16" => U16, "u32" => U32, "u64" => U64, "u128" => U128,
            "i8" => I8, "i16" => I16, "i32" => I32, "i64" => I64, "i128" => I128,
            "f32" => F32, "f64" => F64, "bool" => BoolTy, "str" => Str, "char" => Char,
            "dyn" => Dyn, "Self" => SelfTy, "self" => SelfVal, "true" => True, "false" => False,
            "if" => If, "else" => Else, "while" => While, "loop" => Loop, "for" => For,
            "in" => In, "break" => Break, "continue" => Continue, "return" => Return,
            "match" => Match,
            "chain" => Chain, "start_flag" => StartFlag, "goto" => Goto, "start" => Start,
            "stop" => Stop, "pause" => Pause, "resume" => Resume, "linkage" => Linkage,
            "as" => As, "is" => Is, "where" => Where, "move" => Move, "use" => Use,
            "auto" => Auto, "mut" => Mut, "ref" => Ref, "defer" => Defer,
            "const_assert" => ConstAssert, "macro_rules!" => MacroRules,
            _ => return None,
        })
    }

    /// Reserved words that may NOT be used as identifiers (§2.4). All of `Kw` are reserved in
    /// v2.0; `E-0020` covers the ones the grammar never consumes.
    pub fn is_reserved(s: &str) -> bool {
        Kw::from_text(s).is_some()
    }

    pub fn is_prim_type(&self) -> bool {
        matches!(self,
            Kw::U8 | Kw::U16 | Kw::U32 | Kw::U64 | Kw::U128 | Kw::I8 | Kw::I16 | Kw::I32
            | Kw::I64 | Kw::I128 | Kw::F32 | Kw::F64 | Kw::BoolTy | Kw::Str | Kw::Char | Kw::SelfTy)
    }
}

use Kw::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Radix { Dec, Hex, Oct, Bin }

/// A numeric literal, lexed but not yet type-checked (§2.8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntLit {
    /// Digits only: underscores stripped, no prefix, no suffix.
    pub digits: String,
    pub radix: Radix,
    /// Suffix as written (`u32`, `i64`, `usize`, ...), `None` when unsuffixed.
    pub suffix: Option<String>,
    /// `true` when the value fits in `u128`; otherwise sema must report `E-0042`/`E-0043`.
    pub value: Option<u128>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatLit {
    pub mantissa: String,
    pub exp: Option<String>,
    pub suffix: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lit {
    Int(IntLit),
    Float(FloatLit),
    /// `str_lit` / `raw_str_lit` after escape resolution and dedenting: the *value*.
    Str(String),
    ByteStr(Vec<u8>),
    /// `cord_lit` (§2.9): NFC-normalised at lex time for §2.10 comparisons.
    Cord(String),
    Char(char),
    /** `true` / `false` (§2.6). The lexer emits keywords; the parser normalises them into a literal. */
    Bool(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Delim {
    Paren,
    Bracket,
    Brace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Ident(String),
    /// `'a` / `'static` (§2.5)
    Lifetime(String),
    Keyword(Kw),
    Punct(Punct),
    Lit(Lit),
    /// `@py` `@lua` `@c` `@cpp` `@rust` `@zig` `@ts` `@js` `@sh` `@ps1` `@bat` `@cmd` — stored
    /// without the `@`.
    EmbedLang(String),
    /// The `( ... )` argument list of an embed block, kept raw and re-parsed by the parser.
    EmbedArgs(String),
    /// The `{ ... }` body of an embed block: lexically opaque (§2.11). Raw source, no braces.
    EmbedBody(String),
    /// `#[` / `#![` — attributes are re-parsed from the token stream (§3.1/§6.6).
    AttrOpen { inner: bool },
    Eof,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tok {
    pub kind: Kind,
    pub span: Span,
    /// Exact source text of the token (for `infy fmt` and for macro token-tree comparison).
    pub text: String,
}

impl Tok {
    pub fn is_p(&self, p: Punct) -> bool {
        self.kind == Kind::Punct(p)
    }
    pub fn is_kw(&self, k: Kw) -> bool {
        self.kind == Kind::Keyword(k)
    }
    pub fn is_ident(&self, s: &str) -> bool {
        matches!(&self.kind, Kind::Ident(x) if x == s)
    }
    pub fn is_eof(&self) -> bool {
        self.kind == Kind::Eof
    }
    pub fn ident(&self) -> Option<&str> {
        if let Kind::Ident(s) = &self.kind { Some(s) } else { None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentKind {
    Line,   // //
    Block,  // /* */
    Doc,    // ///
}

#[derive(Clone, Debug)]
pub struct Comment {
    pub kind: CommentKind,
    pub span: Span,
    /// Text after the marker, exactly as written.
    pub text: String,
}

/// Token tree used by `macro_rules!` (§3.9/§6.2).
#[derive(Clone, Debug)]
pub enum Tree {
    Leaf(Tok),
    Group(Delim, Vec<Tree>, Span),
}

impl Tree {
    pub fn span(&self) -> Span {
        match self {
            Tree::Leaf(t) => t.span,
            Tree::Group(_, _, s) => *s,
        }
    }

    fn delim_punct(d: Delim, open: bool) -> Punct {
        match (d, open) {
            (Delim::Paren, true) => Punct::LParen,
            (Delim::Paren, false) => Punct::RParen,
            (Delim::Bracket, true) => Punct::LBracket,
            (Delim::Bracket, false) => Punct::RBracket,
            (Delim::Brace, true) => Punct::LBrace,
            (Delim::Brace, false) => Punct::RBrace,
        }
    }

    /// Flattened leaf token stream, group delimiters materialised as puncts. Used by the macro
    /// matcher/expander and by `infy expand`'s byte-stable reprinting (§6.8).
    pub fn flatten(&self, out: &mut Vec<Tok>) {
        match self {
            Tree::Leaf(t) => out.push(t.clone()),
            Tree::Group(d, ts, s) => {
                out.push(Tok {
                    kind: Kind::Punct(Tree::delim_punct(*d, true)),
                    span: *s,
                    text: String::new(),
                });
                for t in ts {
                    t.flatten(out);
                }
                out.push(Tok {
                    kind: Kind::Punct(Tree::delim_punct(*d, false)),
                    span: *s,
                    text: String::new(),
                });
            }
        }
    }

    pub fn leaves(&self) -> Vec<Tok> {
        let mut v = Vec::new();
        self.flatten(&mut v);
        v
    }
}
