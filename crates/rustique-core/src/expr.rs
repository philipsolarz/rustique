//! Vectorised expressions (`X * 2 + 1`, `X % 3 == 0`), evaluated column-at-a-time with
//! Python semantics (ARCHITECTURE §4.5).
//!
//! An [`Expr`] is an immutable tree over one placeholder, the element `X`. [`eval`] compiles
//! it against the input column into a flat program — static types, folded constants,
//! explicit conversions — and runs that program over cache-sized blocks of the column with
//! one kernel call per node and block. Kernels loop over plain slices (dispatch happens
//! once per node and block, never per element) so LLVM can vectorise them; large columns
//! are split across the rayon pool.
//!
//! # Types
//! Every subexpression has an [`ExprType`]: one of the Python types `bool`, `int` (held as
//! i64), `float` (f64) and `str`, or a fixed-width type (`rs.i8` … `rs.f64`). [`IntMode`]
//! says how the numbers of the input column are typed. The rules are Python's, extended by
//! the semantics of the `rustique` scalar classes:
//! - `bool` arithmetic promotes to `int`, `int` with `float` gives `float`, `/` gives
//!   `float`, and `int ** int` is an `int` unless the exponent is a negative constant.
//! - A fixed-width operand combined with the same type, an `int` or a `bool` keeps its type;
//!   the Python operand must fit it. Fixed-width floats also take `float` operands.
//!   Fixed-width integers do not mix with `float`, and different fixed types do not mix
//!   (`TypeError`). Operator *meaning* stays Python's (`//` floors, `%` takes the divisor's
//!   sign, `/` of integers gives a Python `float`); results that do not fit the type raise
//!   `OverflowError`.
//! - `& | ^ ~` are logical on `bool` (so `~` is `not`, unlike Python's deprecated
//!   `~True == -2`) and bitwise on integers.
//! - Comparisons are exact across all numeric types and produce `bool`; `str` only orders
//!   against `str`.
//!
//! # Errors
//! Evaluation is observably equivalent to evaluating the tree element by element, in order:
//! the reported error is the one Python raises for the first failing element (and, within
//! that element, the first failing operation in evaluation order). Type errors are found
//! statically, so they are also reported for an empty column. [`ExprError::Overflow`] means
//! a result has no native representation — a Python `int` beyond i64, a complex power, a
//! `str` `%`-format, … — and the caller must recompute element by element with Python
//! objects.

use std::fmt;
use std::sync::Arc;

use crate::column::{Column, Value};
use crate::dtype::DType;

mod exec;
mod kernels;
mod plan;
#[cfg(test)]
mod tests;
mod text;

/// Expressions nested deeper than this are rejected (`RecursionError`), which keeps every
/// recursive walk over a tree (planning, rendering, dropping) far from the stack limit.
pub const MAX_DEPTH: usize = 512;

/// Expressions with more nodes than this are rejected (`ValueError`). Subexpressions are
/// shared, so `e = e + e` repeated `n` times is a tree of `2**(n+1) - 1` nodes in `n + 1`
/// objects: rendering it, comparing it or evaluating it element by element in Python walks
/// every one of them. (Native evaluation plans each shared subexpression once.)
pub const MAX_NODES: u64 = 100_000;

/// Unary operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// `-x`
    Neg,
    /// `+x`
    Pos,
    /// `abs(x)`
    Abs,
    /// `~x` (logical `not` on `bool`)
    Invert,
    /// `x.is_nan()`: `False` for integers.
    IsNan,
}

/// Binary operations, in Python operator terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    /// `/`
    TrueDiv,
    /// `//`
    FloorDiv,
    /// `%`
    Mod,
    /// `**`
    Pow,
    /// `<<`
    LShift,
    /// `>>`
    RShift,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinaryOp {
    /// The Python operator symbol.
    pub const fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::TrueDiv => "/",
            BinaryOp::FloorDiv => "//",
            BinaryOp::Mod => "%",
            BinaryOp::Pow => "**",
            BinaryOp::LShift => "<<",
            BinaryOp::RShift => ">>",
            BinaryOp::BitAnd => "&",
            BinaryOp::BitOr => "|",
            BinaryOp::BitXor => "^",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
        }
    }

    pub const fn is_comparison(self) -> bool {
        matches!(self, BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge)
    }
}

/// The type of a (sub)expression's values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExprType {
    /// Python `bool`.
    Bool,
    /// Python `int`, natively held as i64.
    Int,
    /// Python `float`.
    Float,
    /// Python `str`.
    Str,
    /// A fixed-width numeric type (`rs.i8` … `rs.f64`); never `Bool` or `Str`.
    Fixed(DType),
}

impl ExprType {
    /// How the elements of a `dtype` column are typed under `mode`.
    pub fn of_column(dtype: DType, mode: IntMode) -> ExprType {
        match dtype {
            DType::Bool => ExprType::Bool,
            DType::Str => ExprType::Str,
            d if mode == IntMode::Fixed => ExprType::Fixed(d),
            d if d.is_integer() => ExprType::Int,
            _ => ExprType::Float,
        }
    }

    /// The dtype of a column holding values of this type.
    pub const fn dtype(self) -> DType {
        match self {
            ExprType::Bool => DType::Bool,
            ExprType::Int => DType::I64,
            ExprType::Float => DType::F64,
            ExprType::Str => DType::Str,
            ExprType::Fixed(d) => d,
        }
    }

    /// The Python type name (`"int"`, `"i8"`, …) used in error messages and reprs.
    pub const fn name(self) -> &'static str {
        match self {
            ExprType::Bool => "bool",
            ExprType::Int => "int",
            ExprType::Float => "float",
            ExprType::Str => "str",
            ExprType::Fixed(d) => d.name(),
        }
    }

    /// `Fixed(Bool)` and `Fixed(Str)` mean `Bool` and `Str`.
    pub(crate) const fn normalized(self) -> ExprType {
        match self {
            ExprType::Fixed(DType::Bool) => ExprType::Bool,
            ExprType::Fixed(DType::Str) => ExprType::Str,
            t => t,
        }
    }

    pub(crate) const fn is_float(self) -> bool {
        matches!(self, ExprType::Float | ExprType::Fixed(DType::F32 | DType::F64))
    }
}

impl fmt::Display for ExprType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How the integers (and floats) of an input column are typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntMode {
    /// Python numbers: integer columns hold Python `int`s, float columns Python `float`s.
    /// Integer results that leave i64 report [`ExprError::Overflow`].
    Python,
    /// Fixed-width numbers (`rs.i8` … `rs.f64` data): results keep the column's type and
    /// overflow raises `OverflowError`.
    Fixed,
}

/// The Python exception class of an [`ExprError::Raise`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    TypeError,
    ValueError,
    ZeroDivisionError,
    OverflowError,
    /// `OverflowError(errno.ERANGE, strerror(ERANGE))`, as raised by float `**` on overflow.
    RangeError,
    RecursionError,
    /// An internal invariant was violated (a bug in rustique).
    SystemError,
}

/// Why an expression could not be evaluated natively.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprError {
    /// A result has no native representation (a Python `int` outside i64, a complex number,
    /// a `str` `%`-format, …): recompute element by element with Python objects.
    Overflow,
    /// Python raises `kind(message)`.
    Raise { kind: ErrorKind, message: String },
}

impl ExprError {
    pub(crate) fn raise(kind: ErrorKind, message: impl Into<String>) -> ExprError {
        ExprError::Raise { kind, message: message.into() }
    }
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprError::Overflow => f.write_str("result has no native representation"),
            ExprError::Raise { kind, message } => write!(f, "{kind:?}: {message}"),
        }
    }
}

impl std::error::Error for ExprError {}

/// An immutable expression tree over the element placeholder `X`.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    node: Node,
    depth: usize,
    /// Nodes of the tree, shared subexpressions counted once per use (saturating).
    size: u64,
    has_element: bool,
}

/// One node of an [`Expr`].
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// The placeholder `X`.
    Element,
    /// A Python literal: integer values (of any width) are Python `int`s, `F32`/`F64` are
    /// Python `float`s. Use [`Expr::cast`] for a fixed-width constant.
    Const(Value),
    Unary(UnaryOp, Arc<Expr>),
    Binary(BinaryOp, Arc<Expr>, Arc<Expr>),
    /// `x.cast(t)`: `int(x)`, `float(x)`, `bool(x)`, or `rs.T(x)` for numbers (truncating floats).
    Cast(ExprType, Arc<Expr>),
}

impl Expr {
    fn new(node: Node) -> Arc<Expr> {
        let (depth, size, has_element) = match &node {
            Node::Element => (1, 1, true),
            Node::Const(_) => (1, 1, false),
            Node::Unary(_, a) | Node::Cast(_, a) => (a.depth + 1, a.size.saturating_add(1), a.has_element),
            Node::Binary(_, a, b) => (
                a.depth.max(b.depth) + 1,
                a.size.saturating_add(b.size).saturating_add(1),
                a.has_element || b.has_element,
            ),
        };
        Arc::new(Expr { node, depth, size, has_element })
    }

    /// The placeholder `X`.
    pub fn element() -> Arc<Expr> {
        Expr::new(Node::Element)
    }

    pub fn constant(value: impl Into<Value>) -> Arc<Expr> {
        Expr::new(Node::Const(value.into()))
    }

    pub fn unary(op: UnaryOp, arg: Arc<Expr>) -> Arc<Expr> {
        Expr::new(Node::Unary(op, arg))
    }

    pub fn binary(op: BinaryOp, lhs: Arc<Expr>, rhs: Arc<Expr>) -> Arc<Expr> {
        Expr::new(Node::Binary(op, lhs, rhs))
    }

    pub fn cast(to: ExprType, arg: Arc<Expr>) -> Arc<Expr> {
        Expr::new(Node::Cast(to.normalized(), arg))
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    /// Nesting depth (`X` and constants have depth 1).
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Number of nodes, counting a shared subexpression once per use (saturating).
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The error for an expression beyond [`MAX_DEPTH`] or [`MAX_NODES`], if it is one.
    pub fn limit_error(&self) -> Option<ExprError> {
        if self.depth > MAX_DEPTH {
            Some(ExprError::raise(
                ErrorKind::RecursionError,
                format!("expression nesting exceeds the maximum depth of {MAX_DEPTH}"),
            ))
        } else if self.size > MAX_NODES {
            Some(ExprError::raise(
                ErrorKind::ValueError,
                format!("expression has more than {MAX_NODES} nodes (a subexpression used n times counts n times)"),
            ))
        } else {
            None
        }
    }

    /// Whether the tree mentions `X` (otherwise it is a constant expression).
    pub fn has_element(&self) -> bool {
        self.has_element
    }

    /// The static result type for input elements of type `input`, or the error every
    /// element would raise (type errors, failing constant subexpressions).
    pub fn result_type(&self, input: ExprType) -> Result<ExprType, ExprError> {
        if let Some(error) = self.limit_error() {
            return Err(error);
        }
        let input = input.normalized();
        let program = plan::plan(self, input.dtype(), input);
        match (program.stop, program.root) {
            (None, Some(root)) => Ok(program.nodes[root].ty),
            (Some(failure), _) => Err(failure.error),
            (None, None) => Err(exec::bug()),
        }
    }

    /// Render like Python source with full parentheses: `((X * 2) + 1)`, `abs(X)`,
    /// `X.is_nan()`, `(X / 2).cast(int)`. `constant` renders literals.
    pub fn render(&self, constant: &mut dyn FnMut(&Value) -> String) -> String {
        let mut out = String::new();
        self.render_into(&mut out, constant);
        out
    }

    fn render_into(&self, out: &mut String, constant: &mut dyn FnMut(&Value) -> String) {
        match &self.node {
            Node::Element => out.push('X'),
            Node::Const(v) => out.push_str(&constant(v)),
            Node::Unary(UnaryOp::Abs, a) => {
                out.push_str("abs(");
                a.render_into(out, constant);
                out.push(')');
            }
            Node::Unary(UnaryOp::IsNan, a) => {
                a.render_into(out, constant);
                out.push_str(".is_nan()");
            }
            Node::Unary(op, a) => {
                out.push('(');
                out.push_str(match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Pos => "+",
                    _ => "~",
                });
                a.render_into(out, constant);
                out.push(')');
            }
            Node::Binary(op, a, b) => {
                out.push('(');
                a.render_into(out, constant);
                out.push(' ');
                out.push_str(op.symbol());
                out.push(' ');
                b.render_into(out, constant);
                out.push(')');
            }
            Node::Cast(t, a) => {
                a.render_into(out, constant);
                out.push_str(".cast(");
                out.push_str(t.name());
                out.push(')');
            }
        }
    }
}

/// Python-like rendering; literals use Rust formatting (the bindings render Python reprs).
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(&mut |v| text::format_value(v)))
    }
}

/// Evaluate `expr` for every element of `column`.
///
/// The result has the dtype of the expression's [`ExprType`] (comparisons give a `Bool`
/// column, usable as a filter mask). See the module docs for the error contract.
pub fn eval(expr: &Expr, column: &Column, mode: IntMode) -> Result<Column, ExprError> {
    exec::evaluate(expr, column, mode, &exec::Tuning::DEFAULT)
}

/// Python truthiness of the values of `expr`: the mask `lst.filter(expr)` keeps.
pub fn eval_mask(expr: &Expr, column: &Column, mode: IntMode) -> Result<Vec<bool>, ExprError> {
    eval(expr, column, mode).map(into_truthy)
}

/// The elements of `column` for which `expr` is truthy.
pub fn filter(expr: &Expr, column: &Column, mode: IntMode) -> Result<Column, ExprError> {
    let mask = eval_mask(expr, column, mode)?;
    Ok(select(column, &mask))
}

/// The elements of `column` whose `mask` entry is `true` (`mask.len() == column.len()`),
/// compacted in parallel for large columns.
pub fn select(column: &Column, mask: &[bool]) -> Column {
    debug_assert_eq!(column.len(), mask.len());
    crate::map_column!(column, v => select_slice(v, mask))
}

fn select_slice<T: Clone + Send + Sync>(values: &[T], mask: &[bool]) -> Vec<T> {
    let pairs = || values.iter().zip(mask).filter(|(_, keep)| **keep).map(|(x, _)| x.clone());
    if values.len() >= exec::Tuning::DEFAULT.parallel_min {
        use rayon::prelude::*;
        return values.par_iter().zip(mask).filter(|(_, keep)| **keep).map(|(x, _)| x.clone()).collect();
    }
    let mut out = Vec::with_capacity(mask.iter().filter(|keep| **keep).count());
    out.extend(pairs());
    out
}

/// Python truthiness of each element (`0`, `0.0`, `False` and `""` are false; NaN is true).
pub fn truthy(column: &Column) -> Vec<bool> {
    match column {
        Column::Bool(v) => v.clone(),
        Column::Str(v) => v.iter().map(|s| !s.is_empty()).collect(),
        Column::F32(v) => v.iter().map(|&x| x != 0.0).collect(),
        Column::F64(v) => v.iter().map(|&x| x != 0.0).collect(),
        Column::I8(v) => v.iter().map(|&x| x != 0).collect(),
        Column::I16(v) => v.iter().map(|&x| x != 0).collect(),
        Column::I32(v) => v.iter().map(|&x| x != 0).collect(),
        Column::I64(v) => v.iter().map(|&x| x != 0).collect(),
        Column::U8(v) => v.iter().map(|&x| x != 0).collect(),
        Column::U16(v) => v.iter().map(|&x| x != 0).collect(),
        Column::U32(v) => v.iter().map(|&x| x != 0).collect(),
        Column::U64(v) => v.iter().map(|&x| x != 0).collect(),
    }
}

/// [`truthy`], reusing the storage of a `Bool` column.
pub fn into_truthy(column: Column) -> Vec<bool> {
    match column {
        Column::Bool(v) => v,
        other => truthy(&other),
    }
}
