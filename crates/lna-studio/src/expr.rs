//! Small, safe math-expression parser/evaluator for figure plotting.
//!
//! Accepts textbook-style input written by a model: `x^2-2x+1`, `2sin(x)`, `(x+1)(x-1)`,
//! `sqrt(1-x^2)`, `e^(-x)`, `π/2`, `xy` (= x·y), `|x|` is not supported (use `abs(x)`).
//! No side effects, no allocation during evaluation beyond the AST, bounded recursion.

use std::fmt;

pub const VAR_X: usize = 0;
pub const VAR_Y: usize = 1;
pub const VAR_Z: usize = 2;
pub const VAR_T: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Func {
    Sin,
    Cos,
    Tan,
    Cot,
    Sec,
    Csc,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Exp,
    Ln,
    Lg,
    Log,
    Log2,
    Sqrt,
    Cbrt,
    Abs,
    Sign,
    Floor,
    Ceil,
    Round,
    Min,
    Max,
    Pow,
    Atan2,
    Mod,
}

const FUNCS: &[(&str, Func)] = &[
    ("arcsin", Func::Asin),
    ("arccos", Func::Acos),
    ("arctan", Func::Atan),
    ("atan2", Func::Atan2),
    ("log10", Func::Lg),
    ("log2", Func::Log2),
    ("floor", Func::Floor),
    ("round", Func::Round),
    ("asin", Func::Asin),
    ("acos", Func::Acos),
    ("atan", Func::Atan),
    ("sinh", Func::Sinh),
    ("cosh", Func::Cosh),
    ("tanh", Func::Tanh),
    ("sqrt", Func::Sqrt),
    ("cbrt", Func::Cbrt),
    ("sign", Func::Sign),
    ("ceil", Func::Ceil),
    ("sin", Func::Sin),
    ("cos", Func::Cos),
    ("tan", Func::Tan),
    ("cot", Func::Cot),
    ("sec", Func::Sec),
    ("csc", Func::Csc),
    ("exp", Func::Exp),
    ("abs", Func::Abs),
    ("min", Func::Min),
    ("max", Func::Max),
    ("pow", Func::Pow),
    ("mod", Func::Mod),
    ("log", Func::Log),
    ("sgn", Func::Sign),
    ("tg", Func::Tan),
    ("ln", Func::Ln),
    ("lg", Func::Lg),
];

#[derive(Debug, Clone)]
enum Node {
    Num(f64),
    Var(usize),
    Neg(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    Call(Func, Vec<Node>),
}

/// A parsed expression. Cheap to evaluate many times.
#[derive(Debug, Clone)]
pub struct Expr {
    root: Node,
    vars_used: [bool; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExprError(pub String);

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Var(usize),
    Const(f64),
    Func(Func),
    Op(char),
    LParen,
    RParen,
    Comma,
}

fn normalize(src: &str) -> String {
    let mut s = String::with_capacity(src.len() + 8);
    for c in src.chars() {
        match c {
            '×' | '·' | '⋅' | '∙' => s.push('*'),
            '÷' => s.push('/'),
            '−' | '–' => s.push('-'),
            'π' => s.push_str("pi"),
            '√' => s.push_str("sqrt"),
            '²' => s.push_str("^2"),
            '³' => s.push_str("^3"),
            '[' | '{' => s.push('('),
            ']' | '}' => s.push(')'),
            '\\' => {}
            _ => s.push(c),
        }
    }
    s.replace("**", "^")
}

fn split_ident(id: &str, out: &mut Vec<Tok>) -> Result<(), ExprError> {
    let id_norm = id.replace('θ', "t");
    if !id_norm.is_ascii() {
        return Err(ExprError(format!("unknown identifier '{}'", id)));
    }
    let lower = id_norm.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let rest = &lower[i..];
        if let Some((name, f)) = FUNCS.iter().find(|(n, _)| rest.starts_with(n)) {
            out.push(Tok::Func(*f));
            i += name.len();
            continue;
        }
        if rest.starts_with("pi") {
            out.push(Tok::Const(std::f64::consts::PI));
            i += 2;
            continue;
        }
        match b[i] {
            b'x' => out.push(Tok::Var(VAR_X)),
            b'y' => out.push(Tok::Var(VAR_Y)),
            b'z' => out.push(Tok::Var(VAR_Z)),
            b't' | b'u' => out.push(Tok::Var(VAR_T)),
            b'e' => out.push(Tok::Const(std::f64::consts::E)),
            _ => {
                return Err(ExprError(format!(
                    "unknown identifier '{}' (allowed: x, y, z, t, pi, e, and functions like sin, ln, sqrt)",
                    id
                )))
            }
        }
        i += 1;
    }
    Ok(())
}

fn lex(src: &str) -> Result<Vec<Tok>, ExprError> {
    let s = normalize(src);
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // scientific notation: 1e-3 (only when followed by digit or sign+digit)
            if i < chars.len() && (chars[i] == 'E' || chars[i] == 'e') {
                let j = i + 1;
                let k = if j < chars.len() && (chars[j] == '-' || chars[j] == '+') {
                    j + 1
                } else {
                    j
                };
                if k < chars.len() && chars[k].is_ascii_digit() && (chars[i] == 'E' || j != k) {
                    i = k;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().collect();
            let v: f64 = text
                .parse()
                .map_err(|_| ExprError(format!("bad number '{}'", text)))?;
            out.push(Tok::Num(v));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_')
                && !chars[i].is_ascii_digit()
            {
                i += 1;
            }
            // allow trailing digits only for log2/log10/atan2
            let mut word: String = chars[start..i].iter().collect();
            let lw = word.to_ascii_lowercase();
            if lw.ends_with("log") || lw.ends_with("atan") {
                let ds = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let digits: String = chars[ds..i].iter().collect();
                if matches!(digits.as_str(), "2" | "10") {
                    word.push_str(&digits);
                } else {
                    i = ds;
                }
            }
            split_ident(&word, &mut out)?;
            continue;
        }
        match c {
            '+' | '-' | '*' | '/' | '^' => out.push(Tok::Op(c)),
            '(' => out.push(Tok::LParen),
            ')' => out.push(Tok::RParen),
            ',' | ';' => out.push(Tok::Comma),
            _ => return Err(ExprError(format!("unexpected character '{}'", c))),
        }
        i += 1;
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn enter(&mut self) -> Result<(), ExprError> {
        self.depth += 1;
        if self.depth > 64 {
            return Err(ExprError("expression nested too deeply".into()));
        }
        Ok(())
    }
    fn expr(&mut self) -> Result<Node, ExprError> {
        self.enter()?;
        let mut lhs = self.term()?;
        while let Some(Tok::Op(c)) = self.peek() {
            let op = match c {
                '+' => Op::Add,
                '-' => Op::Sub,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.term()?;
            lhs = Node::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        self.depth -= 1;
        Ok(lhs)
    }
    fn term(&mut self) -> Result<Node, ExprError> {
        let mut lhs = self.unary()?;
        loop {
            match self.peek() {
                Some(Tok::Op('*')) => {
                    self.pos += 1;
                    let rhs = self.unary()?;
                    lhs = Node::Bin(Op::Mul, Box::new(lhs), Box::new(rhs));
                }
                Some(Tok::Op('/')) => {
                    self.pos += 1;
                    let rhs = self.unary()?;
                    lhs = Node::Bin(Op::Div, Box::new(lhs), Box::new(rhs));
                }
                // implicit multiplication: 2x, x(x+1), (a)(b), 2sin(x), x y
                Some(Tok::Num(_)) | Some(Tok::Var(_)) | Some(Tok::Const(_))
                | Some(Tok::Func(_)) | Some(Tok::LParen) => {
                    let rhs = self.power()?;
                    lhs = Node::Bin(Op::Mul, Box::new(lhs), Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }
    fn unary(&mut self) -> Result<Node, ExprError> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                self.enter()?;
                let v = self.unary()?;
                self.depth -= 1;
                Ok(Node::Neg(Box::new(v)))
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }
    fn power(&mut self) -> Result<Node, ExprError> {
        let base = self.primary()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            self.enter()?;
            let exp = self.unary()?;
            self.depth -= 1;
            return Ok(Node::Bin(Op::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }
    fn primary(&mut self) -> Result<Node, ExprError> {
        match self.next() {
            Some(Tok::Num(v)) | Some(Tok::Const(v)) => Ok(Node::Num(v)),
            Some(Tok::Var(i)) => Ok(Node::Var(i)),
            Some(Tok::LParen) => {
                let e = self.expr()?;
                match self.next() {
                    Some(Tok::RParen) => Ok(e),
                    _ => Err(ExprError("missing ')'".into())),
                }
            }
            Some(Tok::Func(f)) => {
                // optional power on the function name: sin^2(x)
                let mut fn_pow: Option<Node> = None;
                if let Some(Tok::Op('^')) = self.peek() {
                    self.pos += 1;
                    fn_pow = Some(self.primary()?);
                }
                let args = if let Some(Tok::LParen) = self.peek() {
                    self.pos += 1;
                    let mut args = vec![self.expr()?];
                    while let Some(Tok::Comma) = self.peek() {
                        self.pos += 1;
                        args.push(self.expr()?);
                    }
                    match self.next() {
                        Some(Tok::RParen) => {}
                        _ => return Err(ExprError("missing ')' after function arguments".into())),
                    }
                    args
                } else {
                    // sin x, ln x — bind to the next power-level operand
                    self.enter()?;
                    let a = self.power()?;
                    self.depth -= 1;
                    vec![a]
                };
                let (min, max) = match f {
                    Func::Min | Func::Max => (2, 8),
                    Func::Pow | Func::Atan2 | Func::Mod => (2, 2),
                    Func::Log => (1, 2),
                    _ => (1, 1),
                };
                if args.len() < min || args.len() > max {
                    return Err(ExprError(format!(
                        "wrong number of arguments for function ({})",
                        args.len()
                    )));
                }
                let call = Node::Call(f, args);
                Ok(match fn_pow {
                    Some(p) => Node::Bin(Op::Pow, Box::new(call), Box::new(p)),
                    None => call,
                })
            }
            Some(Tok::RParen) => Err(ExprError("unexpected ')'".into())),
            Some(Tok::Op(c)) => Err(ExprError(format!("unexpected operator '{}'", c))),
            Some(Tok::Comma) => Err(ExprError("unexpected ','".into())),
            None => Err(ExprError("unexpected end of expression".into())),
        }
    }
}

fn mark_vars(n: &Node, used: &mut [bool; 4]) {
    match n {
        Node::Var(i) => used[*i] = true,
        Node::Neg(a) => mark_vars(a, used),
        Node::Bin(_, a, b) => {
            mark_vars(a, used);
            mark_vars(b, used);
        }
        Node::Call(_, args) => args.iter().for_each(|a| mark_vars(a, used)),
        Node::Num(_) => {}
    }
}

impl Expr {
    pub fn parse(src: &str) -> Result<Expr, ExprError> {
        if src.trim().is_empty() {
            return Err(ExprError("empty expression".into()));
        }
        if src.len() > 400 {
            return Err(ExprError("expression too long".into()));
        }
        let toks = lex(src)?;
        let mut p = Parser {
            toks,
            pos: 0,
            depth: 0,
        };
        let root = p.expr()?;
        if p.pos < p.toks.len() {
            return Err(ExprError(format!(
                "unexpected token after position {}",
                p.pos
            )));
        }
        let mut vars_used = [false; 4];
        mark_vars(&root, &mut vars_used);
        Ok(Expr { root, vars_used })
    }

    /// Parses `lhs = rhs` as `lhs - (rhs)`; a plain expression is returned unchanged.
    pub fn parse_equation(src: &str) -> Result<Expr, ExprError> {
        let parts: Vec<&str> = src.split('=').collect();
        match parts.len() {
            1 => Expr::parse(src),
            2 => {
                let l = Expr::parse(parts[0])?;
                let r = Expr::parse(parts[1])?;
                let mut vars_used = l.vars_used;
                for (u, r) in vars_used.iter_mut().zip(r.vars_used) {
                    *u |= r;
                }
                Ok(Expr {
                    root: Node::Bin(Op::Sub, Box::new(l.root), Box::new(r.root)),
                    vars_used,
                })
            }
            _ => Err(ExprError("more than one '=' in equation".into())),
        }
    }

    /// Returns an error when the expression uses a variable outside `allowed` (indices).
    pub fn check_vars(&self, allowed: &[usize], names: &str) -> Result<(), ExprError> {
        for (i, used) in self.vars_used.iter().enumerate() {
            if *used && !allowed.contains(&i) {
                let n = ["x", "y", "z", "t"][i];
                return Err(ExprError(format!(
                    "variable '{}' is not allowed here (use {})",
                    n, names
                )));
            }
        }
        Ok(())
    }

    pub fn uses(&self, var: usize) -> bool {
        self.vars_used[var]
    }

    pub fn eval(&self, vars: &[f64; 4]) -> f64 {
        eval_node(&self.root, vars)
    }

    pub fn eval_x(&self, x: f64) -> f64 {
        self.eval(&[x, 0.0, 0.0, x])
    }

    pub fn eval_xy(&self, x: f64, y: f64) -> f64 {
        self.eval(&[x, y, 0.0, 0.0])
    }

    pub fn eval_t(&self, t: f64) -> f64 {
        self.eval(&[t, 0.0, 0.0, t])
    }
}

fn eval_node(n: &Node, v: &[f64; 4]) -> f64 {
    match n {
        Node::Num(x) => *x,
        Node::Var(i) => v[*i],
        Node::Neg(a) => -eval_node(a, v),
        Node::Bin(op, a, b) => {
            let x = eval_node(a, v);
            let y = eval_node(b, v);
            match op {
                Op::Add => x + y,
                Op::Sub => x - y,
                Op::Mul => x * y,
                Op::Div => x / y,
                Op::Pow => pow_real(x, y),
            }
        }
        Node::Call(f, args) => {
            let a = eval_node(&args[0], v);
            match f {
                Func::Sin => a.sin(),
                Func::Cos => a.cos(),
                Func::Tan => a.tan(),
                Func::Cot => 1.0 / a.tan(),
                Func::Sec => 1.0 / a.cos(),
                Func::Csc => 1.0 / a.sin(),
                Func::Asin => a.asin(),
                Func::Acos => a.acos(),
                Func::Atan => a.atan(),
                Func::Sinh => a.sinh(),
                Func::Cosh => a.cosh(),
                Func::Tanh => a.tanh(),
                Func::Exp => a.exp(),
                Func::Ln => a.ln(),
                Func::Lg => a.log10(),
                Func::Log2 => a.log2(),
                Func::Log => {
                    if args.len() == 2 {
                        // log(base, x)
                        let x = eval_node(&args[1], v);
                        x.ln() / a.ln()
                    } else {
                        a.ln()
                    }
                }
                Func::Sqrt => a.sqrt(),
                Func::Cbrt => a.cbrt(),
                Func::Abs => a.abs(),
                Func::Sign => {
                    if a > 0.0 {
                        1.0
                    } else if a < 0.0 {
                        -1.0
                    } else {
                        0.0
                    }
                }
                Func::Floor => a.floor(),
                Func::Ceil => a.ceil(),
                Func::Round => a.round(),
                Func::Min => args[1..].iter().fold(a, |m, e| m.min(eval_node(e, v))),
                Func::Max => args[1..].iter().fold(a, |m, e| m.max(eval_node(e, v))),
                Func::Pow => pow_real(a, eval_node(&args[1], v)),
                Func::Atan2 => a.atan2(eval_node(&args[1], v)),
                Func::Mod => a.rem_euclid(eval_node(&args[1], v)),
            }
        }
    }
}

/// Real power that also handles odd roots of negatives, e.g. x^(1/3) for x < 0.
fn pow_real(x: f64, y: f64) -> f64 {
    let r = x.powf(y);
    if r.is_nan() && x < 0.0 && y.is_finite() && y != 0.0 {
        let inv = 1.0 / y;
        let k = inv.round();
        if (inv - k).abs() < 1e-9 && (k as i64) % 2 != 0 {
            return -((-x).powf(y));
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str, x: f64) -> f64 {
        Expr::parse(s).unwrap().eval_x(x)
    }

    #[test]
    fn precedence_and_implicit_multiplication() {
        assert_eq!(ev("x^2-2x+1", 3.0), 4.0);
        assert_eq!(ev("-x^2", 2.0), -4.0);
        assert_eq!(ev("2^-1", 0.0), 0.5);
        assert_eq!(ev("(x+1)(x-1)", 3.0), 8.0);
        assert_eq!(ev("2(x+1)", 1.0), 4.0);
        assert!((ev("2sin(x)", std::f64::consts::FRAC_PI_2) - 2.0).abs() < 1e-12);
        assert!((ev("sinx", std::f64::consts::FRAC_PI_2) - 1.0).abs() < 1e-12);
        assert!((ev("π/2", 0.0) - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!((ev("e^x", 1.0) - std::f64::consts::E).abs() < 1e-12);
        assert!((ev("exp(x)", 1.0) - std::f64::consts::E).abs() < 1e-12);
        assert!((ev("x^(1/3)", -8.0) + 2.0).abs() < 1e-9);
        assert!((ev("log(2, x)", 8.0) - 3.0).abs() < 1e-12);
        assert!((ev("lg(x)", 100.0) - 2.0).abs() < 1e-12);
        assert!((ev("sin^2(x)+cos^2(x)", 0.7) - 1.0).abs() < 1e-12);
        assert_eq!(ev("1.5E2", 0.0), 150.0);
        assert!((ev("1e-3", 0.0) - 0.001).abs() < 1e-15);
        // lowercase e without a sign is Euler's number: 2e = 2·e
        assert!((ev("2e", 0.0) - 2.0 * std::f64::consts::E).abs() < 1e-12);
        assert_eq!(ev("x²", 3.0), 9.0);
    }

    #[test]
    fn two_variable_and_equations() {
        let e = Expr::parse("xy").unwrap();
        assert_eq!(e.eval_xy(2.0, 3.0), 6.0);
        let c = Expr::parse_equation("x^2+y^2=1").unwrap();
        assert_eq!(c.eval_xy(1.0, 0.0), 0.0);
        assert!(c.check_vars(&[VAR_X, VAR_Y], "x, y").is_ok());
        assert!(Expr::parse("x+z")
            .unwrap()
            .check_vars(&[VAR_X], "x")
            .is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(Expr::parse("").is_err());
        assert!(Expr::parse("x+").is_err());
        assert!(Expr::parse("foo(x)").is_err());
        assert!(Expr::parse("(x+1").is_err());
        assert!(Expr::parse("system(1)").is_err());
        assert!(Expr::parse(&"(".repeat(200)).is_err());
    }
}
