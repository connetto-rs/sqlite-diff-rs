//! SQL parser for changeset/patchset operations.

use core::hash::Hash;

use crate::{
    DiffSetBuilder, PatchsetFormat, SchemaWithPK, Value, builders::operation::Operation,
    schema::NamedColumns,
};
use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::lexer::{Lexer, LexerError, Token, TokenKind};

/// SQL parser errors.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParseError<'a> {
    /// Lexer error.
    #[error("Lexer error: {0}")]
    Lexer(#[from] LexerError),
    /// Unexpected token.
    #[error("Unexpected token {found:?} at position {pos}, expected {expected}")]
    UnexpectedToken {
        /// What was expected.
        expected: &'static str,
        /// What was found.
        found: TokenKind<'a>,
        /// Position in input.
        pos: usize,
    },
    /// Unexpected end of input.
    #[error("Unexpected end of input, expected {expected}")]
    UnexpectedEof {
        /// What was expected.
        expected: &'static str,
    },
    /// Empty column list.
    #[error("Empty column list in CREATE TABLE")]
    EmptyColumnList,
    /// Duplicate column name.
    #[error("Duplicate column name: {0}")]
    DuplicateColumn(String),
    /// Unknown column in PRIMARY KEY constraint.
    #[error("Unknown column '{column}' in PRIMARY KEY constraint")]
    UnknownPKColumn {
        /// The unknown column name.
        column: String,
    },
    /// Unknown table name in statement.
    #[error("Unknown table name: {0}")]
    UnknownTable(&'a str),
    /// Unknown column name in statement.
    #[error("Unknown column name: {0}")]
    UnknownColumn(&'a str),
    /// Missing WHERE clause in UPDATE or DELETE.
    #[error("Missing WHERE clause in {statement}")]
    MissingWhere {
        /// The statement type.
        statement: &'static str,
    },
    /// Where constraint on non-PK column.
    #[error("WHERE clause on non-primary key column '{column}'")]
    WhereNonPKColumn {
        /// The column name.
        column: &'a str,
    },
    /// Explicit column list has more entries than the VALUES list.
    #[error("column list names {expected} columns but VALUES has {found}")]
    ValuesLengthMismatch {
        /// Number of columns in the explicit column list.
        expected: usize,
        /// Number of values provided in the VALUES clause.
        found: usize,
    },
    /// OR is not supported in a WHERE clause.
    #[error("OR is not supported in WHERE. Use separate statements for multiple rows")]
    OrInWhere,
    /// WHERE clause does not pin every primary key column.
    #[error("WHERE must name all {expected} primary key columns, but only {found} were provided")]
    IncompleteWhereKey {
        /// Total number of primary key columns.
        expected: usize,
        /// How many were actually provided.
        found: usize,
    },
    /// `UPDATE ... SET` gives a primary key column a value other than the one its `WHERE` names.
    #[error(
        "UPDATE changes primary key column '{column}', which SQLite records as a DELETE and an INSERT of the whole row"
    )]
    PrimaryKeyUpdate {
        /// The primary key column.
        column: &'a str,
    },
    /// `INSERT` gives the rowid alias column a value SQLite cannot store as an integer.
    #[error(
        "datatype mismatch: column {column} of table '{table}' aliases the rowid and holds only integers"
    )]
    DatatypeMismatch {
        /// The table name.
        table: &'a str,
        /// Index of the rowid alias column.
        column: usize,
    },
    /// `INSERT` leaves the rowid alias column `NULL` or out, so SQLite assigns
    /// a rowid the statement does not state.
    #[error(
        "column {column} of table '{table}' aliases the rowid and needs a value, because SQLite would assign one"
    )]
    MissingRowid {
        /// The table name.
        table: &'a str,
        /// Index of the rowid alias column.
        column: usize,
    },
}

/// SQL parser. Collects operations into a pending list without applying
/// them to the builder, enabling all-or-nothing application via `into_pending`.
pub(crate) struct Parser<'input, 'builder, T: SchemaWithPK, S> {
    lexer: Lexer<'input>,
    /// Read-only access for schema lookups.
    builder: &'builder DiffSetBuilder<PatchsetFormat, T, S, Vec<u8>>,
    /// Parsed operations waiting to be applied.
    pending: Vec<PendingOp<T, S>>,
}

/// A parsed operation waiting to be applied: the table, its primary key values, and the
/// operation itself. Nothing is applied to the builder until every statement has parsed.
type PendingOp<T, S> = (T, KeyValues<S>, Operation<PatchsetFormat, S, Vec<u8>>);

/// Primary key values in primary key order.
type KeyValues<S> = Vec<Value<S, Vec<u8>>>;

impl<'input, 'builder, T: NamedColumns, S: Clone + Hash + Eq + AsRef<str> + for<'a> From<&'a str>>
    Parser<'input, 'builder, T, S>
{
    /// Create a new parser for the given input.
    #[must_use]
    pub(crate) fn new(
        input: &'input str,
        builder: &'builder DiffSetBuilder<PatchsetFormat, T, S, Vec<u8>>,
    ) -> Self {
        Self {
            lexer: Lexer::new(input),
            builder,
            pending: Vec::new(),
        }
    }

    /// Consume the parser and return all collected operations.
    pub(crate) fn into_pending(self) -> Vec<PendingOp<T, S>> {
        self.pending
    }

    /// Parse all statements from the input.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing fails.
    pub(crate) fn digest_all(&mut self) -> Result<(), ParseError<'input>> {
        loop {
            // Skip any semicolons (leading, trailing, between statements)
            while self.lexer.peek()?.kind == TokenKind::Semicolon {
                self.lexer.next()?;
            }

            if self.lexer.peek()?.kind == TokenKind::Eof {
                break;
            }

            self.digest_statement()?;

            let next = self.lexer.peek()?;
            if !matches!(next.kind, TokenKind::Semicolon | TokenKind::Eof) {
                return Err(ParseError::UnexpectedToken {
                    expected: "; or end of input",
                    found: next.kind.clone(),
                    pos: next.pos,
                });
            }
        }

        Ok(())
    }

    /// Parse a single statement.
    fn digest_statement(&mut self) -> Result<(), ParseError<'input>> {
        let token = self.lexer.peek()?;
        match &token.kind {
            TokenKind::Insert => self.digest_insert(),
            TokenKind::Update => self.digest_update(),
            TokenKind::Delete => self.digest_delete(),
            other => Err(ParseError::UnexpectedToken {
                expected: "INSERT, UPDATE, or DELETE",
                found: other.clone(),
                pos: token.pos,
            }),
        }
    }

    /// Parse an INSERT statement.
    fn digest_insert(&mut self) -> Result<(), ParseError<'input>> {
        self.expect(&TokenKind::Insert)?;
        self.expect(&TokenKind::Into)?;

        let (table, table_name) = self.expect_table()?;

        // Column identifiers for the explicit list, empty means positional.
        // u16 is enough: SQLite's default column limit is 2000, and even with
        // SQLITE_MAX_COLUMN it cannot exceed i16::MAX.
        let mut column_identifiers: Vec<u16> = Vec::new();
        if self.lexer.peek()?.kind == TokenKind::LParen {
            self.lexer.next()?;

            loop {
                column_identifiers.push(self.expect_column(&table)?.0);
                if self.lexer.peek()?.kind != TokenKind::Comma {
                    break;
                }
                self.lexer.next()?;
            }

            self.expect(&TokenKind::RParen)?;
        }

        self.expect(&TokenKind::Values)?;
        self.expect(&TokenKind::LParen)?;

        let mut values = vec![Value::Null; table.number_of_columns()];
        let mut pks = vec![Value::Null; table.number_of_primary_keys()];

        if column_identifiers.is_empty() {
            // Positional form: values map to all columns in order.
            for (col_idx, value_ref) in values.iter_mut().enumerate() {
                if col_idx > 0 {
                    self.expect(&TokenKind::Comma)?;
                }
                *value_ref = Self::with_affinity(&table, col_idx, self.parse_value()?);
                if let Some(pk_idx) = table.primary_key_index(col_idx) {
                    pks[pk_idx] = (*value_ref).clone();
                }
            }
        } else {
            // Explicit column list: exactly one value per listed column.
            let expected = column_identifiers.len();
            let mut parsed = 0usize;
            for &column_index in &column_identifiers {
                let column_index = usize::from(column_index);
                values[column_index] =
                    Self::with_affinity(&table, column_index, self.parse_value()?);
                if let Some(primary_key_index) = table.primary_key_index(column_index) {
                    pks[primary_key_index] = values[column_index].clone();
                }
                parsed += 1;
                if self.lexer.peek()?.kind != TokenKind::Comma {
                    break;
                }
                self.lexer.next()?;
            }
            if parsed != expected {
                return Err(ParseError::ValuesLengthMismatch {
                    expected,
                    found: parsed,
                });
            }
        }

        self.expect(&TokenKind::RParen)?;

        if let Some(alias) = table.rowid_alias() {
            match values[alias] {
                Value::Integer(_) => {}
                Value::Null => {
                    return Err(ParseError::MissingRowid {
                        table: table_name,
                        column: alias,
                    });
                }
                Value::Text(_) | Value::Real(_) | Value::Blob(_) => {
                    return Err(ParseError::DatatypeMismatch {
                        table: table_name,
                        column: alias,
                    });
                }
            }
        }

        self.pending.push((
            table,
            pks,
            Operation::Insert {
                values,
                indirect: false,
            },
        ));

        Ok(())
    }

    /// Parse an UPDATE statement.
    fn digest_update(&mut self) -> Result<(), ParseError<'input>> {
        self.expect(&TokenKind::Update)?;

        let (table, _) = self.expect_table()?;
        self.expect(&TokenKind::Set)?;

        let mut new_values = vec![((), None); table.number_of_columns()];
        let mut key_sets: Vec<(usize, usize, &'input str)> = Vec::new();

        loop {
            let (col_idx, col_name) = self.expect_column(&table)?;
            let col_idx = usize::from(col_idx);
            self.expect(&TokenKind::Equals)?;
            let val = Self::with_affinity(&table, col_idx, self.parse_value()?);
            if let Some(primary_key_index) = table.primary_key_index(col_idx) {
                key_sets.push((col_idx, primary_key_index, col_name));
            }
            new_values[col_idx] = ((), Some(val));

            if self.lexer.peek()?.kind != TokenKind::Comma {
                break;
            }
            self.lexer.next()?;
        }

        let Some(pk) = self.digest_key_where(&table, "UPDATE")? else {
            return Ok(());
        };

        // A session records a key change as a DELETE plus a full-row INSERT, which SQL text cannot supply.
        if let Some(&(_, _, column)) = key_sets.iter().find(|&&(col_idx, primary_key_index, _)| {
            new_values[col_idx].1.as_ref() != Some(&pk[primary_key_index])
        }) {
            return Err(ParseError::PrimaryKeyUpdate { column });
        }

        self.pending.push((
            table,
            pk,
            Operation::Update {
                values: new_values,
                indirect: false,
            },
        ));

        Ok(())
    }

    /// Parse a DELETE statement.
    fn digest_delete(&mut self) -> Result<(), ParseError<'input>> {
        self.expect(&TokenKind::Delete)?;
        self.expect(&TokenKind::From)?;

        let (table, _) = self.expect_table()?;

        let Some(pks) = self.digest_key_where(&table, "DELETE")? else {
            return Ok(());
        };

        self.pending.push((
            table,
            pks,
            Operation::Delete {
                data: (),
                indirect: false,
            },
        ));

        Ok(())
    }

    /// Parse the WHERE clause of `statement`, which must bind every primary key
    /// column and nothing else, into the key values in primary key order.
    ///
    /// Returns `None` when no row can satisfy the clause.
    fn digest_key_where(
        &mut self,
        table: &T,
        statement: &'static str,
    ) -> Result<Option<KeyValues<S>>, ParseError<'input>> {
        if self.lexer.peek()?.kind != TokenKind::Where {
            return Err(ParseError::MissingWhere { statement });
        }

        let n_pk = table.number_of_primary_keys();
        let mut pk = vec![Value::Null; n_pk];
        let mut pk_seen = vec![false; n_pk];

        let can_match = self.digest_where(table, |col_idx, col_name, val| {
            if let Some(primary_key_index) = table.primary_key_index(usize::from(col_idx)) {
                pk[primary_key_index] = val;
                pk_seen[primary_key_index] = true;
                Ok(())
            } else {
                Err(ParseError::WhereNonPKColumn { column: col_name })
            }
        })?;

        let found = pk_seen.iter().filter(|&&s| s).count();
        if found != n_pk {
            return Err(ParseError::IncompleteWhereKey {
                expected: n_pk,
                found,
            });
        }
        Ok(can_match.then_some(pk))
    }

    /// Parse a WHERE clause, calling `digestor` for each `col = val` predicate
    /// with the literal converted by the column's affinity, as SQLite compares it.
    /// Fails with `OrInWhere` if `OR` appears after a predicate.
    ///
    /// Returns whether a row can satisfy every predicate. A predicate cannot
    /// hold when its value is `NULL`, or is not an integer on a rowid alias.
    fn digest_where<D>(&mut self, table: &T, mut digestor: D) -> Result<bool, ParseError<'input>>
    where
        D: FnMut(u16, &'input str, Value<S, Vec<u8>>) -> Result<(), ParseError<'input>>,
    {
        self.expect(&TokenKind::Where)?;

        let mut can_match = true;
        loop {
            let (col_idx, col_name) = self.expect_column(table)?;
            self.expect(&TokenKind::Equals)?;
            let val = Self::with_affinity(table, usize::from(col_idx), self.parse_value()?);
            can_match &= match val {
                Value::Null => false,
                Value::Integer(_) => true,
                Value::Real(_) | Value::Text(_) | Value::Blob(_) => {
                    table.rowid_alias() != Some(usize::from(col_idx))
                }
            };
            digestor(col_idx, col_name, val)?;

            if self.lexer.peek()?.kind != TokenKind::And {
                if self.lexer.peek()?.kind == TokenKind::Or {
                    return Err(ParseError::OrInWhere);
                }
                break;
            }
            self.lexer.next()?;
        }

        Ok(can_match)
    }

    /// `value` as SQLite stores or compares it in column `col_idx` of `table`.
    fn with_affinity(table: &T, col_idx: usize, value: Value<S, Vec<u8>>) -> Value<S, Vec<u8>> {
        match table.column_affinity(col_idx) {
            Some(affinity) => affinity.apply(value),
            None => value,
        }
    }

    /// Parse a value literal.
    fn parse_value(&mut self) -> Result<Value<S, Vec<u8>>, ParseError<'input>> {
        let token = self.lexer.next()?;
        match token.kind {
            TokenKind::Null => Ok(Value::Null),
            // The lexer caps integers at 2^63, the one integer literal past i64::MAX.
            TokenKind::IntegerLiteral(v) => {
                Ok(i64::try_from(v).map_or(Value::Real(TWO_POW_63), Value::Integer))
            }
            TokenKind::RealLiteral(v) => Ok(Value::Real(v)),
            TokenKind::StringLiteral(s) => {
                let text: S = match s {
                    Cow::Borrowed(b) => S::from(b),
                    Cow::Owned(o) => S::from(o.as_str()),
                };
                Ok(Value::Text(text))
            }
            TokenKind::BlobLiteral(b) => Ok(Value::Blob(b)),
            TokenKind::Minus => {
                let next = self.lexer.next()?;
                match next.kind {
                    TokenKind::IntegerLiteral(v) => {
                        debug_assert!(v <= 1 << 63, "the lexer caps integers at 2^63");
                        Ok(Value::Integer(
                            0_i64.checked_sub_unsigned(v).unwrap_or(i64::MIN),
                        ))
                    }
                    TokenKind::RealLiteral(v) => Ok(Value::Real(-v)),
                    other => Err(ParseError::UnexpectedToken {
                        expected: "number after minus",
                        found: other,
                        pos: next.pos,
                    }),
                }
            }
            other => Err(ParseError::UnexpectedToken {
                expected: "value (NULL, number, string, or blob)",
                found: other,
                pos: token.pos,
            }),
        }
    }

    /// Expect a specific token kind.
    fn expect(
        &mut self,
        expected: &TokenKind<'input>,
    ) -> Result<Token<'input>, ParseError<'input>> {
        let token = self.lexer.next()?;
        if core::mem::discriminant(&token.kind) == core::mem::discriminant(expected) {
            Ok(token)
        } else {
            Err(ParseError::UnexpectedToken {
                expected: expected.static_name(),
                found: token.kind,
                pos: token.pos,
            })
        }
    }

    /// Expects a column identifier and returns the corresponding column index in the table schema.
    fn expect_column(&mut self, table: &T) -> Result<(u16, &'input str), ParseError<'input>> {
        let column_name = self.expect_identifier()?;
        #[allow(clippy::cast_possible_truncation)]
        table
            .column_index(column_name)
            .map(|idx| (idx as u16, column_name))
            .ok_or(ParseError::UnknownColumn(column_name))
    }

    /// Expects a table existing in the builder's schema and returns a clone and its name.
    fn expect_table(&mut self) -> Result<(T, &'input str), ParseError<'input>> {
        let table_name = self.expect_identifier()?;
        self.builder
            .table(table_name)
            .cloned()
            .map(|table| (table, table_name))
            .ok_or(ParseError::UnknownTable(table_name))
    }

    /// Expect an identifier and return its name.
    fn expect_identifier(&mut self) -> Result<&'input str, ParseError<'input>> {
        let token = self.lexer.next()?;
        match token.kind {
            TokenKind::Identifier(name) => Ok(name),
            // Also accept keywords as identifiers (common in SQL)
            TokenKind::Insert => Ok("INSERT"),
            TokenKind::Into => Ok("INTO"),
            TokenKind::Values => Ok("VALUES"),
            TokenKind::Update => Ok("UPDATE"),
            TokenKind::Set => Ok("SET"),
            TokenKind::Delete => Ok("DELETE"),
            TokenKind::From => Ok("FROM"),
            TokenKind::Where => Ok("WHERE"),
            TokenKind::And => Ok("AND"),
            TokenKind::Or => Ok("OR"),
            TokenKind::Primary => Ok("PRIMARY"),
            TokenKind::Key => Ok("KEY"),
            TokenKind::Null => Ok("NULL"),
            TokenKind::Integer => Ok("INTEGER"),
            TokenKind::Int => Ok("INT"),
            TokenKind::Real => Ok("REAL"),
            TokenKind::Text => Ok("TEXT"),
            TokenKind::Blob => Ok("BLOB"),
            TokenKind::Not => Ok("NOT"),
            other => Err(ParseError::UnexpectedToken {
                expected: "identifier",
                found: other,
                pos: token.pos,
            }),
        }
    }
}

/// `9223372036854775808`, which SQLite reads as a real unless negated.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec::Vec;

    use crate::schema::SimpleTable;
    use crate::{DiffSetBuilder, PatchsetFormat};

    fn make_builder(
        tables: &[SimpleTable],
    ) -> DiffSetBuilder<PatchsetFormat, SimpleTable, String, Vec<u8>> {
        let mut builder = DiffSetBuilder::default();
        for t in tables {
            builder.add_table(t);
        }
        builder
    }

    #[test]
    fn test_digest_insert() {
        let users =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let mut builder = make_builder(&[users]);
        builder
            .digest_sql("INSERT INTO users (id, name) VALUES (1, 'Alice')")
            .unwrap();
        assert_eq!(builder.len(), 1);
        assert_ne!(builder.build(), [] as [u8; 0]);
    }

    #[test]
    fn test_digest_insert_positional() {
        let users =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let mut builder = make_builder(&[users]);
        builder
            .digest_sql("INSERT INTO users VALUES (1, 'Alice')")
            .unwrap();
        assert_eq!(builder.len(), 1);
    }

    #[test]
    fn test_digest_update() {
        let users =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let mut builder = make_builder(&[users]);
        builder
            .digest_sql("UPDATE users SET name = 'Bob' WHERE id = 1")
            .unwrap();
        assert_eq!(builder.len(), 1);
        assert_ne!(builder.build(), [] as [u8; 0]);
    }

    #[test]
    fn test_digest_delete() {
        let users =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let mut builder = make_builder(&[users]);
        builder
            .digest_sql("DELETE FROM users WHERE id = 1")
            .unwrap();
        assert_eq!(builder.len(), 1);
        assert_ne!(builder.build(), [] as [u8; 0]);
    }

    #[test]
    fn test_digest_delete_rejects_non_pk_in_where() {
        let users = SimpleTable::with_rowid_alias(
            "users",
            &[("id", "INTEGER"), ("name", "TEXT"), ("status", "TEXT")],
            0,
        );
        let mut builder = make_builder(&[users]);
        let result = builder.digest_sql("DELETE FROM users WHERE id = 1 AND status = 'active'");
        assert!(result.is_err());
    }

    #[test]
    fn test_digest_multiple_dml() {
        let users =
            SimpleTable::with_rowid_alias("users", &[("id", "INTEGER"), ("name", "TEXT")], 0);
        let mut builder = make_builder(&[users]);
        builder
            .digest_sql(
                "INSERT INTO users (id, name) VALUES (1, 'Alice');\
                 INSERT INTO users (id, name) VALUES (2, 'Bob');\
                 DELETE FROM users WHERE id = 1;",
            )
            .unwrap();
        // INSERT(1) + INSERT(2) + DELETE(1) leaves only INSERT(2)
        assert_eq!(builder.len(), 1);
        assert_ne!(builder.build(), [] as [u8; 0]);
    }

    #[test]
    fn test_digest_create_table_rejected() {
        let mut builder: DiffSetBuilder<PatchsetFormat, SimpleTable, String, Vec<u8>> =
            DiffSetBuilder::default();
        let result = builder.digest_sql("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)");
        assert!(result.is_err());
    }

    #[test]
    fn test_digest_blob_value() {
        let t = SimpleTable::new("t", &[("data", "BLOB")], &[0]);
        let mut builder = make_builder(&[t]);
        builder
            .digest_sql("INSERT INTO t (data) VALUES (X'DEADBEEF')")
            .unwrap();
        assert_eq!(builder.len(), 1);
    }

    #[test]
    fn test_digest_null_value() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "INTEGER")], 0);
        let mut builder = make_builder(&[t]);
        builder
            .digest_sql("INSERT INTO t (id, v) VALUES (1, NULL)")
            .unwrap();
        assert_eq!(builder.len(), 1);
    }

    #[test]
    fn test_digest_negative_numbers() {
        let t = SimpleTable::new("t", &[("a", "INTEGER"), ("b", "REAL")], &[0]);
        let mut builder = make_builder(&[t]);
        builder
            .digest_sql("INSERT INTO t (a, b) VALUES (-42, -3.14)")
            .unwrap();
        assert_eq!(builder.len(), 1);
    }

    #[test]
    fn test_digest_keyword_column_names() {
        // Each reserved keyword is a column name. This forces expect_identifier
        // to take every keyword arm. The names registered on the schema match
        // the uppercase constants the parser returns for those arms.
        let cols: [(&str, &str); 18] = [
            ("INSERT", "INTEGER"),
            ("INTO", "INTEGER"),
            ("VALUES", "INTEGER"),
            ("UPDATE", "INTEGER"),
            ("SET", "INTEGER"),
            ("DELETE", "INTEGER"),
            ("FROM", "INTEGER"),
            ("WHERE", "INTEGER"),
            ("AND", "INTEGER"),
            ("PRIMARY", "INTEGER"),
            ("KEY", "INTEGER"),
            ("NULL", "INTEGER"),
            ("INTEGER", "INTEGER"),
            ("INT", "INTEGER"),
            ("REAL", "INTEGER"),
            ("TEXT", "INTEGER"),
            ("BLOB", "INTEGER"),
            ("NOT", "INTEGER"),
        ];
        let t = SimpleTable::new("kwords", &cols, &[0]);
        let mut builder = make_builder(&[t]);
        builder
            .digest_sql(
                "INSERT INTO kwords (insert, into, values, update, set, delete, from, where, and, primary, key, null, integer, int, real, text, blob, not) \
                 VALUES (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18)",
            )
            .unwrap();
        assert_eq!(builder.len(), 1);
    }

    // ---- ParseError variant tests ----

    use crate::builders::sql::ParseError;

    #[test]
    fn test_digest_insert_missing_into() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER")], 0);
        let mut builder = make_builder(&[t]);
        let err = builder.digest_sql("INSERT FROM t").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { .. }));
    }

    #[test]
    fn test_digest_insert_unknown_table() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER")], 0);
        let mut builder = make_builder(&[t]);
        let err = builder
            .digest_sql("INSERT INTO unknown_table VALUES (1)")
            .unwrap_err();
        assert!(matches!(err, ParseError::UnknownTable("unknown_table")));
    }

    #[test]
    fn test_digest_update_missing_where() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "INTEGER")], 0);
        let mut builder = make_builder(&[t]);
        let err = builder.digest_sql("UPDATE t SET v = 1").unwrap_err();
        assert!(matches!(
            err,
            ParseError::MissingWhere {
                statement: "UPDATE"
            }
        ));
    }

    #[test]
    fn test_digest_delete_missing_where() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER")], 0);
        let mut builder = make_builder(&[t]);
        let err = builder.digest_sql("DELETE FROM t").unwrap_err();
        assert!(matches!(
            err,
            ParseError::MissingWhere {
                statement: "DELETE"
            }
        ));
    }

    #[test]
    fn test_digest_update_where_non_pk_column() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "INTEGER")], 0);
        let mut builder = make_builder(&[t]);
        let err = builder
            .digest_sql("UPDATE t SET v = 2 WHERE v = 1")
            .unwrap_err();
        assert!(matches!(err, ParseError::WhereNonPKColumn { column: "v" }));
    }

    #[test]
    fn test_digest_unexpected_top_level_token() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER")], 0);
        let mut builder = make_builder(&[t]);
        let err = builder.digest_sql("SELECT 1").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { .. }));
    }

    #[test]
    fn test_digest_expect_identifier_rejects_value_token() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER")], 0);
        let mut builder = make_builder(&[t]);
        // Integer literal where a table name is expected.
        let err = builder.digest_sql("INSERT INTO 42 VALUES (1)").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { .. }));
    }

    #[test]
    fn test_digest_update_changing_key_refused() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "INTEGER")], 0);
        let mut builder = make_builder(&[t]);
        builder.digest_sql("INSERT INTO t VALUES (5, 1)").unwrap();
        let before = builder.build();
        let err = builder
            .digest_sql("UPDATE t SET id = 7 WHERE id = 5")
            .unwrap_err();
        assert_eq!(err, ParseError::PrimaryKeyUpdate { column: "id" });
        assert_eq!(builder.build(), before);
    }

    #[test]
    fn test_digest_update_changing_one_composite_key_column_refused() {
        let t = SimpleTable::new(
            "t",
            &[("a", "INTEGER"), ("b", "INTEGER"), ("v", "INTEGER")],
            &[0, 1],
        );
        let mut builder = make_builder(&[t]);
        let err = builder
            .digest_sql("UPDATE t SET v = 0, b = 9 WHERE a = 1 AND b = 2")
            .unwrap_err();
        assert_eq!(err, ParseError::PrimaryKeyUpdate { column: "b" });
        assert!(builder.is_empty());
    }

    #[test]
    fn test_digest_unterminated_comment_swallows_closing_paren() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "")], 0);
        let mut builder = make_builder(&[t]);
        let err = builder
            .digest_sql("INSERT INTO t VALUES (1, 2 /* never closed)")
            .unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { .. }), "{err:?}");
        assert!(builder.is_empty());
    }
    #[test]
    fn test_digest_update_and_delete_reject_bare_trailing_block_comment() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "INTEGER")], 0);
        for sql in [
            "UPDATE t SET v = 3 WHERE id = 1 /*",
            "DELETE FROM t WHERE id = 1 /*",
        ] {
            let mut builder = make_builder(core::slice::from_ref(&t));
            builder.digest_sql("INSERT INTO t VALUES (5, 6)").unwrap();
            let before = builder.build();
            let err = builder.digest_sql(sql).unwrap_err();
            assert!(
                matches!(
                    err,
                    ParseError::Lexer(super::super::lexer::LexerError::UnexpectedChar {
                        char: '/',
                        pos
                    }) if pos == sql.len() - 2
                ),
                "{sql}: {err:?}"
            );
            assert_eq!(builder.build(), before, "{sql}");
        }
    }

    #[test]
    fn test_digest_bare_trailing_block_comment_rolls_back_batch() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "INTEGER")], 0);
        let mut builder = make_builder(&[t]);
        builder.digest_sql("INSERT INTO t VALUES (5, 6)").unwrap();
        let before = builder.build();
        let sql = "INSERT INTO t VALUES (1, 2); INSERT INTO t VALUES (3, 4)/*";
        assert!(matches!(
            builder.digest_sql(sql),
            Err(ParseError::Lexer(
                super::super::lexer::LexerError::UnexpectedChar { char: '/', .. }
            ))
        ));
        assert_eq!(builder.build(), before);
    }

    #[test]
    fn test_digest_statements_need_a_separator() {
        let t = SimpleTable::with_rowid_alias("t", &[("id", "INTEGER"), ("v", "")], 0);
        let mut builder = make_builder(&[t]);
        let err = builder
            .digest_sql("INSERT INTO t VALUES (1, 2) INSERT INTO t VALUES (3, 4)")
            .unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedToken { .. }), "{err:?}");
        assert!(builder.is_empty());
        builder
            .digest_sql("INSERT INTO t VALUES (1, 2);INSERT INTO t VALUES (3, 4);")
            .unwrap();
        assert_eq!(builder.len(), 2);
    }

    #[test]
    fn test_digest_rowid_alias_refuses_non_integer_key() {
        let k = SimpleTable::with_rowid_alias("k", &[("v", ""), ("id", "INTEGER")], 1);
        let mut builder = make_builder(&[k]);
        builder.digest_sql("INSERT INTO k VALUES (1, 1)").unwrap();
        let before = builder.build();
        for sql in [
            "INSERT INTO k VALUES (1, 'abc')",
            "INSERT INTO k (id, v) VALUES (7.5, 1)",
            "INSERT INTO k VALUES (1, X'07')",
            "INSERT INTO k VALUES (2, 2); INSERT INTO k VALUES (1, '7a')",
        ] {
            let err = builder.digest_sql(sql).unwrap_err();
            assert_eq!(
                err,
                ParseError::DatatypeMismatch {
                    table: "k",
                    column: 1
                },
                "{sql}"
            );
            assert_eq!(builder.build(), before, "{sql}");
        }
    }

    #[test]
    fn test_digest_rowid_alias_refuses_null_or_omitted_key() {
        let k = SimpleTable::with_rowid_alias("k", &[("v", ""), ("id", "INTEGER")], 1);
        let mut builder = make_builder(&[k]);
        builder.digest_sql("INSERT INTO k VALUES (1, 1)").unwrap();
        let before = builder.build();
        for sql in [
            "INSERT INTO k VALUES (1, NULL)",
            "INSERT INTO k (v) VALUES (1)",
        ] {
            let err = builder.digest_sql(sql).unwrap_err();
            assert_eq!(
                err,
                ParseError::MissingRowid {
                    table: "k",
                    column: 1
                },
                "{sql}"
            );
            assert_eq!(builder.build(), before, "{sql}");
        }
    }
}
