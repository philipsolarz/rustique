//! Compiling an [`Expr`] for one input type into a flat post-order [`Program`]: static
//! typing by Python's rules, implicit conversions made explicit nodes, constants folded.
//!
//! Every program node keeps the post-order position (`order`) of the expression node it
//! implements, so failures can be ranked by `(element, order)` — the order in which
//! element-by-element Python evaluation would hit them. A static failure (a type error, or
//! a constant subexpression that raises) ends planning: it applies to every element, and
//! only operations before it can fail first.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::column::{Column, Value};
use crate::dtype::DType;

use super::exec;
use super::{BinaryOp, ErrorKind, Expr, ExprError, ExprType, Node, UnaryOp};

/// A comparison operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    fn of(op: BinaryOp) -> Option<Cmp> {
        Some(match op {
            BinaryOp::Eq => Cmp::Eq,
            BinaryOp::Ne => Cmp::Ne,
            BinaryOp::Lt => Cmp::Lt,
            BinaryOp::Le => Cmp::Le,
            BinaryOp::Gt => Cmp::Gt,
            BinaryOp::Ge => Cmp::Ge,
            _ => return None,
        })
    }

    /// The same test with swapped operands: `a < b` is `b > a`.
    fn flip(self) -> Cmp {
        match self {
            Cmp::Lt => Cmp::Gt,
            Cmp::Le => Cmp::Ge,
            Cmp::Gt => Cmp::Lt,
            Cmp::Ge => Cmp::Le,
            c => c,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Cmp::Eq => "==",
            Cmp::Ne => "!=",
            Cmp::Lt => "<",
            Cmp::Le => "<=",
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
        }
    }

    /// Apply to an ordering (`None`: unordered, e.g. NaN).
    #[inline(always)]
    pub fn test(self, ord: Option<Ordering>) -> bool {
        match self {
            Cmp::Eq => ord == Some(Ordering::Equal),
            Cmp::Ne => ord != Some(Ordering::Equal),
            Cmp::Lt => ord == Some(Ordering::Less),
            Cmp::Le => matches!(ord, Some(Ordering::Less | Ordering::Equal)),
            Cmp::Gt => ord == Some(Ordering::Greater),
            Cmp::Ge => matches!(ord, Some(Ordering::Greater | Ordering::Equal)),
        }
    }
}

/// The first failure, ranked by element index, then evaluation order within the element.
#[derive(Clone, Debug)]
pub(super) struct Failure {
    pub index: usize,
    pub order: u32,
    pub error: ExprError,
}

impl Failure {
    pub fn precedes(&self, other: &Failure) -> bool {
        (self.index, self.order) < (other.index, other.order)
    }
}

/// What a program node computes. Operands are indices of earlier nodes.
#[derive(Clone, Debug)]
pub(super) enum Kind {
    /// The input block (its dtype is the column's).
    Input,
    /// A one-element constant, broadcast. Its column holds an `Int` beyond i64 as `U64`.
    Const(Column),
    /// Numeric conversion to the node type's dtype: exact and range-checked into integers,
    /// rounding into floats. Sources are `bool`, integers or (into floats) floats.
    Convert(usize),
    /// `x.cast(ty)`: `int(x)` truncates floats and parses text, `bool(x)` is truthiness, …
    Cast(usize),
    /// Operand and result share a dtype, except `IsNan` (float → bool).
    Unary(UnaryOp, usize),
    /// Operands share a dtype, which is also the result's, except `/` of integers (→ f64)
    /// and the count of a fixed-width shift (i64 for an `int` count). On `bool` only
    /// `& | ^` (logical).
    Arith(BinaryOp, usize, usize),
    /// Operands share a dtype, or are (i64 | u64, f64) or (u64, i64).
    Compare(Cmp, usize, usize),
    Concat(usize, usize),
    /// `text * count` with an i64 count.
    Repeat(usize, usize),
}

impl Kind {
    /// The operand node indices (first `count` entries of the array).
    pub fn operands(&self) -> ([usize; 2], usize) {
        match *self {
            Kind::Input | Kind::Const(_) => ([0, 0], 0),
            Kind::Convert(a) | Kind::Cast(a) | Kind::Unary(_, a) => ([a, 0], 1),
            Kind::Arith(_, a, b) | Kind::Compare(_, a, b) | Kind::Concat(a, b) | Kind::Repeat(a, b) => {
                ([a, b], 2)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PNode {
    pub kind: Kind,
    pub ty: ExprType,
    /// Post-order position of the expression node this implements.
    pub order: u32,
}

#[derive(Debug)]
pub(super) struct Program {
    /// In evaluation order: operands precede their users.
    pub nodes: Vec<PNode>,
    /// The node holding the result (`None` iff planning stopped).
    pub root: Option<usize>,
    /// The failure every element hits, if planning found one.
    pub stop: Option<Failure>,
}

/// Plan `expr` for input elements stored as `dtype` and typed `ty` (`dtype` may differ
/// from `ty.dtype()`, e.g. an i32 column of Python ints; the input is then converted).
pub(super) fn plan(expr: &Expr, dtype: DType, ty: ExprType) -> Program {
    let mut planner = Planner { nodes: Vec::new(), next_order: 0, input: (dtype, ty), planned: HashMap::new() };
    let result = planner.visit(expr).and_then(|root| {
        // A Python int beyond i64 as the result: only Python can hold it.
        if planner.is_big(root) { Err(fallback(planner.nodes[root].order)) } else { Ok(root) }
    });
    Program { nodes: planner.nodes, root: result.as_ref().ok().copied(), stop: result.err() }
}

type Planned = Result<usize, Failure>;

struct Planner {
    nodes: Vec<PNode>,
    next_order: u32,
    input: (DType, ExprType),
    /// The node implementing each subexpression planned so far, by address: a shared
    /// subexpression is planned (and evaluated) once. Its first use is the one Python
    /// evaluates first, so failures keep their rank.
    planned: HashMap<*const Expr, usize>,
}

fn type_error(order: u32, message: String) -> Failure {
    Failure { index: 0, order, error: ExprError::raise(ErrorKind::TypeError, message) }
}

fn fallback(order: u32) -> Failure {
    Failure { index: 0, order, error: ExprError::Overflow }
}

fn unsupported(op: BinaryOp, lhs: ExprType, rhs: ExprType, order: u32) -> Failure {
    let symbol = if op == BinaryOp::Pow { "** or pow()" } else { op.symbol() };
    type_error(order, format!("unsupported operand type(s) for {symbol}: '{lhs}' and '{rhs}'"))
}

/// The type whose storage is `dtype` (`I64` reads as `int`, `F64` as `float`).
fn natural(dtype: DType) -> ExprType {
    match dtype {
        DType::Bool => ExprType::Bool,
        DType::I64 => ExprType::Int,
        DType::F64 => ExprType::Float,
        DType::Str => ExprType::Str,
        d => ExprType::Fixed(d),
    }
}

impl Planner {
    fn visit(&mut self, expr: &Expr) -> Planned {
        let key = std::ptr::from_ref(expr);
        if let Some(&n) = self.planned.get(&key) {
            return Ok(n);
        }
        let n = self.visit_new(expr)?;
        self.planned.insert(key, n);
        Ok(n)
    }

    fn visit_new(&mut self, expr: &Expr) -> Planned {
        match expr.node() {
            Node::Element => {
                let order = self.tick();
                let (dtype, ty) = self.input;
                let raw = if ty.dtype() == dtype { ty } else { natural(dtype) };
                let input = self.add(Kind::Input, raw, order);
                self.convert(input, ty, order)
            }
            Node::Const(value) => {
                let order = self.tick();
                Ok(self.constant(value, order))
            }
            Node::Unary(op, arg) => {
                let arg = self.visit(arg)?;
                let order = self.tick();
                self.unary(*op, arg, order)
            }
            Node::Binary(op, lhs, rhs) => {
                let lhs = self.visit(lhs)?;
                let rhs = self.visit(rhs)?;
                let order = self.tick();
                self.binary(*op, lhs, rhs, order)
            }
            Node::Cast(to, arg) => {
                let arg = self.visit(arg)?;
                let order = self.tick();
                self.cast(to.normalized(), arg, order)
            }
        }
    }

    fn tick(&mut self) -> u32 {
        self.next_order += 1;
        self.next_order - 1
    }

    fn add(&mut self, kind: Kind, ty: ExprType, order: u32) -> usize {
        self.nodes.push(PNode { kind, ty, order });
        self.nodes.len() - 1
    }

    /// Add a computing node, folding it when all operands are constants.
    fn node(&mut self, kind: Kind, ty: ExprType, order: u32) -> Planned {
        let (operands, count) = kind.operands();
        let constant = operands[..count].iter().all(|&i| self.constant_value(i).is_some());
        let index = self.add(kind, ty, order);
        if constant {
            match exec::fold(&self.nodes, index) {
                Ok(column) => self.nodes[index].kind = Kind::Const(column),
                Err(error) => return Err(Failure { index: 0, order, error }),
            }
        }
        Ok(index)
    }

    fn ty(&self, n: usize) -> ExprType {
        self.nodes[n].ty
    }

    /// The physical dtype of node `n`'s values.
    fn dtype(&self, n: usize) -> DType {
        match &self.nodes[n].kind {
            Kind::Const(c) => c.dtype(),
            _ => self.nodes[n].ty.dtype(),
        }
    }

    fn constant_value(&self, n: usize) -> Option<Value> {
        match &self.nodes[n].kind {
            Kind::Const(c) => c.get(0),
            _ => None,
        }
    }

    /// A Python `int` constant outside i64 (held as u64).
    fn is_big(&self, n: usize) -> bool {
        self.nodes[n].ty == ExprType::Int && self.dtype(n) == DType::U64
    }

    fn constant(&mut self, value: &Value, order: u32) -> usize {
        let (column, ty) = match value {
            Value::Bool(b) => (Column::Bool(vec![*b]), ExprType::Bool),
            Value::F32(x) => (Column::F64(vec![f64::from(*x)]), ExprType::Float),
            Value::F64(x) => (Column::F64(vec![*x]), ExprType::Float),
            Value::Str(s) => (Column::Str(vec![s.clone()]), ExprType::Str),
            // Every remaining variant is an integer of at most 64 bits.
            int => {
                let v = int.as_i128().unwrap_or_default();
                match i64::try_from(v) {
                    Ok(v) => (Column::I64(vec![v]), ExprType::Int),
                    Err(_) => (Column::U64(vec![v as u64]), ExprType::Int),
                }
            }
        };
        self.add(Kind::Const(column), ty, order)
    }

    fn bool_constant(&mut self, value: bool, order: u32) -> usize {
        self.add(Kind::Const(Column::Bool(vec![value])), ExprType::Bool, order)
    }

    /// Convert node `n` to type `ty` (range-checked; failures belong to `order`).
    fn convert(&mut self, n: usize, ty: ExprType, order: u32) -> Planned {
        if self.ty(n) == ty && self.dtype(n) == ty.dtype() {
            return Ok(n);
        }
        self.node(Kind::Convert(n), ty, order)
    }

    /// Lossless conversion to `dtype` for comparisons.
    fn widen(&mut self, n: usize, dtype: DType, order: u32) -> Planned {
        if self.dtype(n) == dtype {
            return Ok(n);
        }
        self.node(Kind::Convert(n), natural(dtype), order)
    }

    fn unary(&mut self, op: UnaryOp, arg: usize, order: u32) -> Planned {
        let ty = self.ty(arg);
        if ty == ExprType::Str {
            let message = match op {
                UnaryOp::Neg => "bad operand type for unary -: 'str'",
                UnaryOp::Pos => "bad operand type for unary +: 'str'",
                UnaryOp::Invert => "bad operand type for unary ~: 'str'",
                UnaryOp::Abs => "bad operand type for abs(): 'str'",
                UnaryOp::IsNan => "must be real number, not str",
            };
            return Err(type_error(order, message.into()));
        }
        if self.is_big(arg) && op != UnaryOp::IsNan {
            return Err(fallback(order));
        }
        match op {
            UnaryOp::IsNan if ty.is_float() => self.node(Kind::Unary(op, arg), ExprType::Bool, order),
            // Integers are never NaN (the operand still runs: it may raise).
            UnaryOp::IsNan => Ok(self.bool_constant(false, order)),
            UnaryOp::Invert if ty.is_float() => {
                Err(type_error(order, format!("bad operand type for unary ~: '{ty}'")))
            }
            UnaryOp::Invert => self.node(Kind::Unary(op, arg), ty, order),
            UnaryOp::Pos if ty == ExprType::Bool => self.convert(arg, ExprType::Int, order),
            UnaryOp::Pos => Ok(arg),
            UnaryOp::Neg | UnaryOp::Abs => {
                let arg = if ty == ExprType::Bool { self.convert(arg, ExprType::Int, order)? } else { arg };
                let ty = self.ty(arg);
                self.node(Kind::Unary(op, arg), ty, order)
            }
        }
    }

    fn binary(&mut self, op: BinaryOp, lhs: usize, rhs: usize, order: u32) -> Planned {
        if let Some(cmp) = Cmp::of(op) {
            return self.compare(cmp, lhs, rhs, order);
        }
        let (lt, rt) = (self.ty(lhs), self.ty(rhs));
        if lt == ExprType::Str || rt == ExprType::Str {
            return self.text_binary(op, lhs, rhs, order);
        }
        match (lt, rt) {
            (ExprType::Fixed(a), ExprType::Fixed(b)) if a != b => Err(unsupported(op, lt, rt, order)),
            (ExprType::Fixed(d), _) | (_, ExprType::Fixed(d)) => self.fixed_binary(op, d, lhs, rhs, order),
            _ => self.python_binary(op, lhs, rhs, order),
        }
    }

    /// Both operands are `bool`, `int` or `float`.
    fn python_binary(&mut self, op: BinaryOp, lhs: usize, rhs: usize, order: u32) -> Planned {
        use BinaryOp::*;
        let (lt, rt) = (self.ty(lhs), self.ty(rhs));
        let float = lt == ExprType::Float || rt == ExprType::Float;
        let bitwise = matches!(op, BitAnd | BitOr | BitXor | LShift | RShift);
        if bitwise && float {
            return Err(unsupported(op, lt, rt, order));
        }
        if matches!(op, BitAnd | BitOr | BitXor) && lt == ExprType::Bool && rt == ExprType::Bool {
            return self.node(Kind::Arith(op, lhs, rhs), ExprType::Bool, order);
        }
        // `int ** negative` is a float; for a constant exponent that is known statically.
        let negative_exponent = op == Pow
            && self.constant_value(rhs).and_then(|v| v.as_i128()).is_some_and(|e| e < 0);
        if float || negative_exponent {
            let lhs = self.convert(lhs, ExprType::Float, order)?;
            let rhs = self.convert(rhs, ExprType::Float, order)?;
            return self.node(Kind::Arith(op, lhs, rhs), ExprType::Float, order);
        }
        let lhs = self.convert(lhs, ExprType::Int, order)?;
        let rhs = self.convert(rhs, ExprType::Int, order)?;
        let ty = if op == TrueDiv { ExprType::Float } else { ExprType::Int };
        self.node(Kind::Arith(op, lhs, rhs), ty, order)
    }

    /// At least one operand is `Fixed(dtype)`, the other is not a different fixed type.
    fn fixed_binary(&mut self, op: BinaryOp, dtype: DType, lhs: usize, rhs: usize, order: u32) -> Planned {
        let (lt, rt) = (self.ty(lhs), self.ty(rhs));
        let fixed = ExprType::Fixed(dtype);
        let other = if lt == fixed { rt } else { lt };
        let compatible = match other {
            ExprType::Bool | ExprType::Int => true,
            ExprType::Float => dtype.is_float(),
            ExprType::Fixed(d) => d == dtype,
            ExprType::Str => false,
        };
        let bitwise = matches!(
            op,
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::LShift | BinaryOp::RShift
        );
        if !compatible || (bitwise && dtype.is_float()) {
            return Err(unsupported(op, lt, rt, order));
        }
        let lhs = self.convert(lhs, fixed, order)?;
        // A shift count is a number of bits, not a value of the type: an `int` count is kept
        // as such and checked against the width when the shift is applied.
        let shift = matches!(op, BinaryOp::LShift | BinaryOp::RShift);
        let rhs = self.convert(rhs, if shift && rt != fixed { ExprType::Int } else { fixed }, order)?;
        let ty = if op == BinaryOp::TrueDiv && dtype.is_integer() { ExprType::Float } else { fixed };
        self.node(Kind::Arith(op, lhs, rhs), ty, order)
    }

    /// At least one operand is `str`.
    fn text_binary(&mut self, op: BinaryOp, lhs: usize, rhs: usize, order: u32) -> Planned {
        let (lt, rt) = (self.ty(lhs), self.ty(rhs));
        let is_text = lt == ExprType::Str;
        match op {
            BinaryOp::Add if is_text && rt == ExprType::Str => {
                self.node(Kind::Concat(lhs, rhs), ExprType::Str, order)
            }
            BinaryOp::Add if is_text => {
                Err(type_error(order, format!("can only concatenate str (not \"{rt}\") to str")))
            }
            BinaryOp::Mul => {
                let (text, count, ct) = if is_text { (lhs, rhs, rt) } else { (rhs, lhs, lt) };
                let integral = match ct {
                    ExprType::Bool | ExprType::Int => true,
                    ExprType::Fixed(d) => d.is_integer(),
                    _ => false,
                };
                if !integral {
                    return Err(type_error(order, format!("can't multiply sequence by non-int of type '{ct}'")));
                }
                let count = self.convert(count, ExprType::Int, order)?;
                self.node(Kind::Repeat(text, count), ExprType::Str, order)
            }
            // printf-style formatting is Python's business.
            BinaryOp::Mod if is_text => Err(fallback(order)),
            _ => Err(unsupported(op, lt, rt, order)),
        }
    }

    fn compare(&mut self, cmp: Cmp, lhs: usize, rhs: usize, order: u32) -> Planned {
        let (lt, rt) = (self.ty(lhs), self.ty(rhs));
        let unrelated = (lt == ExprType::Str) != (rt == ExprType::Str)
            || matches!((lt, rt), (ExprType::Fixed(a), ExprType::Fixed(b)) if a != b);
        if unrelated {
            return match cmp {
                Cmp::Eq => Ok(self.bool_constant(false, order)),
                Cmp::Ne => Ok(self.bool_constant(true, order)),
                _ => Err(type_error(
                    order,
                    format!("'{}' not supported between instances of '{lt}' and '{rt}'", cmp.symbol()),
                )),
            };
        }
        if lt == ExprType::Str {
            return self.node(Kind::Compare(cmp, lhs, rhs), ExprType::Bool, order);
        }
        match (lt.is_float(), rt.is_float()) {
            (true, true) => {
                let f32 = ExprType::Fixed(DType::F32);
                let dtype = if lt == f32 && rt == f32 { DType::F32 } else { DType::F64 };
                let lhs = self.widen(lhs, dtype, order)?;
                let rhs = self.widen(rhs, dtype, order)?;
                self.node(Kind::Compare(cmp, lhs, rhs), ExprType::Bool, order)
            }
            (false, false) => self.int_compare(cmp, lhs, rhs, order),
            (false, true) => self.int_float_compare(cmp, lhs, rhs, order),
            (true, false) => self.int_float_compare(cmp.flip(), rhs, lhs, order),
        }
    }

    fn fits(&self, n: usize, dtype: DType) -> bool {
        let Some(v) = self.constant_value(n).and_then(|v| v.as_i128()) else { return false };
        match dtype {
            DType::Bool => (0..=1).contains(&v),
            d => d.int_range().is_some_and(|(lo, hi)| (lo..=hi).contains(&v)),
        }
    }

    /// Both operands are `bool` or integers: compare exactly in a common type.
    fn int_compare(&mut self, cmp: Cmp, lhs: usize, rhs: usize, order: u32) -> Planned {
        let (ld, rd) = (self.dtype(lhs), self.dtype(rhs));
        let (lhs, rhs, cmp) = if ld == rd {
            (lhs, rhs, cmp)
        } else if self.fits(rhs, ld) {
            (lhs, self.widen(rhs, ld, order)?, cmp)
        } else if self.fits(lhs, rd) {
            (self.widen(lhs, rd, order)?, rhs, cmp)
        } else if ld == DType::U64 {
            (lhs, self.widen(rhs, DType::I64, order)?, cmp)
        } else if rd == DType::U64 {
            (rhs, self.widen(lhs, DType::I64, order)?, cmp.flip())
        } else {
            (self.widen(lhs, DType::I64, order)?, self.widen(rhs, DType::I64, order)?, cmp)
        };
        self.node(Kind::Compare(cmp, lhs, rhs), ExprType::Bool, order)
    }

    /// `int_side cmp float_side`, compared exactly.
    fn int_float_compare(&mut self, cmp: Cmp, int: usize, float: usize, order: u32) -> Planned {
        let float = self.widen(float, DType::F64, order)?;
        let int = if self.dtype(int) == DType::U64 { int } else { self.widen(int, DType::I64, order)? };
        self.node(Kind::Compare(cmp, int, float), ExprType::Bool, order)
    }

    fn cast(&mut self, to: ExprType, arg: usize, order: u32) -> Planned {
        let from = self.ty(arg);
        if from == to && !self.is_big(arg) {
            return Ok(arg);
        }
        match (from, to) {
            (_, ExprType::Str) => Err(type_error(order, format!("cannot cast {from} to str"))),
            (ExprType::Str, ExprType::Fixed(d)) => Err(type_error(order, format!("cannot cast str to {d}"))),
            _ => self.node(Kind::Cast(arg), to, order),
        }
    }
}
