//! A tiny arithmetic expression evaluator for PID scaling formulas.
//!
//! Handoff §6 requires vehicle knowledge to be *data*, not `match` arms. That
//! means the scaling of `41 0C 1A F8` into 1726 rpm has to come out of a file:
//!
//! ```yaml
//! formula: "(256 * A + B) / 4"
//! ```
//!
//! So the decoder needs to evaluate arithmetic at runtime. This is a complete
//! recursive-descent parser for the small language those formulas need:
//! decimal literals, the single-letter byte variables `A`..`H`, `+ - * / %`,
//! unary minus, parentheses, and the two-argument functions `min` and `max`.
//! Anything else is a parse error at load time, not a wrong number at runtime.

use aim_types::{AimError, AimResult, ErrorCode};

/// A parsed formula, ready to evaluate against a byte slice.
#[derive(Debug, Clone, PartialEq)]
pub struct Formula {
    root: Node,
    source: String,
    /// Highest byte index referenced, e.g. `B` -> 1. `None` for a constant
    /// formula. Used to validate payload length.
    max_byte_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Literal(f64),
    Byte(usize),
    Neg(Box<Node>),
    Binary(Op, Box<Node>, Box<Node>),
    Call(Func, Box<Node>, Box<Node>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Func {
    Min,
    Max,
}

impl Formula {
    /// Parse a formula. Errors mention the offending text so a bad data file
    /// fails loudly at load time.
    pub fn parse(source: &str) -> AimResult<Formula> {
        let tokens = tokenize(source)?;
        let mut p = Parser { tokens, pos: 0, source };
        let root = p.expression()?;
        if p.pos != p.tokens.len() {
            return Err(p.err(format!("unexpected trailing input at token {}", p.pos)));
        }
        let max_byte_index = max_index(&root);
        Ok(Formula { root, source: source.to_string(), max_byte_index })
    }

    /// The original formula text, kept for provenance and error messages.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Number of payload bytes this formula needs.
    pub fn required_bytes(&self) -> usize {
        self.max_byte_index.map_or(0, |i| i + 1)
    }

    /// Evaluate against a payload, where `A` is `bytes[0]`, `B` is `bytes[1]`, …
    pub fn eval(&self, bytes: &[u8]) -> AimResult<f64> {
        if bytes.len() < self.required_bytes() {
            return Err(AimError::new(
                ErrorCode::DecoderInputInvalid,
                format!(
                    "formula {:?} needs {} payload bytes, got {}",
                    self.source,
                    self.required_bytes(),
                    bytes.len()
                ),
            ));
        }
        eval_node(&self.root, bytes)
    }
}

fn max_index(n: &Node) -> Option<usize> {
    match n {
        Node::Literal(_) => None,
        Node::Byte(i) => Some(*i),
        Node::Neg(inner) => max_index(inner),
        Node::Binary(_, a, b) | Node::Call(_, a, b) => match (max_index(a), max_index(b)) {
            (Some(x), Some(y)) => Some(x.max(y)),
            (x, y) => x.or(y),
        },
    }
}

fn eval_node(n: &Node, bytes: &[u8]) -> AimResult<f64> {
    Ok(match n {
        Node::Literal(v) => *v,
        Node::Byte(i) => bytes[*i] as f64,
        Node::Neg(inner) => -eval_node(inner, bytes)?,
        Node::Binary(op, a, b) => {
            let (x, y) = (eval_node(a, bytes)?, eval_node(b, bytes)?);
            match op {
                Op::Add => x + y,
                Op::Sub => x - y,
                Op::Mul => x * y,
                Op::Div => {
                    if y == 0.0 {
                        return Err(AimError::new(
                            ErrorCode::DecoderInputInvalid,
                            "division by zero while evaluating a scaling formula",
                        ));
                    }
                    x / y
                }
                Op::Rem => {
                    if y == 0.0 {
                        return Err(AimError::new(
                            ErrorCode::DecoderInputInvalid,
                            "modulo by zero while evaluating a scaling formula",
                        ));
                    }
                    x % y
                }
            }
        }
        Node::Call(f, a, b) => {
            let (x, y) = (eval_node(a, bytes)?, eval_node(b, bytes)?);
            match f {
                Func::Min => x.min(y),
                Func::Max => x.max(y),
            }
        }
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Byte(usize),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    LParen,
    RParen,
    Comma,
}

fn tokenize(s: &str) -> AimResult<Vec<Token>> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '-' => {
                out.push(Token::Minus);
                i += 1;
            }
            '*' => {
                out.push(Token::Star);
                i += 1;
            }
            '/' => {
                out.push(Token::Slash);
                i += 1;
            }
            '%' => {
                out.push(Token::Percent);
                i += 1;
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let v = text.parse::<f64>().map_err(|e| {
                    AimError::new(
                        ErrorCode::DecoderInputInvalid,
                        format!("invalid number {text:?} in formula {s:?}: {e}"),
                    )
                })?;
                out.push(Token::Number(v));
            }
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                // A single uppercase A..H is a payload byte reference.
                if text.len() == 1 {
                    let ch = text.chars().next().expect("length checked");
                    if ('A'..='H').contains(&ch) {
                        out.push(Token::Byte(ch as usize - 'A' as usize));
                        continue;
                    }
                }
                out.push(Token::Ident(text.to_ascii_lowercase()));
            }
            other => {
                return Err(AimError::new(
                    ErrorCode::DecoderInputInvalid,
                    format!("unexpected character {other:?} in formula {s:?}"),
                ))
            }
        }
    }
    if out.is_empty() {
        return Err(AimError::new(ErrorCode::DecoderInputInvalid, format!("empty formula {s:?}")));
    }
    Ok(out)
}

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    source: &'a str,
}

impl Parser<'_> {
    fn err(&self, msg: String) -> AimError {
        AimError::new(ErrorCode::DecoderInputInvalid, format!("formula {:?}: {msg}", self.source))
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn eat(&mut self, t: &Token) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expression(&mut self) -> AimResult<Node> {
        let mut left = self.term()?;
        loop {
            if self.eat(&Token::Plus) {
                left = Node::Binary(Op::Add, Box::new(left), Box::new(self.term()?));
            } else if self.eat(&Token::Minus) {
                left = Node::Binary(Op::Sub, Box::new(left), Box::new(self.term()?));
            } else {
                return Ok(left);
            }
        }
    }

    fn term(&mut self) -> AimResult<Node> {
        let mut left = self.unary()?;
        loop {
            if self.eat(&Token::Star) {
                left = Node::Binary(Op::Mul, Box::new(left), Box::new(self.unary()?));
            } else if self.eat(&Token::Slash) {
                left = Node::Binary(Op::Div, Box::new(left), Box::new(self.unary()?));
            } else if self.eat(&Token::Percent) {
                left = Node::Binary(Op::Rem, Box::new(left), Box::new(self.unary()?));
            } else {
                return Ok(left);
            }
        }
    }

    fn unary(&mut self) -> AimResult<Node> {
        if self.eat(&Token::Minus) {
            return Ok(Node::Neg(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> AimResult<Node> {
        let token = self
            .peek()
            .cloned()
            .ok_or_else(|| self.err(String::from("unexpected end of input")))?;
        self.pos += 1;
        match token {
            Token::Number(v) => Ok(Node::Literal(v)),
            Token::Byte(i) => Ok(Node::Byte(i)),
            Token::LParen => {
                let inner = self.expression()?;
                if !self.eat(&Token::RParen) {
                    return Err(self.err(String::from("missing closing parenthesis")));
                }
                Ok(inner)
            }
            Token::Ident(name) => {
                let func = match name.as_str() {
                    "min" => Func::Min,
                    "max" => Func::Max,
                    other => {
                        return Err(self.err(format!(
                            "unknown identifier {other:?}; only byte references A-H and the functions min/max are allowed"
                        )))
                    }
                };
                if !self.eat(&Token::LParen) {
                    return Err(self.err(format!("{name} must be called with parentheses")));
                }
                let a = self.expression()?;
                if !self.eat(&Token::Comma) {
                    return Err(self.err(format!("{name} takes two arguments")));
                }
                let b = self.expression()?;
                if !self.eat(&Token::RParen) {
                    return Err(self.err(String::from("missing closing parenthesis")));
                }
                Ok(Node::Call(func, Box::new(a), Box::new(b)))
            }
            other => Err(self.err(format!("unexpected token {other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str, bytes: &[u8]) -> f64 {
        Formula::parse(src).unwrap().eval(bytes).unwrap()
    }

    #[test]
    fn real_sae_formulas_produce_the_documented_values() {
        // PID 0x0C engine RPM: 1A F8 -> 1726 rpm.
        assert_eq!(eval("(256 * A + B) / 4", &[0x1A, 0xF8]), 1726.0);
        // PID 0x05 coolant temperature: 5A -> 50 degC.
        assert_eq!(eval("A - 40", &[0x5A]), 50.0);
        // PID 0x04 engine load: FF -> 100 %.
        assert_eq!(eval("A * 100 / 255", &[0xFF]), 100.0);
        // PID 0x06 fuel trim: 80 -> 0 %.
        assert_eq!(eval("A / 1.28 - 100", &[0x80]), 0.0);
        // PID 0x42 control module voltage: 39 5C -> 14.684 V.
        assert!((eval("(256 * A + B) / 1000", &[0x39, 0x5C]) - 14.684).abs() < 1e-9);
        // PID 0x0E timing advance: 80 -> 0 deg.
        assert_eq!(eval("A / 2 - 64", &[0x80]), 0.0);
    }

    #[test]
    fn operator_precedence_and_parentheses() {
        assert_eq!(eval("2 + 3 * 4", &[]), 14.0);
        assert_eq!(eval("(2 + 3) * 4", &[]), 20.0);
        assert_eq!(eval("100 - 20 - 30", &[]), 50.0, "left associative");
        assert_eq!(eval("100 / 5 / 2", &[]), 10.0, "left associative");
        assert_eq!(eval("-A + 10", &[3]), 7.0);
        assert_eq!(eval("--5", &[]), 5.0);
    }

    #[test]
    fn modulo_extracts_bitfield_counts() {
        // PID 0x01: DTC count is the low 7 bits of A.
        assert_eq!(eval("A % 128", &[0x83]), 3.0);
        assert_eq!(eval("A % 128", &[0x03]), 3.0);
    }

    #[test]
    fn min_and_max_are_available() {
        assert_eq!(eval("min(A, 100)", &[200]), 100.0);
        assert_eq!(eval("max(A, 100)", &[200]), 200.0);
        assert_eq!(eval("max(min(A, 100), 0)", &[50]), 50.0);
    }

    #[test]
    fn byte_references_map_a_through_h() {
        assert_eq!(eval("A", &[1, 2, 3, 4, 5, 6, 7, 8]), 1.0);
        assert_eq!(eval("H", &[1, 2, 3, 4, 5, 6, 7, 8]), 8.0);
        assert_eq!(Formula::parse("C").unwrap().required_bytes(), 3);
        assert_eq!(Formula::parse("42").unwrap().required_bytes(), 0);
        assert_eq!(Formula::parse("(256*A+B)/4").unwrap().required_bytes(), 2);
    }

    #[test]
    fn short_payloads_are_rejected_rather_than_read_out_of_bounds() {
        let f = Formula::parse("(256 * A + B) / 4").unwrap();
        let err = f.eval(&[0x1A]).unwrap_err();
        assert_eq!(err.code, ErrorCode::DecoderInputInvalid);
        assert!(err.message.contains("needs 2 payload bytes"));
    }

    #[test]
    fn bad_formulas_fail_at_parse_time() {
        for src in
            ["", "  ", "A +", "(A", "A)", "A $ B", "rpm(A)", "Z + 1", "min(A)", "min A, B", "A B"]
        {
            assert!(Formula::parse(src).is_err(), "expected {src:?} to be rejected");
        }
    }

    #[test]
    fn lowercase_identifiers_are_not_byte_references() {
        // 'a' is not a byte reference: it would be a silent wrong answer.
        assert!(Formula::parse("a + 1").is_err());
    }

    #[test]
    fn division_by_zero_is_an_error_not_infinity() {
        let f = Formula::parse("100 / A").unwrap();
        let err = f.eval(&[0]).unwrap_err();
        assert_eq!(err.code, ErrorCode::DecoderInputInvalid);
        let f = Formula::parse("100 % A").unwrap();
        assert!(f.eval(&[0]).is_err());
    }

    #[test]
    fn source_text_is_retained_for_provenance() {
        let f = Formula::parse("(256 * A + B) / 4").unwrap();
        assert_eq!(f.source(), "(256 * A + B) / 4");
    }
}
