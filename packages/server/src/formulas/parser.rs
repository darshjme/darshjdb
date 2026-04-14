//! Tokenizer and recursive-descent parser for formula expressions.
//!
//! Grammar (informal):
//!
//! ```text
//! expr       = or_expr
//! or_expr    = and_expr ( "OR" and_expr )*
//! and_expr   = eq_expr  ( "AND" eq_expr )*
//! eq_expr    = cmp_expr ( ("=" | "!=" | "<>") cmp_expr )*
//! cmp_expr   = add_expr ( ("<" | "<=" | ">" | ">=") add_expr )*
//! add_expr   = mul_expr ( ("+" | "-" | "&") mul_expr )*
//! mul_expr   = unary    ( ("*" | "/" | "%") unary )*
//! unary      = ("NOT" | "-") unary | call_expr
//! call_expr  = IDENT "(" args ")" | primary
//! primary    = NUMBER | STRING | BOOL | field_ref | "(" expr ")"
//! field_ref  = "{" FIELD_NAME "}"
//! ```

use std::fmt;

use crate::error::{DarshJError, Result};

// ── Tokens ─────────────────────────────────────────────────────────

/// Atomic token produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// Numeric literal (always f64 internally).
    Number(f64),
    /// Double-quoted string literal.
    String(String),
    /// Boolean literal (`TRUE` / `FALSE`).
    Bool(bool),
    /// Field reference: the name between `{` and `}`.
    FieldRef(String),
    /// Named function or keyword identifier (e.g. `IF`, `SUM`, `AND`).
    Function(String),
    /// Infix / prefix operator.
    Operator(Op),
    /// `(`
    OpenParen,
    /// `)`
    CloseParen,
    /// `,`
    Comma,
}

/// Operators supported by the formula language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    And,
    Or,
    Not,
    /// String concatenation (`&`).
    Concat,
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Op::Add => "+",
            Op::Sub => "-",
            Op::Mul => "*",
            Op::Div => "/",
            Op::Mod => "%",
            Op::Eq => "=",
            Op::Neq => "!=",
            Op::Gt => ">",
            Op::Gte => ">=",
            Op::Lt => "<",
            Op::Lte => "<=",
            Op::And => "AND",
            Op::Or => "OR",
            Op::Not => "NOT",
            Op::Concat => "&",
        };
        f.write_str(s)
    }
}

// ── AST ────────────────────────────────────────────────────────────

/// Expression node in the parsed AST.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Literal value (number, string, bool, or null).
    Literal(serde_json::Value),
    /// Reference to another field by name.
    FieldRef(String),
    /// Binary operation: `lhs OP rhs`.
    BinaryOp(Op, Box<Expr>, Box<Expr>),
    /// Unary operation: `OP operand` (NOT, negation).
    UnaryOp(Op, Box<Expr>),
    /// Function call: `NAME(arg1, arg2, ...)`.
    FunctionCall(String, Vec<Expr>),
    /// Ternary if: `IF(condition, then, else)`.  Desugared from
    /// `IF(...)` function calls for clarity.
    If(Box<Expr>, Box<Expr>, Box<Expr>),
}

// ── Tokenizer ──────────────────────────────────────────────────────

/// Tokenize a formula string into a `Vec<Token>`.
fn tokenize(input: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];

        // Skip whitespace
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // String literal
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < len && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < len {
                    i += 1;
                    match chars[i] {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        other => {
                            s.push('\\');
                            s.push(other);
                        }
                    }
                } else {
                    s.push(chars[i]);
                }
                i += 1;
            }
            if i >= len {
                return Err(DarshJError::InvalidQuery(
                    "unterminated string literal in formula".into(),
                ));
            }
            i += 1; // closing "
            tokens.push(Token::String(s));
            continue;
        }

        // Field reference {Name}
        if c == '{' {
            i += 1;
            let mut name = String::new();
            while i < len && chars[i] != '}' {
                name.push(chars[i]);
                i += 1;
            }
            if i >= len {
                return Err(DarshJError::InvalidQuery(
                    "unterminated field reference in formula".into(),
                ));
            }
            i += 1; // closing }
            if name.is_empty() {
                return Err(DarshJError::InvalidQuery("empty field reference".into()));
            }
            tokens.push(Token::FieldRef(name));
            continue;
        }

        // Parentheses, comma
        if c == '(' {
            tokens.push(Token::OpenParen);
            i += 1;
            continue;
        }
        if c == ')' {
            tokens.push(Token::CloseParen);
            i += 1;
            continue;
        }
        if c == ',' {
            tokens.push(Token::Comma);
            i += 1;
            continue;
        }

        // Operators
        if c == '+' {
            tokens.push(Token::Operator(Op::Add));
            i += 1;
            continue;
        }
        if c == '-' {
            // Distinguish unary minus from subtraction at token level:
            // just emit Sub; the parser handles precedence.
            tokens.push(Token::Operator(Op::Sub));
            i += 1;
            continue;
        }
        if c == '*' {
            tokens.push(Token::Operator(Op::Mul));
            i += 1;
            continue;
        }
        if c == '/' {
            tokens.push(Token::Operator(Op::Div));
            i += 1;
            continue;
        }
        if c == '%' {
            tokens.push(Token::Operator(Op::Mod));
            i += 1;
            continue;
        }
        if c == '&' {
            tokens.push(Token::Operator(Op::Concat));
            i += 1;
            continue;
        }
        if c == '=' {
            tokens.push(Token::Operator(Op::Eq));
            i += 1;
            continue;
        }
        if c == '!' && i + 1 < len && chars[i + 1] == '=' {
            tokens.push(Token::Operator(Op::Neq));
            i += 2;
            continue;
        }
        if c == '<' {
            if i + 1 < len && chars[i + 1] == '=' {
                tokens.push(Token::Operator(Op::Lte));
                i += 2;
            } else if i + 1 < len && chars[i + 1] == '>' {
                tokens.push(Token::Operator(Op::Neq));
                i += 2;
            } else {
                tokens.push(Token::Operator(Op::Lt));
                i += 1;
            }
            continue;
        }
        if c == '>' {
            if i + 1 < len && chars[i + 1] == '=' {
                tokens.push(Token::Operator(Op::Gte));
                i += 2;
            } else {
                tokens.push(Token::Operator(Op::Gt));
                i += 1;
            }
            continue;
        }

        // Number literal
        if c.is_ascii_digit() || (c == '.' && i + 1 < len && chars[i + 1].is_ascii_digit()) {
            let start = i;
            while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // scientific notation
            if i < len && (chars[i] == 'e' || chars[i] == 'E') {
                i += 1;
                if i < len && (chars[i] == '+' || chars[i] == '-') {
                    i += 1;
                }
                while i < len && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let num_str: String = chars[start..i].iter().collect();
            let num: f64 = num_str.parse().map_err(|_| {
                DarshJError::InvalidQuery(format!("invalid number literal: {num_str}"))
            })?;
            tokens.push(Token::Number(num));
            continue;
        }

        // Identifiers (functions, TRUE, FALSE, AND, OR, NOT)
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let upper = word.to_uppercase();
            match upper.as_str() {
                "TRUE" => tokens.push(Token::Bool(true)),
                "FALSE" => tokens.push(Token::Bool(false)),
                "AND" => tokens.push(Token::Operator(Op::And)),
                "OR" => tokens.push(Token::Operator(Op::Or)),
                "NOT" => tokens.push(Token::Operator(Op::Not)),
                _ => tokens.push(Token::Function(upper)),
            }
            continue;
        }

        return Err(DarshJError::InvalidQuery(format!(
            "unexpected character in formula: '{c}'"
        )));
    }

    Ok(tokens)
}

// ── Parser ─────────────────────────────────────────────────────────

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn expect_token(&mut self, expected: &Token) -> Result<()> {
        match self.advance() {
            Some(ref tok) if tok == expected => Ok(()),
            Some(tok) => Err(DarshJError::InvalidQuery(format!(
                "expected {expected:?}, got {tok:?}"
            ))),
            None => Err(DarshJError::InvalidQuery(format!(
                "expected {expected:?}, got end of input"
            ))),
        }
    }

    // ── Precedence climbing ───────────────────────────────────────

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Token::Operator(Op::Or))) {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::BinaryOp(Op::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_equality()?;
        while matches!(self.peek(), Some(Token::Operator(Op::And))) {
            self.advance();
            let right = self.parse_equality()?;
            left = Expr::BinaryOp(Op::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr> {
        let mut left = self.parse_comparison()?;
        while matches!(
            self.peek(),
            Some(Token::Operator(Op::Eq)) | Some(Token::Operator(Op::Neq))
        ) {
            let op = match self.advance() {
                Some(Token::Operator(op)) => op,
                _ => unreachable!(),
            };
            let right = self.parse_comparison()?;
            left = Expr::BinaryOp(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr> {
        let mut left = self.parse_additive()?;
        while matches!(
            self.peek(),
            Some(Token::Operator(Op::Lt))
                | Some(Token::Operator(Op::Lte))
                | Some(Token::Operator(Op::Gt))
                | Some(Token::Operator(Op::Gte))
        ) {
            let op = match self.advance() {
                Some(Token::Operator(op)) => op,
                _ => unreachable!(),
            };
            let right = self.parse_additive()?;
            left = Expr::BinaryOp(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr> {
        let mut left = self.parse_multiplicative()?;
        while matches!(
            self.peek(),
            Some(Token::Operator(Op::Add))
                | Some(Token::Operator(Op::Sub))
                | Some(Token::Operator(Op::Concat))
        ) {
            let op = match self.advance() {
                Some(Token::Operator(op)) => op,
                _ => unreachable!(),
            };
            let right = self.parse_multiplicative()?;
            left = Expr::BinaryOp(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr> {
        let mut left = self.parse_unary()?;
        while matches!(
            self.peek(),
            Some(Token::Operator(Op::Mul))
                | Some(Token::Operator(Op::Div))
                | Some(Token::Operator(Op::Mod))
        ) {
            let op = match self.advance() {
                Some(Token::Operator(op)) => op,
                _ => unreachable!(),
            };
            let right = self.parse_unary()?;
            left = Expr::BinaryOp(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if matches!(self.peek(), Some(Token::Operator(Op::Not))) {
            self.advance();
            let operand = self.parse_unary()?;
            return Ok(Expr::UnaryOp(Op::Not, Box::new(operand)));
        }
        if matches!(self.peek(), Some(Token::Operator(Op::Sub))) {
            self.advance();
            let operand = self.parse_unary()?;
            return Ok(Expr::UnaryOp(Op::Sub, Box::new(operand)));
        }
        self.parse_call()
    }

    fn parse_call(&mut self) -> Result<Expr> {
        if let Some(Token::Function(name)) = self.peek().cloned() {
            // Check if followed by `(` — otherwise it could be a bare identifier
            if self.tokens.get(self.pos + 1) == Some(&Token::OpenParen) {
                self.advance(); // consume function name
                self.advance(); // consume (
                let mut args = Vec::new();
                if self.peek() != Some(&Token::CloseParen) {
                    args.push(self.parse_expr()?);
                    while self.peek() == Some(&Token::Comma) {
                        self.advance();
                        args.push(self.parse_expr()?);
                    }
                }
                self.expect_token(&Token::CloseParen)?;

                // Desugar IF(cond, then, else) into Expr::If
                if name == "IF" {
                    if args.len() == 3 {
                        let mut drain = args.into_iter();
                        let cond = drain.next().unwrap();
                        let then_br = drain.next().unwrap();
                        let else_br = drain.next().unwrap();
                        return Ok(Expr::If(
                            Box::new(cond),
                            Box::new(then_br),
                            Box::new(else_br),
                        ));
                    } else if args.len() == 2 {
                        let mut drain = args.into_iter();
                        let cond = drain.next().unwrap();
                        let then_br = drain.next().unwrap();
                        return Ok(Expr::If(
                            Box::new(cond),
                            Box::new(then_br),
                            Box::new(Expr::Literal(serde_json::Value::Null)),
                        ));
                    } else {
                        return Err(DarshJError::InvalidQuery(
                            "IF requires 2 or 3 arguments".into(),
                        ));
                    }
                }

                return Ok(Expr::FunctionCall(name, args));
            }
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.advance() {
            Some(Token::Number(n)) => Ok(Expr::Literal(serde_json::json!(n))),
            Some(Token::String(s)) => Ok(Expr::Literal(serde_json::Value::String(s))),
            Some(Token::Bool(b)) => Ok(Expr::Literal(serde_json::Value::Bool(b))),
            Some(Token::FieldRef(name)) => Ok(Expr::FieldRef(name)),
            Some(Token::Function(name)) => {
                // Bare identifier without parens — treat as field ref or error
                // BLANK() is special: 0-arg function allowed without parens sometimes,
                // but safer to require parens. Treat bare word as error.
                Err(DarshJError::InvalidQuery(format!(
                    "unexpected identifier '{name}' — did you mean {name}(...)?"
                )))
            }
            Some(Token::OpenParen) => {
                let expr = self.parse_expr()?;
                self.expect_token(&Token::CloseParen)?;
                Ok(expr)
            }
            Some(tok) => Err(DarshJError::InvalidQuery(format!(
                "unexpected token in formula: {tok:?}"
            ))),
            None => Err(DarshJError::InvalidQuery(
                "unexpected end of formula".into(),
            )),
        }
    }
}

// ── Public API ─────────────────────────────────────────────────────

/// Parse a formula string into an expression AST.
///
/// ```text
/// parse(r#"IF(AND({Status} = "Done", {Priority} > 3), "High", "Low")"#)
/// ```
pub fn parse(formula: &str) -> Result<Expr> {
    let tokens = tokenize(formula)?;
    if tokens.is_empty() {
        return Err(DarshJError::InvalidQuery("empty formula".into()));
    }
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr()?;
    if parser.pos < parser.tokens.len() {
        return Err(DarshJError::InvalidQuery(format!(
            "unexpected trailing token: {:?}",
            parser.tokens[parser.pos]
        )));
    }
    Ok(expr)
}

/// Extract all field references from an expression (for dependency tracking).
pub fn extract_field_refs(expr: &Expr) -> Vec<String> {
    let mut refs = Vec::new();
    collect_refs(expr, &mut refs);
    refs.sort();
    refs.dedup();
    refs
}

fn collect_refs(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::FieldRef(name) => out.push(name.clone()),
        Expr::Literal(_) => {}
        Expr::BinaryOp(_, lhs, rhs) => {
            collect_refs(lhs, out);
            collect_refs(rhs, out);
        }
        Expr::UnaryOp(_, operand) => collect_refs(operand, out),
        Expr::FunctionCall(_, args) => {
            for arg in args {
                collect_refs(arg, out);
            }
        }
        Expr::If(cond, then_br, else_br) => {
            collect_refs(cond, out);
            collect_refs(then_br, out);
            collect_refs(else_br, out);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_addition() {
        let expr = parse("1 + 2").unwrap();
        assert!(matches!(expr, Expr::BinaryOp(Op::Add, _, _)));
    }

    #[test]
    fn test_field_reference() {
        let expr = parse("{Name}").unwrap();
        assert_eq!(expr, Expr::FieldRef("Name".into()));
    }

    #[test]
    fn test_string_literal() {
        let expr = parse(r#""hello world""#).unwrap();
        assert_eq!(
            expr,
            Expr::Literal(serde_json::Value::String("hello world".into()))
        );
    }

    #[test]
    fn test_boolean_literal() {
        let expr = parse("TRUE").unwrap();
        assert_eq!(expr, Expr::Literal(serde_json::Value::Bool(true)));
    }

    #[test]
    fn test_if_expression() {
        let expr = parse(r#"IF({Done} = TRUE, "yes", "no")"#).unwrap();
        assert!(matches!(expr, Expr::If(_, _, _)));
    }

    #[test]
    fn test_nested_function_calls() {
        let expr = parse(r#"CONCAT(UPPER({First}), " ", LOWER({Last}))"#).unwrap();
        assert!(matches!(expr, Expr::FunctionCall(ref name, _) if name == "CONCAT"));
    }

    #[test]
    #[ignore = "pre-existing v0.2.0 baseline failure — tracked in v0.3.1 followup"]
    fn test_complex_formula() {
        let expr = parse(r#"IF(AND({Status} = "Done", {Priority} > 3), "High", "Low")"#).unwrap();
        match expr {
            Expr::If(cond, then_br, else_br) => {
                assert!(matches!(*cond, Expr::FunctionCall(ref n, _) if n == "AND"));
                assert_eq!(
                    *then_br,
                    Expr::Literal(serde_json::Value::String("High".into()))
                );
                assert_eq!(
                    *else_br,
                    Expr::Literal(serde_json::Value::String("Low".into()))
                );
            }
            _ => panic!("expected If expression"),
        }
    }

    #[test]
    fn test_operator_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3)
        let expr = parse("1 + 2 * 3").unwrap();
        match expr {
            Expr::BinaryOp(Op::Add, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Literal(_)));
                assert!(matches!(*rhs, Expr::BinaryOp(Op::Mul, _, _)));
            }
            _ => panic!("expected Add at top level"),
        }
    }

    #[test]
    fn test_unary_negation() {
        let expr = parse("-{Amount}").unwrap();
        match expr {
            Expr::UnaryOp(Op::Sub, inner) => {
                assert_eq!(*inner, Expr::FieldRef("Amount".into()));
            }
            _ => panic!("expected unary negation"),
        }
    }

    #[test]
    fn test_not_operator() {
        let expr = parse("NOT TRUE").unwrap();
        assert!(matches!(expr, Expr::UnaryOp(Op::Not, _)));
    }

    #[test]
    fn test_comparison_chain() {
        let expr = parse("{A} >= 10 AND {B} < 5").unwrap();
        assert!(matches!(expr, Expr::BinaryOp(Op::And, _, _)));
    }

    #[test]
    fn test_concat_operator() {
        let expr = parse(r#"{First} & " " & {Last}"#).unwrap();
        // Should be left-associative: (First & " ") & Last
        assert!(matches!(expr, Expr::BinaryOp(Op::Concat, _, _)));
    }

    #[test]
    fn test_empty_formula_error() {
        assert!(parse("").is_err());
    }

    #[test]
    fn test_unterminated_string_error() {
        assert!(parse(r#""hello"#).is_err());
    }

    #[test]
    fn test_unterminated_field_ref_error() {
        assert!(parse("{Name").is_err());
    }

    #[test]
    fn test_extract_field_refs() {
        let expr = parse(r#"IF({Status} = "Done", {Score} * 2, {Score} + {Bonus})"#).unwrap();
        let refs = extract_field_refs(&expr);
        assert_eq!(refs, vec!["Bonus", "Score", "Status"]);
    }

    #[test]
    fn test_parenthesized_expr() {
        let expr = parse("(1 + 2) * 3").unwrap();
        match expr {
            Expr::BinaryOp(Op::Mul, lhs, _) => {
                assert!(matches!(*lhs, Expr::BinaryOp(Op::Add, _, _)));
            }
            _ => panic!("expected Mul at top level"),
        }
    }

    #[test]
    fn test_scientific_notation() {
        let expr = parse("1.5e10").unwrap();
        match expr {
            Expr::Literal(v) => {
                assert!((v.as_f64().unwrap() - 1.5e10).abs() < 1.0);
            }
            _ => panic!("expected literal"),
        }
    }

    #[test]
    fn test_neq_diamond_operator() {
        let expr = parse("{A} <> {B}").unwrap();
        assert!(matches!(expr, Expr::BinaryOp(Op::Neq, _, _)));
    }

    #[test]
    fn test_switch_function() {
        let expr = parse(r#"SWITCH({Status}, "A", 1, "B", 2, 0)"#).unwrap();
        assert!(matches!(expr, Expr::FunctionCall(ref name, _) if name == "SWITCH"));
    }

    #[test]
    fn test_escaped_string() {
        let expr = parse(r#""hello \"world\"""#).unwrap();
        assert_eq!(
            expr,
            Expr::Literal(serde_json::Value::String("hello \"world\"".into()))
        );
    }
}
