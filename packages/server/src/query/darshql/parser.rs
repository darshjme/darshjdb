//! Hand-written recursive descent parser for DarshQL.
//!
//! Tokenizes the input string and then parses it into an AST.
//! No external parser libraries (pest, nom) — just clean Rust.

use crate::error::DarshJError;

use super::ast::*;

// ── Token ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    // Keywords
    Select,
    From,
    Where,
    OrderBy,
    Limit,
    Start,
    Create,
    Set,
    Content,
    Update,
    Delete,
    Insert,
    Into,
    Values,
    Relate,
    Live,
    Define,
    Table,
    Field,
    On,
    Type,
    Default,
    Assert,
    Info,
    For,
    Db,
    And,
    Or,
    As,
    Asc,
    Desc,
    Schemafull,
    Schemaless,
    Drop,
    GroupBy,
    Fetch,
    Is,
    Not,
    Null,
    True,
    False,
    Like,
    Contains,
    Since,

    // Symbols
    Star,         // *
    Comma,        // ,
    Dot,          // .
    LParen,       // (
    RParen,       // )
    LBrace,       // {
    RBrace,       // }
    Eq,           // =
    Neq,          // !=
    Gt,           // >
    Gte,          // >=
    Lt,           // <
    Lte,          // <=
    Arrow,        // ->
    BackArrow,    // <-
    Semicolon,    // ;
    Colon,        // :
    LAngle,       // < (when used for type cast)

    // Literals
    Ident(String),
    StringLit(String),
    NumberLit(f64),
    IntLit(i64),

    Eof,
}

// ── Lexer ──────────────────────────────────────────────────────────

struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

impl Lexer {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        self.pos += 1;
        c
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.advance();
            } else if c == '-' && self.chars.get(self.pos + 1) == Some(&'-') {
                // Line comment
                while let Some(c) = self.advance() {
                    if c == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    fn tokenize(&mut self) -> Result<Vec<Token>, DarshJError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace();
            match self.peek() {
                None => {
                    tokens.push(Token::Eof);
                    break;
                }
                Some(c) => {
                    let tok = match c {
                        '*' => {
                            self.advance();
                            Token::Star
                        }
                        ',' => {
                            self.advance();
                            Token::Comma
                        }
                        '.' => {
                            self.advance();
                            Token::Dot
                        }
                        '(' => {
                            self.advance();
                            Token::LParen
                        }
                        ')' => {
                            self.advance();
                            Token::RParen
                        }
                        '{' => {
                            self.advance();
                            Token::LBrace
                        }
                        '}' => {
                            self.advance();
                            Token::RBrace
                        }
                        ';' => {
                            self.advance();
                            Token::Semicolon
                        }
                        ':' => {
                            self.advance();
                            Token::Colon
                        }
                        '=' => {
                            self.advance();
                            Token::Eq
                        }
                        '!' => {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                Token::Neq
                            } else {
                                return Err(DarshJError::InvalidQuery(
                                    "unexpected '!' — did you mean '!='?".into(),
                                ));
                            }
                        }
                        '>' => {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                Token::Gte
                            } else {
                                Token::Gt
                            }
                        }
                        '-' if self.chars.get(self.pos + 1) == Some(&'>') => {
                            self.advance();
                            self.advance();
                            Token::Arrow
                        }
                        '<' => {
                            self.advance();
                            if self.peek() == Some('-') {
                                self.advance();
                                Token::BackArrow
                            } else if self.peek() == Some('=') {
                                self.advance();
                                Token::Lte
                            } else {
                                Token::Lt
                            }
                        }
                        '\'' | '"' => self.lex_string()?,
                        c if c.is_ascii_digit() || (c == '-' && self.is_number_ahead()) => {
                            self.lex_number()?
                        }
                        c if c.is_alphabetic() || c == '_' || c == '$' => self.lex_ident_or_kw(),
                        _ => {
                            return Err(DarshJError::InvalidQuery(format!(
                                "unexpected character: '{c}'"
                            )));
                        }
                    };
                    tokens.push(tok);
                }
            }
        }
        Ok(tokens)
    }

    fn is_number_ahead(&self) -> bool {
        self.chars
            .get(self.pos + 1)
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
    }

    fn lex_string(&mut self) -> Result<Token, DarshJError> {
        let quote = self.advance().unwrap();
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    return Err(DarshJError::InvalidQuery("unterminated string".into()));
                }
                Some(c) if c == quote => break,
                Some('\\') => match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('\\') => s.push('\\'),
                    Some(q) if q == quote => s.push(q),
                    Some(other) => {
                        s.push('\\');
                        s.push(other);
                    }
                    None => return Err(DarshJError::InvalidQuery("unterminated escape".into())),
                },
                Some(c) => s.push(c),
            }
        }
        Ok(Token::StringLit(s))
    }

    fn lex_number(&mut self) -> Result<Token, DarshJError> {
        let mut s = String::new();
        let mut is_float = false;

        if self.peek() == Some('-') {
            s.push('-');
            self.advance();
        }

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                s.push(c);
                self.advance();
            } else if c == '.' && !is_float {
                is_float = true;
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }

        if is_float {
            let v: f64 = s
                .parse()
                .map_err(|_| DarshJError::InvalidQuery(format!("invalid number: {s}")))?;
            Ok(Token::NumberLit(v))
        } else {
            let v: i64 = s
                .parse()
                .map_err(|_| DarshJError::InvalidQuery(format!("invalid integer: {s}")))?;
            Ok(Token::IntLit(v))
        }
    }

    fn lex_ident_or_kw(&mut self) -> Token {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '$' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        match s.to_uppercase().as_str() {
            "SELECT" => Token::Select,
            "FROM" => Token::From,
            "WHERE" => Token::Where,
            "ORDER" => Token::OrderBy, // we'll consume "BY" in parser
            "LIMIT" => Token::Limit,
            "START" => Token::Start,
            "CREATE" => Token::Create,
            "SET" => Token::Set,
            "CONTENT" => Token::Content,
            "UPDATE" => Token::Update,
            "DELETE" => Token::Delete,
            "INSERT" => Token::Insert,
            "INTO" => Token::Into,
            "VALUES" => Token::Values,
            "RELATE" => Token::Relate,
            "LIVE" => Token::Live,
            "DEFINE" => Token::Define,
            "TABLE" => Token::Table,
            "FIELD" => Token::Field,
            "ON" => Token::On,
            "TYPE" => Token::Type,
            "DEFAULT" => Token::Default,
            "ASSERT" => Token::Assert,
            "INFO" => Token::Info,
            "FOR" => Token::For,
            "DB" => Token::Db,
            "AND" => Token::And,
            "OR" => Token::Or,
            "AS" => Token::As,
            "ASC" => Token::Asc,
            "DESC" => Token::Desc,
            "SCHEMAFULL" => Token::Schemafull,
            "SCHEMALESS" => Token::Schemaless,
            "DROP" => Token::Drop,
            "GROUP" => Token::GroupBy, // we'll consume "BY" in parser
            "FETCH" => Token::Fetch,
            "IS" => Token::Is,
            "NOT" => Token::Not,
            "NULL" => Token::Null,
            "TRUE" => Token::True,
            "FALSE" => Token::False,
            "LIKE" => Token::Like,
            "CONTAINS" => Token::Contains,
            "SINCE" => Token::Since,
            "BY" => Token::Ident("BY".into()), // consumed as part of ORDER BY / GROUP BY
            _ => Token::Ident(s),
        }
    }
}

// ── Parser ─────────────────────────────────────────────────────────

/// Recursive descent parser for DarshQL.
pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    /// Parse a DarshQL string into a list of statements.
    pub fn parse(input: &str) -> Result<Vec<Statement>, DarshJError> {
        let mut lexer = Lexer::new(input);
        let tokens = lexer.tokenize()?;
        let mut parser = Self { tokens, pos: 0 };
        let mut stmts = Vec::new();

        while !parser.is_at_end() {
            stmts.push(parser.parse_statement()?);
            // Optional semicolon between statements.
            parser.eat(Token::Semicolon);
        }

        Ok(stmts)
    }

    // ── Helpers ────────────────────────────────────────────────────

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        tok
    }

    fn eat(&mut self, expected: Token) -> bool {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(&expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: Token) -> Result<Token, DarshJError> {
        let tok = self.advance();
        if std::mem::discriminant(&tok) == std::mem::discriminant(&expected) {
            Ok(tok)
        } else {
            Err(DarshJError::InvalidQuery(format!(
                "expected {expected:?}, got {tok:?}"
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<String, DarshJError> {
        match self.advance() {
            Token::Ident(s) => Ok(s),
            // Allow some keywords to be used as identifiers in certain positions.
            Token::Table => Ok("table".into()),
            Token::Type => Ok("type".into()),
            Token::Set => Ok("set".into()),
            Token::Default => Ok("default".into()),
            Token::Since => Ok("since".into()),
            Token::Db => Ok("db".into()),
            other => Err(DarshJError::InvalidQuery(format!(
                "expected identifier, got {other:?}"
            ))),
        }
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }

    /// Consume "BY" after ORDER/GROUP keywords.
    fn eat_by(&mut self) {
        if let Token::Ident(s) = self.peek() {
            if s.to_uppercase() == "BY" {
                self.advance();
            }
        }
    }

    // ── Statement dispatch ─────────────────────────────────────────

    fn parse_statement(&mut self) -> Result<Statement, DarshJError> {
        match self.peek().clone() {
            Token::Select => self.parse_select().map(Statement::Select),
            Token::Create => self.parse_create().map(Statement::Create),
            Token::Update => self.parse_update().map(Statement::Update),
            Token::Delete => self.parse_delete().map(Statement::Delete),
            Token::Insert => self.parse_insert().map(Statement::Insert),
            Token::Relate => self.parse_relate().map(Statement::Relate),
            Token::Live => self.parse_live_select().map(Statement::LiveSelect),
            Token::Define => self.parse_define(),
            Token::Info => self.parse_info().map(Statement::InfoFor),
            other => Err(DarshJError::InvalidQuery(format!(
                "unexpected token at statement start: {other:?}"
            ))),
        }
    }

    // ── SELECT ─────────────────────────────────────────────────────

    fn parse_select(&mut self) -> Result<SelectStatement, DarshJError> {
        self.expect(Token::Select)?;

        let fields = self.parse_field_list()?;

        self.expect(Token::From)?;
        let from = self.parse_target()?;

        let condition = if self.eat(Token::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        let group_by = if self.eat(Token::GroupBy) {
            self.eat_by();
            self.parse_ident_list()?
        } else {
            Vec::new()
        };

        let order = if self.eat(Token::OrderBy) {
            self.eat_by();
            self.parse_order_by_list()?
        } else {
            Vec::new()
        };

        let limit = if self.eat(Token::Limit) {
            Some(self.parse_u64()?)
        } else {
            None
        };

        let start = if self.eat(Token::Start) {
            Some(self.parse_u64()?)
        } else {
            None
        };

        let fetch = if self.eat(Token::Fetch) {
            self.parse_ident_list()?
        } else {
            Vec::new()
        };

        Ok(SelectStatement {
            fields,
            from,
            condition,
            order,
            limit,
            start,
            group_by,
            fetch,
        })
    }

    // ── CREATE ─────────────────────────────────────────────────────

    fn parse_create(&mut self) -> Result<CreateStatement, DarshJError> {
        self.expect(Token::Create)?;
        let target = self.parse_target()?;
        let data = self.parse_set_or_content()?;
        Ok(CreateStatement { target, data })
    }

    // ── UPDATE ─────────────────────────────────────────────────────

    fn parse_update(&mut self) -> Result<UpdateStatement, DarshJError> {
        self.expect(Token::Update)?;
        let target = self.parse_target()?;
        let data = self.parse_set_or_content()?;
        let condition = if self.eat(Token::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(UpdateStatement {
            target,
            data,
            condition,
        })
    }

    // ── DELETE ─────────────────────────────────────────────────────

    fn parse_delete(&mut self) -> Result<DeleteStatement, DarshJError> {
        self.expect(Token::Delete)?;
        let target = self.parse_target()?;
        let condition = if self.eat(Token::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(DeleteStatement { target, condition })
    }

    // ── INSERT ─────────────────────────────────────────────────────

    fn parse_insert(&mut self) -> Result<InsertStatement, DarshJError> {
        self.expect(Token::Insert)?;
        self.expect(Token::Into)?;
        let table = self.expect_ident()?;

        self.expect(Token::LParen)?;
        let fields = self.parse_ident_list()?;
        self.expect(Token::RParen)?;

        self.expect(Token::Values)?;
        let mut values = Vec::new();
        loop {
            self.expect(Token::LParen)?;
            let mut row = Vec::new();
            loop {
                row.push(self.parse_expr()?);
                if !self.eat(Token::Comma) {
                    break;
                }
            }
            self.expect(Token::RParen)?;
            values.push(row);
            if !self.eat(Token::Comma) {
                break;
            }
        }

        Ok(InsertStatement {
            table,
            fields,
            values,
        })
    }

    // ── RELATE ─────────────────────────────────────────────────────

    fn parse_relate(&mut self) -> Result<RelateStatement, DarshJError> {
        self.expect(Token::Relate)?;
        let from = self.parse_record_id()?;
        self.expect(Token::Arrow)?;
        let edge = self.expect_ident()?;
        self.expect(Token::Arrow)?;
        let to = self.parse_record_id()?;

        let data = if self.eat(Token::Set) {
            Some(SetOrContent::Set(self.parse_set_pairs()?))
        } else if self.eat(Token::Content) {
            Some(self.parse_content_json()?)
        } else {
            None
        };

        Ok(RelateStatement {
            from,
            edge,
            to,
            data,
        })
    }

    // ── LIVE SELECT ────────────────────────────────────────────────

    fn parse_live_select(&mut self) -> Result<LiveSelectStatement, DarshJError> {
        self.expect(Token::Live)?;
        self.expect(Token::Select)?;

        let fields = self.parse_field_list()?;
        self.expect(Token::From)?;
        let from = self.parse_target()?;

        let condition = if self.eat(Token::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        Ok(LiveSelectStatement {
            fields,
            from,
            condition,
        })
    }

    // ── DEFINE ─────────────────────────────────────────────────────

    fn parse_define(&mut self) -> Result<Statement, DarshJError> {
        self.expect(Token::Define)?;
        match self.peek().clone() {
            Token::Table => self.parse_define_table().map(Statement::DefineTable),
            Token::Field => self.parse_define_field().map(Statement::DefineField),
            other => Err(DarshJError::InvalidQuery(format!(
                "expected TABLE or FIELD after DEFINE, got {other:?}"
            ))),
        }
    }

    fn parse_define_table(&mut self) -> Result<DefineTableStatement, DarshJError> {
        self.expect(Token::Table)?;
        let name = self.expect_ident()?;

        let mut schema_mode = SchemaMode::default();
        let mut drop = false;

        // Parse optional modifiers in any order.
        loop {
            match self.peek() {
                Token::Schemafull => {
                    self.advance();
                    schema_mode = SchemaMode::Schemafull;
                }
                Token::Schemaless => {
                    self.advance();
                    schema_mode = SchemaMode::Schemaless;
                }
                Token::Drop => {
                    self.advance();
                    drop = true;
                }
                _ => break,
            }
        }

        Ok(DefineTableStatement {
            name,
            schema_mode,
            drop,
        })
    }

    fn parse_define_field(&mut self) -> Result<DefineFieldStatement, DarshJError> {
        self.expect(Token::Field)?;
        let name = self.expect_ident()?;

        self.expect(Token::On)?;
        // Optional TABLE keyword.
        self.eat(Token::Table);
        let table = self.expect_ident()?;

        let field_type = if self.eat(Token::Type) {
            let type_name = self.expect_ident()?;
            DarshType::from_str(&type_name.to_lowercase())
        } else {
            None
        };

        let default = if self.eat(Token::Default) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        let assert = if self.eat(Token::Assert) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        Ok(DefineFieldStatement {
            name,
            table,
            field_type,
            default,
            assert,
        })
    }

    // ── INFO FOR ───────────────────────────────────────────────────

    fn parse_info(&mut self) -> Result<InfoForStatement, DarshJError> {
        self.expect(Token::Info)?;
        self.expect(Token::For)?;

        let target = if self.eat(Token::Db) {
            InfoTarget::Db
        } else if self.eat(Token::Table) {
            let name = self.expect_ident()?;
            InfoTarget::Table(name)
        } else {
            let name = self.expect_ident()?;
            // Accept bare table name without TABLE keyword.
            InfoTarget::Table(name)
        };

        Ok(InfoForStatement { target })
    }

    // ── Shared sub-parsers ─────────────────────────────────────────

    fn parse_target(&mut self) -> Result<Target, DarshJError> {
        let name = self.expect_ident()?;
        if self.eat(Token::Colon) {
            let id = self.parse_record_id_value()?;
            Ok(Target::Record(RecordId { table: name, id }))
        } else {
            Ok(Target::Table(name))
        }
    }

    fn parse_record_id(&mut self) -> Result<RecordId, DarshJError> {
        let table = self.expect_ident()?;
        self.expect(Token::Colon)?;
        let id = self.parse_record_id_value()?;
        Ok(RecordId { table, id })
    }

    /// Parse the value part of a record id — can be an ident, string, or number.
    fn parse_record_id_value(&mut self) -> Result<String, DarshJError> {
        match self.advance() {
            Token::Ident(s) => Ok(s),
            Token::StringLit(s) => Ok(s),
            Token::IntLit(n) => Ok(n.to_string()),
            Token::NumberLit(n) => Ok(n.to_string()),
            other => Err(DarshJError::InvalidQuery(format!(
                "expected record id value, got {other:?}"
            ))),
        }
    }

    fn parse_field_list(&mut self) -> Result<Vec<Field>, DarshJError> {
        let mut fields = Vec::new();
        loop {
            fields.push(self.parse_field()?);
            if !self.eat(Token::Comma) {
                break;
            }
        }
        Ok(fields)
    }

    fn parse_field(&mut self) -> Result<Field, DarshJError> {
        // Type cast: <type>field
        if matches!(self.peek(), Token::Lt) {
            // Check if this looks like a type cast: <ident>
            let saved = self.pos;
            self.advance(); // consume <
            if let Token::Ident(type_name) = self.peek().clone() {
                self.advance(); // consume type name
                if self.eat(Token::Gt) {
                    // It's a type cast.
                    if let Some(dt) = DarshType::from_str(&type_name.to_lowercase()) {
                        let inner = self.parse_field()?;
                        let field = Field::Cast {
                            cast_type: dt,
                            expr: Box::new(inner),
                        };
                        return self.maybe_alias(field);
                    }
                }
            }
            // Not a type cast, restore position.
            self.pos = saved;
        }

        // Star
        if matches!(self.peek(), Token::Star) {
            self.advance();
            return Ok(Field::All);
        }

        // Graph traversal: ->edge->edge or <-edge
        if matches!(self.peek(), Token::Arrow | Token::BackArrow) {
            let traversal = self.parse_graph_traversal()?;
            return self.maybe_alias(Field::Graph(traversal));
        }

        // Ident — could be function call or plain field.
        let name = self.expect_ident()?;

        // Function call: name(...)
        if matches!(self.peek(), Token::LParen) {
            self.advance(); // consume (
            let mut args = Vec::new();
            if !matches!(self.peek(), Token::RParen) {
                loop {
                    // Args can be graph traversals or fields.
                    if matches!(self.peek(), Token::Arrow | Token::BackArrow) {
                        let trav = self.parse_graph_traversal()?;
                        args.push(Field::Graph(trav));
                    } else if matches!(self.peek(), Token::Star) {
                        self.advance();
                        args.push(Field::All);
                    } else {
                        let ident = self.expect_ident()?;
                        args.push(Field::Attribute(ident));
                    }
                    if !self.eat(Token::Comma) {
                        break;
                    }
                }
            }
            self.expect(Token::RParen)?;

            // Must have alias: AS alias
            if self.eat(Token::As) {
                let alias = self.expect_ident()?;
                return Ok(Field::Computed {
                    func: name,
                    args,
                    alias,
                });
            }
            // Auto-generate alias from function name.
            let alias = format!("{name}_result");
            return Ok(Field::Computed {
                func: name,
                args,
                alias,
            });
        }

        let field = Field::Attribute(name);
        self.maybe_alias(field)
    }

    fn maybe_alias(&mut self, field: Field) -> Result<Field, DarshJError> {
        // For casts and graphs, allow AS alias to become Computed.
        if self.eat(Token::As) {
            let alias = self.expect_ident()?;
            match field {
                Field::Cast { cast_type, expr } => Ok(Field::Computed {
                    func: format!("cast_{:?}", cast_type).to_lowercase(),
                    args: vec![*expr],
                    alias,
                }),
                Field::Graph(trav) => Ok(Field::Computed {
                    func: "graph".into(),
                    args: vec![Field::Graph(trav)],
                    alias,
                }),
                Field::Attribute(name) => Ok(Field::Computed {
                    func: "ident".into(),
                    args: vec![Field::Attribute(name)],
                    alias,
                }),
                other => Ok(Field::Computed {
                    func: "expr".into(),
                    args: vec![other],
                    alias,
                }),
            }
        } else {
            Ok(field)
        }
    }

    fn parse_graph_traversal(&mut self) -> Result<GraphTraversal, DarshJError> {
        let mut steps = Vec::new();
        loop {
            let direction = match self.peek() {
                Token::Arrow => {
                    self.advance();
                    EdgeDirection::Out
                }
                Token::BackArrow => {
                    self.advance();
                    EdgeDirection::In
                }
                _ => break,
            };
            let edge = self.expect_ident()?;
            steps.push(GraphStep { edge, direction });
        }
        if steps.is_empty() {
            return Err(DarshJError::InvalidQuery(
                "expected graph traversal step".into(),
            ));
        }
        Ok(GraphTraversal { steps })
    }

    fn parse_set_or_content(&mut self) -> Result<SetOrContent, DarshJError> {
        if self.eat(Token::Set) {
            Ok(SetOrContent::Set(self.parse_set_pairs()?))
        } else if self.eat(Token::Content) {
            self.parse_content_json()
        } else {
            // Default to empty SET if no data clause.
            Ok(SetOrContent::Set(Vec::new()))
        }
    }

    fn parse_set_pairs(&mut self) -> Result<Vec<(String, Expr)>, DarshJError> {
        let mut pairs = Vec::new();
        loop {
            let key = self.expect_ident()?;
            self.expect(Token::Eq)?;
            let val = self.parse_expr()?;
            pairs.push((key, val));
            if !self.eat(Token::Comma) {
                break;
            }
        }
        Ok(pairs)
    }

    fn parse_content_json(&mut self) -> Result<SetOrContent, DarshJError> {
        // Expect a JSON object literal: { ... }
        // We'll collect the raw tokens between braces and parse as JSON.
        self.expect(Token::LBrace)?;
        let mut depth = 1;
        let mut json_str = String::from("{");

        while depth > 0 {
            let tok = self.advance();
            match &tok {
                Token::LBrace => {
                    depth += 1;
                    json_str.push('{');
                }
                Token::RBrace => {
                    depth -= 1;
                    if depth > 0 {
                        json_str.push('}');
                    }
                }
                Token::Colon => json_str.push(':'),
                Token::Comma => json_str.push(','),
                Token::StringLit(s) => {
                    json_str.push('"');
                    json_str.push_str(s);
                    json_str.push('"');
                }
                Token::IntLit(n) => json_str.push_str(&n.to_string()),
                Token::NumberLit(n) => json_str.push_str(&n.to_string()),
                Token::True => json_str.push_str("true"),
                Token::False => json_str.push_str("false"),
                Token::Null => json_str.push_str("null"),
                Token::Ident(s) => {
                    json_str.push('"');
                    json_str.push_str(s);
                    json_str.push('"');
                }
                Token::Eof => {
                    return Err(DarshJError::InvalidQuery(
                        "unterminated CONTENT object".into(),
                    ));
                }
                _ => {
                    return Err(DarshJError::InvalidQuery(format!(
                        "unexpected token in CONTENT: {tok:?}"
                    )));
                }
            }
        }
        json_str.push('}');

        let value: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| DarshJError::InvalidQuery(format!("invalid CONTENT JSON: {e}")))?;

        Ok(SetOrContent::Content(value))
    }

    fn parse_ident_list(&mut self) -> Result<Vec<String>, DarshJError> {
        let mut idents = Vec::new();
        loop {
            idents.push(self.expect_ident()?);
            if !self.eat(Token::Comma) {
                break;
            }
        }
        Ok(idents)
    }

    fn parse_order_by_list(&mut self) -> Result<Vec<OrderBy>, DarshJError> {
        let mut orders = Vec::new();
        loop {
            let field = self.expect_ident()?;
            let direction = if self.eat(Token::Desc) {
                SortDir::Desc
            } else {
                self.eat(Token::Asc);
                SortDir::Asc
            };
            orders.push(OrderBy { field, direction });
            if !self.eat(Token::Comma) {
                break;
            }
        }
        Ok(orders)
    }

    fn parse_u64(&mut self) -> Result<u64, DarshJError> {
        match self.advance() {
            Token::IntLit(n) if n >= 0 => Ok(n as u64),
            other => Err(DarshJError::InvalidQuery(format!(
                "expected positive integer, got {other:?}"
            ))),
        }
    }

    // ── Expression parser (precedence climbing) ────────────────────

    fn parse_expr(&mut self) -> Result<Expr, DarshJError> {
        self.parse_or_expr()
    }

    fn parse_or_expr(&mut self) -> Result<Expr, DarshJError> {
        let mut left = self.parse_and_expr()?;
        while self.eat(Token::Or) {
            let right = self.parse_and_expr()?;
            left = Expr::LogicalOp {
                left: Box::new(left),
                op: LogicOp::Or,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self) -> Result<Expr, DarshJError> {
        let mut left = self.parse_comparison()?;
        while self.eat(Token::And) {
            let right = self.parse_comparison()?;
            left = Expr::LogicalOp {
                left: Box::new(left),
                op: LogicOp::And,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, DarshJError> {
        let left = self.parse_primary()?;

        let op = match self.peek() {
            Token::Eq => Some(BinOp::Eq),
            Token::Neq => Some(BinOp::Neq),
            Token::Gt => Some(BinOp::Gt),
            Token::Gte => Some(BinOp::Gte),
            Token::Lt => Some(BinOp::Lt),
            Token::Lte => Some(BinOp::Lte),
            Token::Like => Some(BinOp::Like),
            Token::Contains => Some(BinOp::Contains),
            Token::Is => Some(BinOp::Is),
            _ => None,
        };

        if let Some(op) = op {
            self.advance();

            // Handle IS NOT
            let op = if op == BinOp::Is && self.eat(Token::Not) {
                BinOp::IsNot
            } else {
                op
            };

            let right = self.parse_primary()?;
            Ok(Expr::BinaryOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            })
        } else {
            Ok(left)
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, DarshJError> {
        match self.peek().clone() {
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(Expr::Paren(Box::new(expr)))
            }
            Token::StringLit(s) => {
                self.advance();
                Ok(Expr::Value(serde_json::Value::String(s)))
            }
            Token::IntLit(n) => {
                self.advance();
                Ok(Expr::Value(serde_json::json!(n)))
            }
            Token::NumberLit(n) => {
                self.advance();
                Ok(Expr::Value(serde_json::json!(n)))
            }
            Token::True => {
                self.advance();
                Ok(Expr::Value(serde_json::Value::Bool(true)))
            }
            Token::False => {
                self.advance();
                Ok(Expr::Value(serde_json::Value::Bool(false)))
            }
            Token::Null => {
                self.advance();
                Ok(Expr::Value(serde_json::Value::Null))
            }
            // Type cast: <type>expr
            Token::Lt => {
                let saved = self.pos;
                self.advance();
                if let Token::Ident(type_name) = self.peek().clone() {
                    self.advance();
                    if self.eat(Token::Gt) {
                        if let Some(dt) = DarshType::from_str(&type_name.to_lowercase()) {
                            let inner = self.parse_primary()?;
                            return Ok(Expr::Cast {
                                cast_type: dt,
                                expr: Box::new(inner),
                            });
                        }
                    }
                }
                self.pos = saved;
                Err(DarshJError::InvalidQuery(
                    "unexpected '<' in expression".into(),
                ))
            }
            // Graph traversal in expression context.
            Token::Arrow | Token::BackArrow => {
                let trav = self.parse_graph_traversal()?;
                Ok(Expr::GraphExpr(trav))
            }
            Token::Ident(name) => {
                self.advance();

                // Check for record link: ident:ident
                if self.eat(Token::Colon) {
                    let id = self.parse_record_id_value()?;
                    return Ok(Expr::RecordLink(RecordId { table: name, id }));
                }

                // Check for function call: name(...)
                if matches!(self.peek(), Token::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Token::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if !self.eat(Token::Comma) {
                                break;
                            }
                        }
                    }
                    self.expect(Token::RParen)?;
                    return Ok(Expr::FnCall { name, args });
                }

                Ok(Expr::Ident(name))
            }
            other => Err(DarshJError::InvalidQuery(format!(
                "unexpected token in expression: {other:?}"
            ))),
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_select() {
        let stmts = Parser::parse("SELECT * FROM users WHERE age > 18").unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Select(s) => {
                assert!(matches!(&s.fields[0], Field::All));
                assert!(matches!(&s.from, Target::Table(t) if t == "users"));
                assert!(s.condition.is_some());
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn parse_select_with_order_limit() {
        let stmts =
            Parser::parse("SELECT name, age FROM users ORDER BY age DESC LIMIT 10 START 5")
                .unwrap();
        match &stmts[0] {
            Statement::Select(s) => {
                assert_eq!(s.fields.len(), 2);
                assert_eq!(s.order.len(), 1);
                assert_eq!(s.order[0].direction, SortDir::Desc);
                assert_eq!(s.limit, Some(10));
                assert_eq!(s.start, Some(5));
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn parse_graph_traversal() {
        let stmts =
            Parser::parse("SELECT ->friends->friends FROM user:darsh").unwrap();
        match &stmts[0] {
            Statement::Select(s) => {
                assert_eq!(s.fields.len(), 1);
                match &s.fields[0] {
                    Field::Graph(g) => {
                        assert_eq!(g.steps.len(), 2);
                        assert_eq!(g.steps[0].edge, "friends");
                        assert_eq!(g.steps[0].direction, EdgeDirection::Out);
                    }
                    _ => panic!("expected Graph field"),
                }
                assert!(matches!(&s.from, Target::Record(r) if r.table == "user" && r.id == "darsh"));
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn parse_create_with_record_link() {
        let stmts = Parser::parse(
            r#"CREATE user:darsh SET name = "Darsh", company = company:knowai"#,
        )
        .unwrap();
        match &stmts[0] {
            Statement::Create(c) => {
                assert!(matches!(&c.target, Target::Record(r) if r.id == "darsh"));
                match &c.data {
                    SetOrContent::Set(pairs) => {
                        assert_eq!(pairs.len(), 2);
                        assert!(matches!(&pairs[1].1, Expr::RecordLink(r) if r.table == "company" && r.id == "knowai"));
                    }
                    _ => panic!("expected Set"),
                }
            }
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn parse_relate() {
        let stmts = Parser::parse(
            r#"RELATE user:darsh->works_at->company:knowai SET since = "2024""#,
        )
        .unwrap();
        match &stmts[0] {
            Statement::Relate(r) => {
                assert_eq!(r.from.table, "user");
                assert_eq!(r.from.id, "darsh");
                assert_eq!(r.edge, "works_at");
                assert_eq!(r.to.table, "company");
                assert_eq!(r.to.id, "knowai");
                assert!(r.data.is_some());
            }
            _ => panic!("expected Relate"),
        }
    }

    #[test]
    fn parse_live_select() {
        let stmts = Parser::parse("LIVE SELECT * FROM users").unwrap();
        match &stmts[0] {
            Statement::LiveSelect(ls) => {
                assert!(matches!(&ls.fields[0], Field::All));
                assert!(matches!(&ls.from, Target::Table(t) if t == "users"));
            }
            _ => panic!("expected LiveSelect"),
        }
    }

    #[test]
    fn parse_type_cast() {
        let stmts = Parser::parse("SELECT <int>age, <string>id FROM users").unwrap();
        match &stmts[0] {
            Statement::Select(s) => {
                assert_eq!(s.fields.len(), 2);
                assert!(matches!(&s.fields[0], Field::Cast { cast_type: DarshType::Int, .. }));
                assert!(matches!(&s.fields[1], Field::Cast { cast_type: DarshType::String, .. }));
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn parse_computed_field() {
        let stmts =
            Parser::parse("SELECT *, count(->posts) AS post_count FROM users").unwrap();
        match &stmts[0] {
            Statement::Select(s) => {
                assert_eq!(s.fields.len(), 2);
                assert!(matches!(&s.fields[0], Field::All));
                match &s.fields[1] {
                    Field::Computed { func, alias, .. } => {
                        assert_eq!(func, "count");
                        assert_eq!(alias, "post_count");
                    }
                    _ => panic!("expected Computed"),
                }
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn parse_define_table() {
        let stmts = Parser::parse("DEFINE TABLE users SCHEMAFULL").unwrap();
        match &stmts[0] {
            Statement::DefineTable(dt) => {
                assert_eq!(dt.name, "users");
                assert_eq!(dt.schema_mode, SchemaMode::Schemafull);
            }
            _ => panic!("expected DefineTable"),
        }
    }

    #[test]
    fn parse_define_field() {
        let stmts =
            Parser::parse("DEFINE FIELD email ON TABLE users TYPE string").unwrap();
        match &stmts[0] {
            Statement::DefineField(df) => {
                assert_eq!(df.name, "email");
                assert_eq!(df.table, "users");
                assert_eq!(df.field_type, Some(DarshType::String));
            }
            _ => panic!("expected DefineField"),
        }
    }

    #[test]
    fn parse_info_for_db() {
        let stmts = Parser::parse("INFO FOR DB").unwrap();
        match &stmts[0] {
            Statement::InfoFor(i) => {
                assert!(matches!(&i.target, InfoTarget::Db));
            }
            _ => panic!("expected InfoFor"),
        }
    }

    #[test]
    fn parse_info_for_table() {
        let stmts = Parser::parse("INFO FOR TABLE users").unwrap();
        match &stmts[0] {
            Statement::InfoFor(i) => {
                assert!(matches!(&i.target, InfoTarget::Table(t) if t == "users"));
            }
            _ => panic!("expected InfoFor"),
        }
    }

    #[test]
    fn parse_delete() {
        let stmts = Parser::parse("DELETE user:darsh").unwrap();
        match &stmts[0] {
            Statement::Delete(d) => {
                assert!(matches!(&d.target, Target::Record(r) if r.id == "darsh"));
            }
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn parse_update() {
        let stmts =
            Parser::parse(r#"UPDATE users SET active = true WHERE age >= 18"#).unwrap();
        match &stmts[0] {
            Statement::Update(u) => {
                assert!(matches!(&u.target, Target::Table(t) if t == "users"));
                assert!(u.condition.is_some());
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn parse_insert() {
        let stmts = Parser::parse(
            r#"INSERT INTO users (name, age) VALUES ("Darsh", 25), ("Alice", 30)"#,
        )
        .unwrap();
        match &stmts[0] {
            Statement::Insert(i) => {
                assert_eq!(i.table, "users");
                assert_eq!(i.fields.len(), 2);
                assert_eq!(i.values.len(), 2);
            }
            _ => panic!("expected Insert"),
        }
    }

    #[test]
    fn parse_multiple_statements() {
        let stmts = Parser::parse(
            "SELECT * FROM users; CREATE user:test SET name = \"Test\"",
        )
        .unwrap();
        assert_eq!(stmts.len(), 2);
        assert!(matches!(&stmts[0], Statement::Select(_)));
        assert!(matches!(&stmts[1], Statement::Create(_)));
    }

    #[test]
    fn parse_logical_operators() {
        let stmts = Parser::parse("SELECT * FROM users WHERE age > 18 AND active = true")
            .unwrap();
        match &stmts[0] {
            Statement::Select(s) => {
                assert!(matches!(
                    &s.condition,
                    Some(Expr::LogicalOp { op: LogicOp::And, .. })
                ));
            }
            _ => panic!("expected Select"),
        }
    }
}
