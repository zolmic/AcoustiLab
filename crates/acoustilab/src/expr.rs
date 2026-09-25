//! Expressions of parametric netlists (docs/parameters.md).
//!
//! A small, total language over numbers, booleans and strings:
//!
//! ```text
//! expr    := or
//! or      := and ('||' and)*
//! and     := cmp ('&&' cmp)*
//! cmp     := add (('==' | '!=' | '<' | '<=' | '>' | '>=') add)?
//! add     := mul (('+' | '-') mul)*
//! mul     := unary (('*' | '/' | '%') unary)*
//! unary   := ('-' | '+' | '!') unary | pow
//! pow     := atom ('^' unary)?            right-associative; -2^2 = -4
//! atom    := number | 'string' | "string" | true | false
//!          | name | name '(' args ')' | '(' expr ')'
//! ```
//!
//! Names are parameters, or the constants `pi` and `e`. `if(c, a, b)`
//! evaluates only the branch it takes, so `if(n > 0, 1/n, 0)` is safe.
//! Arithmetic that produces NaN or an infinity is an error, never a value.

use std::fmt;

/// A value of the expression language.
#[derive(Debug, Clone, PartialEq)]
pub enum PValue {
    Num(f64),
    Bool(bool),
    Str(String),
}

impl PValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            PValue::Num(_) => "number",
            PValue::Bool(_) => "boolean",
            PValue::Str(_) => "string",
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            PValue::Num(x) => Some(*x),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            PValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// JSON form. Integral numbers of moderate size become JSON integers,
    /// so keys read as counts (`count`, `segments`, `level`) accept them.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            PValue::Num(x) => num_to_json(*x),
            PValue::Bool(b) => serde_json::Value::Bool(*b),
            PValue::Str(s) => serde_json::Value::String(s.clone()),
        }
    }

    /// From a JSON scalar; `None` for arrays, objects and null.
    pub fn from_json(v: &serde_json::Value) -> Option<PValue> {
        match v {
            serde_json::Value::Number(n) => n.as_f64().map(PValue::Num),
            serde_json::Value::Bool(b) => Some(PValue::Bool(*b)),
            serde_json::Value::String(s) => Some(PValue::Str(s.clone())),
            _ => None,
        }
    }
}

impl fmt::Display for PValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PValue::Num(x) => write!(f, "{x}"),
            PValue::Bool(b) => write!(f, "{b}"),
            PValue::Str(s) => write!(f, "'{s}'"),
        }
    }
}

/// JSON number, as an integer when `x` is integral and exactly representable.
pub fn num_to_json(x: f64) -> serde_json::Value {
    if x.fract() == 0.0 && x.abs() < 9.007_199_254_740_992e15 {
        if x >= 0.0 {
            serde_json::Value::from(x as u64)
        } else {
            serde_json::Value::from(x as i64)
        }
    } else {
        serde_json::Number::from_f64(x)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
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
    fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Pow => "^",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Lit(PValue),
    Var(String),
    Neg(Box<Node>),
    Not(Box<Node>),
    Bin(BinOp, Box<Node>, Box<Node>),
    Call(String, Vec<Node>),
}

/// A parsed expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    src: String,
    root: Node,
}

/// Functions the language knows, with their arity (`None`: one or more).
pub const FUNCTIONS: &[(&str, Option<usize>)] = &[
    ("sqrt", Some(1)),
    ("abs", Some(1)),
    ("exp", Some(1)),
    ("ln", Some(1)),
    ("log10", Some(1)),
    ("log2", Some(1)),
    ("sin", Some(1)),
    ("cos", Some(1)),
    ("tan", Some(1)),
    ("asin", Some(1)),
    ("acos", Some(1)),
    ("atan", Some(1)),
    ("atan2", Some(2)),
    ("hypot", Some(2)),
    ("floor", Some(1)),
    ("ceil", Some(1)),
    ("round", Some(1)),
    ("min", None),
    ("max", None),
    ("clamp", Some(3)),
    ("if", Some(3)),
];

/// Names that cannot be parameters.
pub fn is_reserved(name: &str) -> bool {
    matches!(name, "pi" | "e" | "true" | "false") || FUNCTIONS.iter().any(|(f, _)| *f == name)
}

/// True for names the lexer reads as one identifier.
pub fn is_identifier(name: &str) -> bool {
    let mut c = name.chars();
    matches!(c.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_')
        && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    const OPS: [&str; 18] = [
        "&&", "||", "==", "!=", "<=", ">=", "**", "+", "-", "*", "/", "%", "^", "<", ">", "!", "(",
        ")",
    ];
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if c.is_ascii_digit()
            || (c == '.' && i + 1 < b.len() && (b[i + 1] as char).is_ascii_digit())
        {
            while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                if j < b.len() && (b[j] as char).is_ascii_digit() {
                    i = j;
                    while i < b.len() && (b[i] as char).is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text = &src[start..i];
            let x: f64 = text
                .parse()
                .map_err(|_| format!("bad number '{text}' at {}", start + 1))?;
            out.push((Tok::Num(x), start));
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push((Tok::Ident(src[start..i].to_string()), start));
            continue;
        }
        if c == '\'' || c == '"' {
            let q = b[i];
            i += 1;
            let s0 = i;
            while i < b.len() && b[i] != q {
                i += 1;
            }
            if i >= b.len() {
                return Err(format!("unterminated string at {}", start + 1));
            }
            out.push((Tok::Str(src[s0..i].to_string()), start));
            i += 1;
            continue;
        }
        if c == ',' {
            out.push((Tok::Comma, start));
            i += 1;
            continue;
        }
        let rest = &src[i..];
        match OPS.iter().find(|op| rest.starts_with(**op)) {
            Some(&"(") => out.push((Tok::LParen, start)),
            Some(&")") => out.push((Tok::RParen, start)),
            Some(&"**") => out.push((Tok::Op("^"), start)),
            Some(op) => out.push((Tok::Op(op), start)),
            None => return Err(format!("unexpected '{c}' at {}", start + 1)),
        }
        i += match OPS.iter().find(|op| rest.starts_with(**op)) {
            Some(op) => op.len(),
            None => 1,
        };
    }
    Ok(out)
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    len: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.0)
    }

    fn at(&self) -> usize {
        self.toks.get(self.pos).map_or(self.len, |t| t.1) + 1
    }

    fn eat_op(&mut self, ops: &[&'static str]) -> Option<&'static str> {
        if let Some(Tok::Op(o)) = self.peek() {
            if let Some(found) = ops.iter().find(|x| *x == o) {
                self.pos += 1;
                return Some(found);
            }
        }
        None
    }

    fn expr(&mut self) -> Result<Node, String> {
        self.or()
    }

    fn or(&mut self) -> Result<Node, String> {
        let mut l = self.and()?;
        while self.eat_op(&["||"]).is_some() {
            let r = self.and()?;
            l = Node::Bin(BinOp::Or, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Node, String> {
        let mut l = self.cmp()?;
        while self.eat_op(&["&&"]).is_some() {
            let r = self.cmp()?;
            l = Node::Bin(BinOp::And, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn cmp(&mut self) -> Result<Node, String> {
        let l = self.add()?;
        let op = match self.eat_op(&["==", "!=", "<=", ">=", "<", ">"]) {
            Some("==") => BinOp::Eq,
            Some("!=") => BinOp::Ne,
            Some("<=") => BinOp::Le,
            Some(">=") => BinOp::Ge,
            Some("<") => BinOp::Lt,
            Some(">") => BinOp::Gt,
            _ => return Ok(l),
        };
        let r = self.add()?;
        if matches!(self.peek(), Some(Tok::Op(o)) if ["==", "!=", "<=", ">=", "<", ">"].contains(o))
        {
            return Err(format!(
                "comparisons do not chain (at {}); use && between them",
                self.at()
            ));
        }
        Ok(Node::Bin(op, Box::new(l), Box::new(r)))
    }

    fn add(&mut self) -> Result<Node, String> {
        let mut l = self.mul()?;
        while let Some(o) = self.eat_op(&["+", "-"]) {
            let r = self.mul()?;
            let op = if o == "+" { BinOp::Add } else { BinOp::Sub };
            l = Node::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn mul(&mut self) -> Result<Node, String> {
        let mut l = self.unary()?;
        while let Some(o) = self.eat_op(&["*", "/", "%"]) {
            let r = self.unary()?;
            let op = match o {
                "*" => BinOp::Mul,
                "/" => BinOp::Div,
                _ => BinOp::Rem,
            };
            l = Node::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Node, String> {
        match self.eat_op(&["-", "+", "!"]) {
            Some("-") => Ok(Node::Neg(Box::new(self.unary()?))),
            Some("+") => self.unary(),
            Some(_) => Ok(Node::Not(Box::new(self.unary()?))),
            None => self.pow(),
        }
    }

    fn pow(&mut self) -> Result<Node, String> {
        let base = self.atom()?;
        if self.eat_op(&["^"]).is_some() {
            let exp = self.unary()?;
            return Ok(Node::Bin(BinOp::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Node, String> {
        let at = self.at();
        let tok = self
            .toks
            .get(self.pos)
            .map(|t| t.0.clone())
            .ok_or_else(|| "unexpected end of expression".to_string())?;
        self.pos += 1;
        match tok {
            Tok::Num(x) => Ok(Node::Lit(PValue::Num(x))),
            Tok::Str(s) => Ok(Node::Lit(PValue::Str(s))),
            Tok::LParen => {
                let e = self.expr()?;
                match self.peek() {
                    Some(Tok::RParen) => {
                        self.pos += 1;
                        Ok(e)
                    }
                    _ => Err(format!("expected ')' at {}", self.at())),
                }
            }
            Tok::Ident(name) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.expr()?);
                            match self.peek() {
                                Some(Tok::Comma) => self.pos += 1,
                                Some(Tok::RParen) => break,
                                _ => return Err(format!("expected ',' or ')' at {}", self.at())),
                            }
                        }
                    }
                    self.pos += 1;
                    let arity = FUNCTIONS
                        .iter()
                        .find(|(f, _)| *f == name)
                        .map(|(_, a)| *a)
                        .ok_or_else(|| format!("unknown function '{name}' at {at}"))?;
                    match arity {
                        Some(n) if args.len() != n => {
                            return Err(format!(
                                "'{name}' takes {n} argument(s), got {}",
                                args.len()
                            ))
                        }
                        None if args.is_empty() => {
                            return Err(format!("'{name}' needs at least one argument"))
                        }
                        _ => {}
                    }
                    return Ok(Node::Call(name, args));
                }
                Ok(match name.as_str() {
                    "true" => Node::Lit(PValue::Bool(true)),
                    "false" => Node::Lit(PValue::Bool(false)),
                    "pi" => Node::Lit(PValue::Num(std::f64::consts::PI)),
                    "e" => Node::Lit(PValue::Num(std::f64::consts::E)),
                    _ if FUNCTIONS.iter().any(|(f, _)| *f == name) => {
                        return Err(format!("function '{name}' needs arguments (at {at})"))
                    }
                    _ => Node::Var(name),
                })
            }
            Tok::Op(o) => Err(format!("unexpected '{o}' at {at}")),
            Tok::RParen => Err(format!("unexpected ')' at {at}")),
            Tok::Comma => Err(format!("unexpected ',' at {at}")),
        }
    }
}

fn free_vars(n: &Node, out: &mut Vec<String>) {
    match n {
        Node::Lit(_) => {}
        Node::Var(v) => {
            if !out.contains(v) {
                out.push(v.clone());
            }
        }
        Node::Neg(a) | Node::Not(a) => free_vars(a, out),
        Node::Bin(_, a, b) => {
            free_vars(a, out);
            free_vars(b, out);
        }
        Node::Call(_, args) => args.iter().for_each(|a| free_vars(a, out)),
    }
}

fn num(v: PValue, what: &str) -> Result<f64, String> {
    match v {
        PValue::Num(x) => Ok(x),
        other => Err(format!("{what} needs a number, got {}", other.type_name())),
    }
}

fn boolean(v: PValue, what: &str) -> Result<bool, String> {
    match v {
        PValue::Bool(b) => Ok(b),
        other => Err(format!("{what} needs a boolean, got {}", other.type_name())),
    }
}

fn finite(x: f64, what: &str) -> Result<PValue, String> {
    if x.is_finite() {
        Ok(PValue::Num(x))
    } else {
        Err(format!("{what} is not a finite number"))
    }
}

fn eval(n: &Node, env: &dyn Fn(&str) -> Option<PValue>) -> Result<PValue, String> {
    match n {
        Node::Lit(v) => Ok(v.clone()),
        Node::Var(name) => env(name).ok_or_else(|| format!("unknown name '{name}'")),
        Node::Neg(a) => finite(-num(eval(a, env)?, "'-'")?, "negation"),
        Node::Not(a) => Ok(PValue::Bool(!boolean(eval(a, env)?, "'!'")?)),
        Node::Bin(op, a, b) => {
            // Short-circuit logic.
            if *op == BinOp::And || *op == BinOp::Or {
                let l = boolean(eval(a, env)?, op.symbol())?;
                if (*op == BinOp::And && !l) || (*op == BinOp::Or && l) {
                    return Ok(PValue::Bool(l));
                }
                return Ok(PValue::Bool(boolean(eval(b, env)?, op.symbol())?));
            }
            let l = eval(a, env)?;
            let r = eval(b, env)?;
            match op {
                BinOp::Eq | BinOp::Ne => {
                    if std::mem::discriminant(&l) != std::mem::discriminant(&r) {
                        return Err(format!(
                            "cannot compare {} with {}",
                            l.type_name(),
                            r.type_name()
                        ));
                    }
                    let eq = l == r;
                    Ok(PValue::Bool(if *op == BinOp::Eq { eq } else { !eq }))
                }
                BinOp::Add => match (l, r) {
                    (PValue::Str(x), PValue::Str(y)) => Ok(PValue::Str(x + &y)),
                    (l, r) => finite(num(l, "'+'")? + num(r, "'+'")?, "the sum"),
                },
                _ => {
                    let s = op.symbol();
                    let (x, y) = (num(l, s)?, num(r, s)?);
                    match op {
                        BinOp::Sub => finite(x - y, "the difference"),
                        BinOp::Mul => finite(x * y, "the product"),
                        BinOp::Div => {
                            if y == 0.0 {
                                Err("division by zero".into())
                            } else {
                                finite(x / y, "the quotient")
                            }
                        }
                        BinOp::Rem => {
                            if y == 0.0 {
                                Err("remainder by zero".into())
                            } else {
                                finite(x % y, "the remainder")
                            }
                        }
                        BinOp::Pow => finite(x.powf(y), "the power"),
                        BinOp::Lt => Ok(PValue::Bool(x < y)),
                        BinOp::Le => Ok(PValue::Bool(x <= y)),
                        BinOp::Gt => Ok(PValue::Bool(x > y)),
                        BinOp::Ge => Ok(PValue::Bool(x >= y)),
                        _ => unreachable!("handled above"),
                    }
                }
            }
        }
        Node::Call(f, args) => {
            if f == "if" {
                return if boolean(eval(&args[0], env)?, "if()")? {
                    eval(&args[1], env)
                } else {
                    eval(&args[2], env)
                };
            }
            let xs = args
                .iter()
                .map(|a| eval(a, env).and_then(|v| num(v, &format!("{f}()"))))
                .collect::<Result<Vec<f64>, String>>()?;
            let x = xs[0];
            let what = format!("{f}()");
            let domain = |ok: bool| {
                if ok {
                    Ok(())
                } else {
                    Err(format!("{f}() is undefined for {x}"))
                }
            };
            let y = match f.as_str() {
                "sqrt" => {
                    domain(x >= 0.0)?;
                    x.sqrt()
                }
                "abs" => x.abs(),
                "exp" => x.exp(),
                "ln" => {
                    domain(x > 0.0)?;
                    x.ln()
                }
                "log10" => {
                    domain(x > 0.0)?;
                    x.log10()
                }
                "log2" => {
                    domain(x > 0.0)?;
                    x.log2()
                }
                "sin" => x.sin(),
                "cos" => x.cos(),
                "tan" => x.tan(),
                "asin" => {
                    domain(x.abs() <= 1.0)?;
                    x.asin()
                }
                "acos" => {
                    domain(x.abs() <= 1.0)?;
                    x.acos()
                }
                "atan" => x.atan(),
                "atan2" => x.atan2(xs[1]),
                "hypot" => x.hypot(xs[1]),
                "floor" => x.floor(),
                "ceil" => x.ceil(),
                "round" => x.round(),
                "min" => xs.iter().copied().fold(f64::INFINITY, f64::min),
                "max" => xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                "clamp" => {
                    if xs[1] > xs[2] {
                        return Err(format!("clamp(): lower bound {} > upper {}", xs[1], xs[2]));
                    }
                    x.clamp(xs[1], xs[2])
                }
                _ => return Err(format!("unknown function '{f}'")),
            };
            finite(y, &what)
        }
    }
}

impl Expr {
    /// Parses an expression (without the leading `=` of netlist strings).
    pub fn parse(src: &str) -> Result<Expr, String> {
        let toks = lex(src)?;
        if toks.is_empty() {
            return Err("empty expression".into());
        }
        let mut p = Parser {
            toks,
            pos: 0,
            len: src.len(),
        };
        let root = p.expr()?;
        if p.pos < p.toks.len() {
            return Err(format!("unexpected text at {}", p.at()));
        }
        Ok(Expr {
            src: src.to_string(),
            root,
        })
    }

    pub fn source(&self) -> &str {
        &self.src
    }

    /// Names the expression refers to (excluding constants and functions),
    /// in order of first appearance.
    pub fn names(&self) -> Vec<String> {
        let mut v = Vec::new();
        free_vars(&self.root, &mut v);
        v
    }

    /// If the expression is a bare name, that name.
    pub fn as_name(&self) -> Option<&str> {
        match &self.root {
            Node::Var(v) => Some(v),
            _ => None,
        }
    }

    /// Evaluates with `env` resolving names.
    pub fn eval(&self, env: &dyn Fn(&str) -> Option<PValue>) -> Result<PValue, String> {
        eval(&self.root, env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(src: &str) -> Result<PValue, String> {
        let vars = |n: &str| match n {
            "r" => Some(PValue::Num(2.0)),
            "n" => Some(PValue::Num(0.0)),
            "pad" => Some(PValue::Str("velour".into())),
            "sealed" => Some(PValue::Bool(true)),
            _ => None,
        };
        Expr::parse(src)?.eval(&vars)
    }

    fn num(src: &str) -> f64 {
        ev(src).unwrap().as_num().unwrap()
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(num("1 + 2 * 3"), 7.0);
        assert_eq!(num("(1 + 2) * 3"), 9.0);
        assert_eq!(num("-2^2"), -4.0);
        assert_eq!(num("2^3^2"), 512.0);
        assert_eq!(num("2**3"), 8.0);
        assert_eq!(num("2^-1"), 0.5);
        assert_eq!(num("7 % 4"), 3.0);
        assert_eq!(num("1.5e-3 * 1e3"), 1.5);
        assert_eq!(num(".5 + 1."), 1.5);
        assert!((num("pi * r^2") - 4.0 * std::f64::consts::PI).abs() < 1e-15);
        assert_eq!(num("max(1, r, 3) + min(4, r)"), 5.0);
        assert_eq!(num("clamp(5, 0, r)"), 2.0);
        assert!((num("hypot(3, 4)") - 5.0).abs() < 1e-15);
    }

    #[test]
    fn logic_strings_and_lazy_if() {
        assert_eq!(ev("r > 1 && !(n > 0)").unwrap(), PValue::Bool(true));
        assert_eq!(ev("pad == 'velour'").unwrap(), PValue::Bool(true));
        assert_eq!(ev("pad != \"leather\"").unwrap(), PValue::Bool(true));
        assert_eq!(ev("'a_' + pad").unwrap(), PValue::Str("a_velour".into()));
        assert_eq!(ev("if(n > 0, 1 / n, 0)").unwrap(), PValue::Num(0.0));
        assert_eq!(ev("sealed || 1 / n > 0").unwrap(), PValue::Bool(true));
        assert_eq!(
            ev("if(sealed, 'ambient', 'a_rear')").unwrap(),
            PValue::Str("ambient".into())
        );
    }

    #[test]
    fn errors_are_reported_not_propagated_as_nan() {
        assert!(ev("1 / n").unwrap_err().contains("division by zero"));
        assert!(ev("sqrt(-1)").unwrap_err().contains("undefined"));
        assert!(ev("ln(n)").is_err());
        assert!(ev("10^400").unwrap_err().contains("finite"));
        assert!(ev("q + 1").unwrap_err().contains("unknown name 'q'"));
        assert!(ev("pad + 1").unwrap_err().contains("number"));
        assert!(ev("pad == 1").unwrap_err().contains("compare"));
        assert!(ev("if(1, 2, 3)").unwrap_err().contains("boolean"));
        assert!(Expr::parse("1 < 2 < 3").unwrap_err().contains("chain"));
        assert!(Expr::parse("foo(1)")
            .unwrap_err()
            .contains("unknown function"));
        assert!(Expr::parse("sqrt(1, 2)").unwrap_err().contains("argument"));
        assert!(Expr::parse("(1 + 2").is_err());
        assert!(Expr::parse("1 +").is_err());
        assert!(Expr::parse("1 2").is_err());
        assert!(Expr::parse("'abc").is_err());
        assert!(Expr::parse("").is_err());
        assert!(Expr::parse("sqrt").is_err());
        assert!(Expr::parse("1 $ 2").is_err());
    }

    #[test]
    fn names_and_json() {
        let e = Expr::parse("pi * r^2 * depth + r").unwrap();
        assert_eq!(e.names(), vec!["r".to_string(), "depth".to_string()]);
        assert_eq!(Expr::parse("vent_d").unwrap().as_name(), Some("vent_d"));
        assert_eq!(Expr::parse("vent_d * 1").unwrap().as_name(), None);
        assert_eq!(PValue::Num(3.0).to_json(), serde_json::json!(3));
        assert!(PValue::Num(3.0).to_json().is_u64());
        assert_eq!(PValue::Num(-2.0).to_json(), serde_json::json!(-2));
        assert_eq!(PValue::Num(2.5).to_json(), serde_json::json!(2.5));
        assert!(is_identifier("vent_d_mm") && !is_identifier("2x") && !is_identifier("a-b"));
        assert!(is_reserved("pi") && is_reserved("sqrt") && !is_reserved("radius"));
    }
}
