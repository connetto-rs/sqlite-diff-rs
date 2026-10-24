//! Parser for `SQLite` changeset/patchset binary format.
//!
//! Parses `SQLite` session extension changesets and patchsets from binary into
//! [`DiffSetBuilder`] instances.
//!
//! # Binary Format
//!
//! The format consists of one or more table sections:
//!
//! ```text
//! Table Header:
//! ├── Marker: 'T' (0x54) for changeset, 'P' (0x50) for patchset
//! ├── Column count (1 byte)
//! ├── PK flags (1 byte per column: 0 = not part of the key, k = k-th key column)
//! └── Table name (null-terminated UTF-8)
//!
//! Change Records (repeated):
//! ├── Operation code: INSERT=0x12, DELETE=0x09, UPDATE=0x17
//! ├── Indirect flag (1 byte, usually 0)
//! └── Values (encoded per operation type)
//! ```
//!

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::hash::Hash;

use crate::IndexableValues;

/// Type alias for update operation values.
type UpdateValues = Vec<(MaybeValue<String, Vec<u8>>, MaybeValue<String, Vec<u8>>)>;

/// Type alias for parsed values result.
type ParsedValues = (Vec<MaybeValue<String, Vec<u8>>>, usize);
use crate::builders::{
    ChangesetFormat, DiffSet, DiffSetBuilder, Format, Operation, PatchsetFormat,
};
use crate::encoding::varint::decode_varint;
use crate::encoding::{MaybeValue, Value, decode_value, markers, op_codes};
use crate::schema::{DynTable, SchemaWithPK};

/// Errors that can occur during parsing.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// Unexpected end of input.
    #[error("Unexpected end of input at position {0}")]
    UnexpectedEof(usize),

    /// Invalid table marker (expected 'T' or 'P').
    #[error("Invalid table marker 0x{0:02x} at position {1}")]
    InvalidTableMarker(u8, usize),

    /// Invalid operation code.
    #[error("Invalid operation code 0x{0:02x} at position {1}")]
    InvalidOpCode(u8, usize),

    /// Invalid UTF-8 in table name.
    #[error("Invalid UTF-8 in table name at position {0}")]
    InvalidTableName(usize),

    /// Failed to decode a value.
    #[error("Failed to decode value at position {0}")]
    InvalidValue(usize),

    /// Table name not null-terminated.
    #[error("Table name not null-terminated")]
    UnterminatedTableName,

    /// Mixed format markers in the same file.
    #[error("Mixed format markers: expected {expected:?}, found {found:?} at position {position}")]
    MixedFormats {
        /// The expected format marker.
        expected: FormatMarker,
        /// The found format marker.
        found: FormatMarker,
        /// The position where the mismatch occurred.
        position: usize,
    },

    /// Primary-key flags in a table header are not a unique dense 1-based sequence.
    ///
    /// The nonzero bytes must form exactly `{1, 2, ..., n}` where `n` is the count
    /// of nonzero bytes. All-zero flags (no primary key) are valid.
    #[error(
        "Invalid primary-key flags for table {table_name:?} at position {position}: \
         nonzero bytes must be a unique dense 1-based sequence"
    )]
    InvalidPrimaryKeyFlags {
        /// The table whose header contained invalid flags.
        table_name: String,
        /// Byte position of the pk_flags region in the input.
        position: usize,
    },
}

/// The detected format marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatMarker {
    /// Changeset format ('T' marker).
    Changeset,
    /// Patchset format ('P' marker).
    Patchset,
}

/// A table schema parsed from binary changeset/patchset data.
///
/// This type implements [`DynTable`] and [`SchemaWithPK`], allowing it
/// to be used with [`DiffSetBuilder`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableSchema<S> {
    /// The table name.
    name: S,
    /// Number of columns.
    column_count: usize,
    /// Primary key flags - raw bytes from the changeset/patchset.
    ///
    /// Each byte represents the 1-based ordinal position in the composite PK,
    /// or 0 if the column is not part of the primary key.
    /// For example, `[1, 0, 2]` means column 0 is the first PK column,
    /// column 1 is not a PK column, and column 2 is the second PK column.
    pk_flags: Vec<u8>,
}

impl<S> TableSchema<S> {
    /// Create a new parsed table schema.
    ///
    /// # Panics
    ///
    /// Panics if `pk_flags` is not one byte per column, or if its nonzero bytes
    /// are not the dense key ordinals `1..=n`. Byte input reaching the same
    /// invariant through the parser is refused with
    /// [`ParseError::InvalidPrimaryKeyFlags`] instead.
    #[inline]
    #[must_use]
    pub fn new(name: S, column_count: usize, pk_flags: Vec<u8>) -> Self {
        assert_eq!(pk_flags.len(), column_count);
        assert!(
            pk_flags_are_dense_ordinals(&pk_flags),
            "pk_flags must hold the dense key ordinals 1..=n"
        );
        Self {
            name,
            column_count,
            pk_flags,
        }
    }

    /// Returns the name of the table.
    #[inline]
    #[must_use]
    pub fn name(&self) -> &S {
        &self.name
    }

    /// Returns the raw primary-key flags. Each byte at index `i`
    /// represents column `i`: `0` means the column is not part of the
    /// primary key, and a non-zero value `k` means it is the `k`-th
    /// column in the composite primary key.
    #[inline]
    #[must_use]
    pub fn pk_flags(&self) -> &[u8] {
        &self.pk_flags
    }
}

impl<S: AsRef<str> + Clone + Eq + core::fmt::Debug> DynTable for TableSchema<S> {
    #[inline]
    fn name(&self) -> &str {
        self.name.as_ref()
    }

    #[inline]
    fn number_of_columns(&self) -> usize {
        self.column_count
    }

    #[inline]
    fn write_pk_flags(&self, buf: &mut [u8]) {
        assert_eq!(buf.len(), self.column_count);
        buf.copy_from_slice(&self.pk_flags);
    }
}

impl<N: AsRef<str> + Clone + core::hash::Hash + Eq + core::fmt::Debug> SchemaWithPK
    for TableSchema<N>
{
    fn number_of_primary_keys(&self) -> usize {
        self.pk_flags.iter().filter(|&&b| b > 0).count()
    }

    fn primary_key_index(&self, col_idx: usize) -> Option<usize> {
        self.pk_flags.get(col_idx).and_then(|&pk_ordinal| {
            if pk_ordinal > 0 {
                Some(usize::from(pk_ordinal - 1))
            } else {
                None
            }
        })
    }

    fn extract_pk<S, B>(
        &self,
        values: &impl IndexableValues<Text = S, Binary = B>,
    ) -> alloc::vec::Vec<Value<S, B>>
    where
        S: Clone,
        B: Clone,
    {
        self.primary_key_columns()
            .map(|i| {
                values
                    .get(i)
                    .expect("primary key column index out of bounds, values shorter than schema")
            })
            .collect()
    }
}

/// A parsed changeset or patchset whose operations retain their stored order.
#[derive(Debug, Clone, Eq)]
pub enum ParsedDiffSet {
    /// A parsed changeset.
    Changeset(DiffSet<ChangesetFormat, TableSchema<String>, String, Vec<u8>>),
    /// A parsed patchset.
    Patchset(DiffSet<PatchsetFormat, TableSchema<String>, String, Vec<u8>>),
}

impl PartialEq for ParsedDiffSet {
    fn eq(&self, other: &Self) -> bool {
        let self_empty = match self {
            ParsedDiffSet::Changeset(d) => d.is_empty(),
            ParsedDiffSet::Patchset(d) => d.is_empty(),
        };
        let other_empty = match other {
            ParsedDiffSet::Changeset(d) => d.is_empty(),
            ParsedDiffSet::Patchset(d) => d.is_empty(),
        };

        if self_empty && other_empty {
            return true;
        }

        // Otherwise compare by variant and content
        match (self, other) {
            (ParsedDiffSet::Changeset(a), ParsedDiffSet::Changeset(b)) => a == b,
            (ParsedDiffSet::Patchset(a), ParsedDiffSet::Patchset(b)) => a == b,
            _ => false,
        }
    }
}

impl TryFrom<&[u8]> for ParsedDiffSet {
    type Error = ParseError;

    fn try_from(data: &[u8]) -> Result<Self, Self::Error> {
        Self::parse(data)
    }
}

impl From<ParsedDiffSet> for Vec<u8> {
    fn from(diffset: ParsedDiffSet) -> Self {
        match diffset {
            ParsedDiffSet::Changeset(d) => d.into(),
            ParsedDiffSet::Patchset(d) => d.into(),
        }
    }
}

impl ParsedDiffSet {
    /// Parse binary data into a frozen [`DiffSet`].
    ///
    /// The format (changeset vs patchset) is determined by the first table marker.
    ///
    /// # Errors
    ///
    /// Returns a `ParseError` if the data is malformed or contains invalid values.
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        if data.is_empty() {
            // Empty data defaults to changeset
            return Ok(ParsedDiffSet::Changeset(DiffSet::default()));
        }

        // Peek at the first byte to determine format
        match data[0] {
            markers::CHANGESET => {
                let diffset = parse_as_changeset(data)?;
                Ok(ParsedDiffSet::Changeset(diffset))
            }
            markers::PATCHSET => {
                let diffset = parse_as_patchset(data)?;
                Ok(ParsedDiffSet::Patchset(diffset))
            }
            b => Err(ParseError::InvalidTableMarker(b, 0)),
        }
    }

    /// Returns true if this is a changeset.
    #[must_use]
    pub fn is_changeset(&self) -> bool {
        matches!(self, ParsedDiffSet::Changeset(_))
    }

    /// Returns true if this is a patchset.
    #[must_use]
    pub fn is_patchset(&self) -> bool {
        matches!(self, ParsedDiffSet::Patchset(_))
    }

    /// Returns the table schemas for all tables with non-empty operations.
    #[must_use]
    pub fn table_schemas(&self) -> Vec<&TableSchema<String>> {
        match self {
            ParsedDiffSet::Changeset(d) => d
                .tables
                .iter()
                .filter(|(_, ops)| !ops.is_empty())
                .map(|(schema, _)| schema)
                .collect(),
            ParsedDiffSet::Patchset(d) => d
                .tables
                .iter()
                .filter(|(_, ops)| !ops.is_empty())
                .map(|(schema, _)| schema)
                .collect(),
        }
    }

    /// Stably reorder table sections by `key`, preserving each schema and its operations.
    pub fn sort_tables_by_key<K: Ord>(&mut self, mut key: impl FnMut(&TableSchema<String>) -> K) {
        match self {
            ParsedDiffSet::Changeset(d) => d.tables.sort_by_key(|(schema, _)| key(schema)),
            ParsedDiffSet::Patchset(d) => d.tables.sort_by_key(|(schema, _)| key(schema)),
        }
    }

    /// Rename table sections in place.
    ///
    /// The callback receives each section name and returns a new name, or
    /// `None` to leave it unchanged. Returns the number of sections renamed.
    /// Only the name changes. Columns, primary-key flags, and operations are
    /// untouched, and two sections mapped to the same name stay separate.
    pub fn rename_tables<F>(&mut self, mut rename: F) -> usize
    where
        F: FnMut(&str) -> Option<String>,
    {
        fn rename_in<Fmt, F>(tables: &mut [(TableSchema<String>, Fmt)], rename: &mut F) -> usize
        where
            F: FnMut(&str) -> Option<String>,
        {
            let mut renamed = 0;
            for (schema, _) in tables.iter_mut() {
                if let Some(new_name) = rename(schema.name.as_str()) {
                    schema.name = new_name;
                    renamed += 1;
                }
            }
            renamed
        }

        match self {
            ParsedDiffSet::Changeset(d) => rename_in(&mut d.tables, &mut rename),
            ParsedDiffSet::Patchset(d) => rename_in(&mut d.tables, &mut rename),
        }
    }
}

/// Parse binary data as a changeset.
///
/// # Errors
///
/// Returns a `ParseError` if the data is malformed or not a valid changeset.
fn parse_as_changeset(
    data: &[u8],
) -> Result<DiffSet<ChangesetFormat, TableSchema<String>, String, Vec<u8>>, ParseError> {
    parse_as(data, FormatMarker::Changeset, parse_changeset_operation)
}

/// Parse every table of `data`, which must all be in the `expected` format, adding
/// each operation with `parse_operation`.
///
/// # Errors
///
/// Returns a `ParseError` if the data is malformed or mixes formats.
fn parse_as<F: Format<String, Vec<u8>>>(
    data: &[u8],
    expected: FormatMarker,
    parse_operation: ParseOperation<F>,
) -> Result<ParsedSet<F>, ParseError>
where
    ParsedSet<F>: From<ParsedBuilder<F>>,
{
    let mut builder = DiffSetBuilder::new();
    let mut pos = 0;

    while pos < data.len() {
        let (schema, format, header_len) = parse_table_header(&data[pos..], pos)?;
        if format != expected {
            return Err(ParseError::MixedFormats {
                expected,
                found: format,
                position: pos,
            });
        }
        pos += header_len;

        while pos < data.len() {
            let byte = data[pos];
            if byte == markers::CHANGESET || byte == markers::PATCHSET {
                break;
            }
            let op_len = parse_operation(&data[pos..], pos, &schema, &mut builder)?;
            pos += op_len;
        }
    }

    Ok(builder.into())
}

/// Parse binary data as a patchset.
///
/// # Errors
///
/// Returns a `ParseError` if the data is malformed or not a valid patchset.
fn parse_as_patchset(
    data: &[u8],
) -> Result<DiffSet<PatchsetFormat, TableSchema<String>, String, Vec<u8>>, ParseError> {
    parse_as(data, FormatMarker::Patchset, parse_patchset_operation)
}

/// Parse a table header and return the schema.
fn parse_table_header(
    data: &[u8],
    base_pos: usize,
) -> Result<(TableSchema<String>, FormatMarker, usize), ParseError> {
    let mut pos = 0;

    if data.is_empty() {
        return Err(ParseError::UnexpectedEof(base_pos));
    }
    let format = match data[pos] {
        markers::CHANGESET => FormatMarker::Changeset,
        markers::PATCHSET => FormatMarker::Patchset,
        b => return Err(ParseError::InvalidTableMarker(b, base_pos + pos)),
    };
    pos += 1;

    let (column_count, varint_len) = decode_varint(&data[pos..])
        .ok_or(ParseError::UnexpectedEof(base_pos + pos))
        .and_then(|(count, len)| {
            usize::try_from(count)
                .map(|count| (count, len))
                .map_err(|_| ParseError::UnexpectedEof(base_pos + pos))
        })?;
    pos += varint_len;

    let pk_flags_end = pos
        .checked_add(column_count)
        .filter(|&end| end <= data.len())
        .ok_or(ParseError::UnexpectedEof(base_pos + pos))?;
    let pk_flags_pos = base_pos + pos;
    let pk_flags: Vec<u8> = data[pos..pk_flags_end].to_vec();
    pos = pk_flags_end;

    let name_start = pos;
    while pos < data.len() && data[pos] != 0 {
        pos += 1;
    }
    if pos >= data.len() {
        return Err(ParseError::UnterminatedTableName);
    }
    let name = String::from_utf8(data[name_start..pos].to_vec())
        .map_err(|_| ParseError::InvalidTableName(base_pos + name_start))?;
    pos += 1;

    if !pk_flags_are_dense_ordinals(&pk_flags) {
        return Err(ParseError::InvalidPrimaryKeyFlags {
            table_name: name,
            position: pk_flags_pos,
        });
    }

    Ok((TableSchema::new(name, column_count, pk_flags), format, pos))
}

/// Whether the nonzero flag bytes are the dense key ordinals `1..=n`.
///
/// All-zero flags describe a table with no primary key and are accepted.
fn pk_flags_are_dense_ordinals(flags: &[u8]) -> bool {
    let key_count = flags.iter().filter(|&&flag| flag != 0).count();
    if key_count > usize::from(u8::MAX) {
        return false;
    }
    let mut seen: [u64; 4] = [0; 4];
    for &flag in flags {
        if flag == 0 {
            continue;
        }
        let ordinal = usize::from(flag);
        if ordinal > key_count {
            return false;
        }
        let mask = 1u64 << (ordinal % 64);
        if seen[ordinal / 64] & mask != 0 {
            return false;
        }
        seen[ordinal / 64] |= mask;
    }
    true
}

/// Parse operation header (`op_code` + indirect flag).
///
/// Returns `(op_code, indirect, bytes_consumed)`. Any non-zero indirect byte
/// parses as `true` to match SQLite's permissive treatment of the flag.
fn parse_operation_header(data: &[u8], base_pos: usize) -> Result<(u8, bool, usize), ParseError> {
    if data.len() < 2 {
        return Err(ParseError::UnexpectedEof(base_pos));
    }
    Ok((data[0], data[1] != 0, 2))
}

/// A builder of parsed tables in format `F`.
type ParsedBuilder<F> = DiffSetBuilder<F, TableSchema<String>, String, Vec<u8>>;

/// A diff set of parsed tables in format `F`.
type ParsedSet<F> = DiffSet<F, TableSchema<String>, String, Vec<u8>>;

/// Parses one operation at a byte offset into the builder, returning its length.
type ParseOperation<F> =
    fn(&[u8], usize, &TableSchema<String>, &mut ParsedBuilder<F>) -> Result<usize, ParseError>;

/// A full row image and the number of bytes it took.
type ParsedRow = (Vec<Value<String, Vec<u8>>>, usize);

/// Parse `count` values with undefined ones read as NULL, a full row image.
fn parse_row(data: &[u8], base_pos: usize, count: usize) -> Result<ParsedRow, ParseError> {
    let (values, len) = parse_values(data, base_pos, count)?;
    Ok((
        values
            .into_iter()
            .map(|v| v.unwrap_or(Value::Null))
            .collect(),
        len,
    ))
}

/// Parse an INSERT body, which both formats store as a full row, into `builder`.
fn parse_insert<F: Format<String, Vec<u8>>>(
    data: &[u8],
    base_pos: usize,
    schema: &TableSchema<String>,
    builder: &mut ParsedBuilder<F>,
    indirect: bool,
) -> Result<usize, ParseError>
where
    Operation<F, String, Vec<u8>>: core::ops::Add<Output = Option<Operation<F, String, Vec<u8>>>>,
{
    let (values, len) = parse_row(data, base_pos, schema.column_count)?;
    let pk = schema.extract_pk(&values);
    builder.add_operation(schema, pk, Operation::Insert { values, indirect });
    Ok(len)
}

/// Parse a changeset operation.
fn parse_changeset_operation(
    data: &[u8],
    base_pos: usize,
    schema: &TableSchema<String>,
    builder: &mut DiffSetBuilder<ChangesetFormat, TableSchema<String>, String, Vec<u8>>,
) -> Result<usize, ParseError> {
    let (op_code, indirect, mut pos) = parse_operation_header(data, base_pos)?;

    match op_code {
        op_codes::INSERT => {
            pos += parse_insert(&data[pos..], base_pos + pos, schema, builder, indirect)?;
        }
        op_codes::DELETE => {
            let (values, len) = parse_row(&data[pos..], base_pos + pos, schema.column_count)?;
            pos += len;
            let pk = schema.extract_pk(&values);
            builder.add_operation(
                schema,
                pk,
                Operation::Delete {
                    data: values,
                    indirect,
                },
            );
        }
        op_codes::UPDATE => {
            let (old_values, old_len) =
                parse_values(&data[pos..], base_pos + pos, schema.column_count)?;
            pos += old_len;
            let (new_values, new_len) =
                parse_values(&data[pos..], base_pos + pos, schema.column_count)?;
            pos += new_len;
            // Extract PK using old values (convert None to Null)
            let pk_values: Vec<Value<String, Vec<u8>>> = old_values
                .iter()
                .map(|v| v.clone().unwrap_or(Value::Null))
                .collect();
            let pk = schema.extract_pk(&pk_values);
            let values: UpdateValues = old_values.into_iter().zip(new_values).collect();
            builder.add_operation(schema, pk, Operation::Update { values, indirect });
        }
        _ => return Err(ParseError::InvalidOpCode(op_code, base_pos)),
    }

    Ok(pos)
}

/// Parse a patchset operation.
fn parse_patchset_operation(
    data: &[u8],
    base_pos: usize,
    schema: &TableSchema<String>,
    builder: &mut DiffSetBuilder<PatchsetFormat, TableSchema<String>, String, Vec<u8>>,
) -> Result<usize, ParseError> {
    let (op_code, indirect, mut pos) = parse_operation_header(data, base_pos)?;

    match op_code {
        op_codes::INSERT => {
            pos += parse_insert(&data[pos..], base_pos + pos, schema, builder, indirect)?;
        }
        op_codes::DELETE => {
            // Patchset DELETE: only PK values in column order
            let pk_count = schema.pk_flags.iter().filter(|&&b| b > 0).count();
            let (pk_values, len) = parse_values(&data[pos..], base_pos + pos, pk_count)?;
            pos += len;
            // Expand PK values to full row, then extract_pk to get ordinal-sorted PK.
            // This is needed because the binary format stores PKs in column order,
            // but the builder stores them sorted by pk_ordinal (matching the serializer).
            let full_values = expand_pk_values(&schema.pk_flags, pk_values, schema.column_count);
            // Convert MaybeValue to Value for extract_pk (PK values should always be defined)
            let full_values_concrete: Vec<Value<String, Vec<u8>>> = full_values
                .into_iter()
                .map(|v| v.unwrap_or(Value::Null))
                .collect();
            let pk = schema.extract_pk(&full_values_concrete);
            builder.add_operation(schema, pk, Operation::Delete { data: (), indirect });
        }
        op_codes::UPDATE => {
            // Patchset UPDATE wire layout, matching SQLite's session extension: one record
            // of exactly `column_count` entries in column order. A primary key column
            // carries its value, any other column carries its new value or `0x00` when it
            // did not change. It is NOT a primary key block followed by a non-primary key
            // block: those only look alike when the primary key is the first column, which
            // is what hid this for so long.
            //
            // The full-width `Vec<((), MaybeValue)>` keeps downstream code (`extract_pk`,
            // `sql_output`, consolidation, reversal) uniform: primary key slots hold
            // `Some(value)`, other slots hold `Some(new_value)` or `None` for undefined.
            let (record, record_len) =
                parse_values(&data[pos..], base_pos + pos, schema.column_count)?;
            pos += record_len;

            let mut values: Vec<((), MaybeValue<String, Vec<u8>>)> =
                alloc::vec![((), None); schema.column_count];
            for (col_idx, (&pk_flag, entry)) in schema.pk_flags.iter().zip(record).enumerate() {
                if pk_flag > 0 {
                    // A primary key column always carries a defined value. A stray
                    // undefined marker is normalised to Null to stay lenient for
                    // fuzz-generated input, matching `expand_pk_values` in the DELETE path.
                    values[col_idx] = ((), Some(entry.unwrap_or(Value::Null)));
                } else {
                    values[col_idx] = ((), entry);
                }
            }

            let pk = schema.extract_pk(&values);
            builder.add_operation(schema, pk, Operation::Update { values, indirect });
        }
        _ => return Err(ParseError::InvalidOpCode(op_code, base_pos)),
    }

    Ok(pos)
}

/// Expand PK-only values to full row with None (undefined) for non-PK columns.
///
/// The `pk_flags` are raw bytes where non-zero means the column is part of the PK.
/// PK values are expected in the order they appear in `pk_flags` (not sorted by ordinal).
fn expand_pk_values(
    pk_flags: &[u8],
    pk_values: Vec<MaybeValue<String, Vec<u8>>>,
    column_count: usize,
) -> Vec<MaybeValue<String, Vec<u8>>> {
    let mut full: Vec<MaybeValue<String, Vec<u8>>> = vec![None; column_count];
    let mut pk_iter = pk_values.into_iter();
    for (i, &pk_ordinal) in pk_flags.iter().enumerate() {
        if pk_ordinal > 0
            && let Some(v) = pk_iter.next()
        {
            full[i] = v;
        }
    }
    full
}

/// Parse a sequence of values.
fn parse_values(data: &[u8], base_pos: usize, count: usize) -> Result<ParsedValues, ParseError> {
    let mut values = Vec::with_capacity(count);
    let mut pos = 0;

    for _ in 0..count {
        let (value, value_len) =
            decode_value(&data[pos..]).ok_or(ParseError::InvalidValue(base_pos + pos))?;
        values.push(value);
        pos += value_len;
    }

    Ok((values, pos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SimpleTable;
    use alloc::vec;

    #[test]
    fn test_parse_empty() {
        let result = ParsedDiffSet::parse(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_changeset());
    }

    #[test]
    fn test_parse_table_header() {
        // 'T', 2 columns, pk_flags [1, 0], table name "t\0"
        let data = [b'T', 2, 1, 0, b't', 0];
        let (schema, format, len) = parse_table_header(&data, 0).unwrap();

        assert_eq!(format, FormatMarker::Changeset);
        assert_eq!(schema.column_count, 2);
        assert_eq!(schema.pk_flags, vec![1, 0]); // Raw bytes: 1 = first PK column, 0 = not PK
        assert_eq!(schema.name, "t");
        assert_eq!(len, 6);
    }

    #[test]
    fn test_parse_insert_changeset() {
        // Table header + INSERT with integer 1 and text "a"
        let mut data = vec![b'T', 2, 1, 0, b't', 0];
        // INSERT opcode, indirect=0
        data.push(op_codes::INSERT);
        data.push(0);
        // Integer 1 (type 1, 8 bytes)
        data.push(0x01);
        data.extend(&1i64.to_be_bytes());
        // Text "a" (type 3, length 1, "a")
        data.push(0x03);
        data.push(1);
        data.push(b'a');

        let parsed = ParsedDiffSet::parse(&data).unwrap();
        assert!(parsed.is_changeset());
    }

    #[test]
    fn test_parse_delete_changeset() {
        let mut data = vec![b'T', 2, 1, 0, b't', 0];
        data.push(op_codes::DELETE);
        data.push(0);
        // Integer 1
        data.push(0x01);
        data.extend(&1i64.to_be_bytes());
        // Text "a"
        data.push(0x03);
        data.push(1);
        data.push(b'a');

        let parsed = ParsedDiffSet::parse(&data).unwrap();
        assert!(parsed.is_changeset());
    }

    #[test]
    fn test_parse_delete_patchset() {
        // Patchset DELETE only has PK values
        let mut data = vec![b'P', 2, 1, 0, b't', 0];
        data.push(op_codes::DELETE);
        data.push(0);
        // Only PK value (integer 1)
        data.push(0x01);
        data.extend(&1i64.to_be_bytes());

        let parsed = ParsedDiffSet::parse(&data).unwrap();
        assert!(parsed.is_patchset());
    }

    #[test]
    fn test_parse_update_changeset() {
        let mut data = vec![b'T', 2, 1, 0, b't', 0];
        data.push(op_codes::UPDATE);
        data.push(0);
        // Old values: integer 1, text "a"
        data.push(0x01);
        data.extend(&1i64.to_be_bytes());
        data.push(0x03);
        data.push(1);
        data.push(b'a');
        // New values: integer 1, text "b"
        data.push(0x01);
        data.extend(&1i64.to_be_bytes());
        data.push(0x03);
        data.push(1);
        data.push(b'b');

        let parsed = ParsedDiffSet::parse(&data).unwrap();
        assert!(parsed.is_changeset());
    }

    #[test]
    fn test_is_changeset() {
        let data = vec![b'T', 1, 1, b't', 0];
        let parsed = ParsedDiffSet::parse(&data).unwrap();
        assert!(parsed.is_changeset());
        assert!(!parsed.is_patchset());
    }

    #[test]
    fn test_is_patchset() {
        let data = vec![b'P', 1, 1, b't', 0];
        let parsed = ParsedDiffSet::parse(&data).unwrap();
        assert!(parsed.is_patchset());
        assert!(!parsed.is_changeset());
    }

    #[test]
    fn test_parsed_table_schema_dyn_table() {
        let schema: TableSchema<String> = TableSchema::new("users".into(), 3, vec![1, 0, 0]);
        assert_eq!(schema.name(), "users");
        assert_eq!(schema.number_of_columns(), 3);

        let mut buf = [0u8; 3];
        schema.write_pk_flags(&mut buf);
        assert_eq!(buf, [1, 0, 0]);
    }

    #[test]
    fn test_parsed_table_schema_extract_pk() {
        let schema: TableSchema<String> = TableSchema::new("users".into(), 3, vec![1, 0, 2]);
        let values: Vec<Value<String, Vec<u8>>> = vec![
            Value::Integer(1),
            Value::Text("alice".into()),
            Value::Integer(100),
        ];
        let pk = schema.extract_pk(&values);
        let expected: Vec<Value<String, Vec<u8>>> = vec![Value::Integer(1), Value::Integer(100)];
        assert_eq!(pk, expected);
    }

    // ---- Error path tests ----

    #[test]
    fn test_parse_invalid_table_marker() {
        let data = [0xFFu8, 1, 1, b't', 0];
        let err = ParsedDiffSet::parse(&data).unwrap_err();
        assert!(
            matches!(err, ParseError::InvalidTableMarker(0xFF, 0)),
            "got {err:?}"
        );
    }

    #[test]
    fn test_parse_unexpected_eof_in_table_header() {
        // 'T' marker but no column count
        let data = *b"T";
        let err = ParsedDiffSet::parse(&data).unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof(_)), "got {err:?}");
    }

    #[test]
    fn test_parse_unexpected_eof_in_pk_flags() {
        // 'T', column count 3, but only 1 PK flag byte
        let data = [b'T', 3, 1];
        let err = ParsedDiffSet::parse(&data).unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof(_)), "got {err:?}");
    }

    #[test]
    fn test_parse_column_count_past_usize_max() {
        // 'T', then a ten-byte varint column count that decodes to 2^64 - 4
        let data = [
            b'T', 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7c, 0x01, 0x29,
        ];
        let err = ParsedDiffSet::parse(&data).unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof(_)), "got {err:?}");
    }

    #[test]
    fn test_parse_unterminated_table_name() {
        // 'T', 1 column, pk_flags [1], then "abc" with no null terminator
        let data = [b'T', 1, 1, b'a', b'b', b'c'];
        let err = ParsedDiffSet::parse(&data).unwrap_err();
        assert!(
            matches!(err, ParseError::UnterminatedTableName),
            "got {err:?}"
        );
    }

    #[test]
    fn test_parse_invalid_utf8_in_table_name() {
        // 'T', 1 column, pk_flags [1], then 0xFF (invalid UTF-8), then null
        let data = [b'T', 1, 1, 0xFF, 0];
        let err = ParsedDiffSet::parse(&data).unwrap_err();
        assert!(
            matches!(err, ParseError::InvalidTableName(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn test_parse_mixed_formats_changeset_then_patchset() {
        // First table 'T' (changeset), then second table 'P' (patchset)
        let mut data = vec![b'T', 1, 1, b'a', 0];
        // Now a 'P' table header without preceding operations
        data.extend_from_slice(&[b'P', 1, 1, b'b', 0]);
        let err = ParsedDiffSet::parse(&data).unwrap_err();
        assert!(
            matches!(
                err,
                ParseError::MixedFormats {
                    expected: FormatMarker::Changeset,
                    found: FormatMarker::Patchset,
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn test_parse_mixed_formats_patchset_then_changeset() {
        let mut data = vec![b'P', 1, 1, b'a', 0];
        data.extend_from_slice(&[b'T', 1, 1, b'b', 0]);
        let err = ParsedDiffSet::parse(&data).unwrap_err();
        assert!(
            matches!(
                err,
                ParseError::MixedFormats {
                    expected: FormatMarker::Patchset,
                    found: FormatMarker::Changeset,
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    /// Build the operation header bytes followed by a single integer payload.
    fn make_insert_with_indirect(indirect_byte: u8) -> Vec<u8> {
        let mut data = vec![b'T', 1, 1, b't', 0];
        data.push(op_codes::INSERT);
        data.push(indirect_byte);
        // Integer 1
        data.push(0x01);
        data.extend(&1i64.to_be_bytes());
        data
    }

    fn first_op_indirect_changeset(data: &[u8]) -> bool {
        let parsed = ParsedDiffSet::parse(data).unwrap();
        let ParsedDiffSet::Changeset(set) = parsed else {
            panic!("expected Changeset");
        };
        set.tables
            .iter()
            .find_map(|(_schema, rows)| rows.first().map(|(_, op)| op.indirect()))
            .expect("expected at least one op")
    }

    #[test]
    fn test_parse_changeset_indirect_flag_set() {
        let data = make_insert_with_indirect(1);
        assert!(first_op_indirect_changeset(&data));
    }

    #[test]
    fn test_parse_changeset_indirect_flag_clear() {
        let data = make_insert_with_indirect(0);
        assert!(!first_op_indirect_changeset(&data));
    }

    #[test]
    fn test_parse_indirect_nonzero_treated_as_true() {
        // Any non-zero byte must parse as indirect = true.
        let data = make_insert_with_indirect(0x42);
        assert!(first_op_indirect_changeset(&data));
    }

    #[test]
    fn test_parsed_diffset_variant_mismatch_partial_eq() {
        let changeset = ParsedDiffSet::parse(&[b'T', 1, 1, b't', 0]).unwrap();
        let patchset = ParsedDiffSet::parse(&[b'P', 1, 1, b't', 0]).unwrap();
        // Both are empty so PartialEq short-circuits to true. Add a real op
        // to each so the variant-mismatch arm in the `match` is reached.
        let mut full_changeset = vec![b'T', 1, 1, b't', 0];
        full_changeset.push(op_codes::INSERT);
        full_changeset.push(0);
        full_changeset.push(0x01);
        full_changeset.extend(&1i64.to_be_bytes());
        let cs = ParsedDiffSet::parse(&full_changeset).unwrap();

        let mut full_patchset = vec![b'P', 1, 1, b't', 0];
        full_patchset.push(op_codes::INSERT);
        full_patchset.push(0);
        full_patchset.push(0x01);
        full_patchset.extend(&1i64.to_be_bytes());
        let ps = ParsedDiffSet::parse(&full_patchset).unwrap();

        assert_ne!(cs, ps);
        // Empty/empty still equal regardless of variant.
        assert_eq!(changeset, patchset);
    }

    #[test]
    fn test_parse_unexpected_eof_in_operation_header() {
        // Valid changeset header followed by a single byte (op_code only,
        // no indirect byte) — parse_operation_header must return UnexpectedEof.
        let data = [b'T', 1, 1, b't', 0, op_codes::INSERT];
        let err = ParsedDiffSet::parse(&data).unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof(_)), "got {err:?}");
    }

    #[test]
    fn test_parse_patchset_indirect_flag_set() {
        // Patchset INSERT carries full row values, same header layout.
        let mut data = vec![b'P', 1, 1, b't', 0];
        data.push(op_codes::INSERT);
        data.push(1);
        data.push(0x01);
        data.extend(&1i64.to_be_bytes());

        let parsed = ParsedDiffSet::parse(&data).unwrap();
        let ParsedDiffSet::Patchset(set) = parsed else {
            panic!("expected Patchset");
        };
        let indirect = set
            .tables
            .iter()
            .find_map(|(_schema, rows)| rows.first().map(|(_, op)| op.indirect()))
            .expect("expected at least one op");
        assert!(indirect);
    }

    /// Assert a real SQLite patchset UPDATE byte string parses to a single
    /// UPDATE operation, run caller-supplied checks against the destructured
    /// state, and confirm the parsed value re-serializes byte-identically.
    ///
    /// Centralizing the destructures here keeps the individual scenario tests
    /// focused on their assertions and folds the defensive `panic!` arms into
    /// one place. Each test below feeds bytes captured from a real
    /// `Session::patchset_strm` call, so the checker sees the exact wire
    /// layout the parser now targets.
    fn assert_patchset_update_roundtrip(
        data: &[u8],
        check: impl FnOnce(
            &TableSchema<String>,
            &[Value<String, Vec<u8>>],
            &[((), MaybeValue<String, Vec<u8>>)],
            bool,
        ),
    ) {
        let parsed = ParsedDiffSet::parse(data).expect("SQLite patchset UPDATE must parse");
        let ParsedDiffSet::Patchset(set) = parsed else {
            panic!("expected Patchset, got {parsed:?}");
        };
        let (schema, rows) = set.tables.first().expect("expected one table");
        assert_eq!(rows.len(), 1, "expected exactly one row");
        let (pk, op) = rows.first().expect("row map non-empty");
        let Operation::Update { values, indirect } = op else {
            panic!("expected Update, got {op:?}");
        };
        check(schema, pk.as_slice(), values.as_slice(), *indirect);
        let serialized: Vec<u8> = set.into();
        assert_eq!(serialized, data, "roundtrip must match SQLite output");
    }

    /// Real SQLite session output for a standalone patchset UPDATE against a
    /// pre-existing row on a single-column PK table:
    ///
    /// ```text
    /// CREATE TABLE orders (id INTEGER PRIMARY KEY, amount INTEGER, status TEXT);
    /// INSERT INTO orders VALUES (5, 100, 'pending'); -- before session.attach()
    /// UPDATE orders SET status = 'shipped' WHERE id = 5; -- after attach, tracked
    /// ```
    ///
    /// Wire layout:
    /// - 12 bytes header ('P', 3, [1,0,0], "orders\0")
    /// - 2 bytes op header (UPDATE, indirect=0)
    /// - 9 bytes old side: INTEGER 5 (only the PK column)
    /// - 10 bytes new side: undefined (amount unchanged) + TEXT 'shipped'
    ///
    /// Total 33 bytes. Historically the parser expected `column_count` values on
    /// each side (padded with undefined) and returned `InvalidValue` mid-buffer.
    #[test]
    fn test_parse_patchset_update_sqlite_wire_layout_single_pk() {
        let data: [u8; 33] = [
            0x50, 0x03, 0x01, 0x00, 0x00, b'o', b'r', b'd', b'e', b'r', b's', 0x00, 0x17, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x03, 0x07, b's', b'h',
            b'i', b'p', b'p', b'e', b'd',
        ];
        assert_patchset_update_roundtrip(&data, |schema, pk, values, indirect| {
            assert_eq!(schema.name, "orders");
            assert_eq!(schema.column_count, 3);
            assert_eq!(schema.pk_flags, vec![1, 0, 0]);
            assert_eq!(pk, &[Value::Integer(5)]);
            assert!(!indirect);
            assert_eq!(values.len(), 3);
            assert_eq!(values[0].1, Some(Value::Integer(5))); // PK preserved
            assert_eq!(values[1].1, None); // amount unchanged
            assert_eq!(values[2].1, Some(Value::Text("shipped".into())));
        });
    }

    /// Real SQLite output for a composite PK, `PRIMARY KEY(a, b)`:
    ///
    /// ```text
    /// CREATE TABLE items (a INTEGER NOT NULL, b INTEGER NOT NULL, val TEXT, PRIMARY KEY(a, b));
    /// INSERT INTO items VALUES (1, 2, 'v1'); -- before attach
    /// UPDATE items SET val = 'v2' WHERE a = 1 AND b = 2;
    /// ```
    ///
    /// Wire layout: two PK values on the old side (INTEGER 1, INTEGER 2), one
    /// non-PK value on the new side (TEXT 'v2'). 35 bytes total.
    #[test]
    fn test_parse_patchset_update_sqlite_wire_layout_composite_pk() {
        let data: [u8; 35] = [
            0x50, 0x03, 0x01, 0x02, 0x00, b'i', b't', b'e', b'm', b's', 0x00, 0x17, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x02, 0x03, 0x02, b'v', b'2',
        ];
        assert_patchset_update_roundtrip(&data, |schema, pk, values, _indirect| {
            assert_eq!(schema.name, "items");
            assert_eq!(schema.pk_flags, vec![1, 2, 0]);
            // `extract_pk` returns values sorted by PK ordinal. With PK(a, b) and
            // pk_flags [1, 2, 0], ordinal 1 is column `a`, ordinal 2 is column `b`.
            assert_eq!(pk, &[Value::Integer(1), Value::Integer(2)]);
            assert_eq!(values.len(), 3);
            assert_eq!(values[0].1, Some(Value::Integer(1))); // a (PK)
            assert_eq!(values[1].1, Some(Value::Integer(2))); // b (PK)
            assert_eq!(values[2].1, Some(Value::Text("v2".into())));
        });
    }

    /// Every non-PK column is present on the new side, in column order, either
    /// as its new value or as the undefined marker `0x00` when unchanged.
    ///
    /// ```text
    /// UPDATE orders SET amount = 200, status = 'shipped' WHERE id = 5;
    /// ```
    ///
    /// Two non-PK columns changed, so both are defined values (no undefined
    /// markers). 41 bytes total.
    #[test]
    fn test_parse_patchset_update_all_non_pk_changed() {
        let data: [u8; 41] = [
            0x50, 0x03, 0x01, 0x00, 0x00, b'o', b'r', b'd', b'e', b'r', b's', 0x00, 0x17, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x01, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0xc8, 0x03, 0x07, b's', b'h', b'i', b'p', b'p', b'e', b'd',
        ];
        assert_patchset_update_roundtrip(&data, |_schema, _pk, values, _indirect| {
            assert_eq!(values[0].1, Some(Value::Integer(5)));
            assert_eq!(values[1].1, Some(Value::Integer(200)));
            assert_eq!(values[2].1, Some(Value::Text("shipped".into())));
        });
    }

    /// Assert a parsed [`TableSchema`] and a [`SimpleTable`] of the same shape
    /// agree on every [`SchemaWithPK`] accessor, so the read and build sides
    /// are symmetric.
    fn assert_schema_pk_parity(
        parsed: &TableSchema<String>,
        simple: &SimpleTable,
        row: &[Value<String, Vec<u8>>],
    ) {
        assert_eq!(
            parsed.number_of_primary_keys(),
            simple.number_of_primary_keys(),
            "number_of_primary_keys",
        );
        for col in 0..simple.number_of_columns() {
            assert_eq!(
                parsed.primary_key_index(col),
                simple.primary_key_index(col),
                "primary_key_index at col {col}",
            );
        }
        assert_eq!(
            parsed.primary_key_columns().collect::<Vec<usize>>(),
            simple.primary_key_columns().collect::<Vec<usize>>(),
            "primary_key_columns",
        );
        assert_eq!(
            parsed.extract_pk(&row),
            simple.extract_pk(&row),
            "extract_pk"
        );
    }

    #[test]
    fn test_parsed_schema_pk_parity_single_key() {
        // Changeset over `kv(id, val)` with single-column key `id` (flags [1, 0]).
        let mut data = vec![b'T', 2, 1, 0, b'k', b'v', 0];
        data.push(op_codes::INSERT);
        data.push(0);
        data.push(0x01);
        data.extend(&1i64.to_be_bytes());
        data.push(0x03);
        data.push(1);
        data.push(b'x');

        let ParsedDiffSet::Changeset(set) = ParsedDiffSet::parse(&data).unwrap() else {
            panic!("expected changeset");
        };
        let (parsed, _rows) = set.tables.first().expect("one table");
        assert_eq!(parsed.pk_flags(), &[1, 0]);

        let simple = SimpleTable::with_rowid_alias("kv", &[("id", "INTEGER"), ("val", "TEXT")], 0);
        let row: Vec<Value<String, Vec<u8>>> = vec![Value::Integer(1), Value::Text("x".into())];
        assert_schema_pk_parity(parsed, &simple, &row);
        assert_eq!(parsed.primary_key_columns().collect::<Vec<usize>>(), [0]);
    }

    #[test]
    fn test_parsed_schema_pk_parity_composite_reordered_key() {
        // Changeset over `abc(a, b, c)` whose key is `(b, a)`, so the key column
        // order differs from table order. Flags are [2, 1, 0].
        let mut data = vec![b'T', 3, 2, 1, 0, b'a', b'b', b'c', 0];
        data.push(op_codes::INSERT);
        data.push(0);
        data.push(0x01);
        data.extend(&10i64.to_be_bytes());
        data.push(0x01);
        data.extend(&20i64.to_be_bytes());
        data.push(0x03);
        data.push(1);
        data.push(b'z');

        let ParsedDiffSet::Changeset(set) = ParsedDiffSet::parse(&data).unwrap() else {
            panic!("expected changeset");
        };
        let (parsed, _rows) = set.tables.first().expect("one table");
        assert_eq!(parsed.pk_flags(), &[2, 1, 0]);

        let simple = SimpleTable::new(
            "abc",
            &[("a", "INTEGER"), ("b", "INTEGER"), ("c", "TEXT")],
            &[1, 0],
        );
        let row: Vec<Value<String, Vec<u8>>> = vec![
            Value::Integer(10),
            Value::Integer(20),
            Value::Text("z".into()),
        ];
        assert_schema_pk_parity(parsed, &simple, &row);
        // Key order is (b, a): column 1 first, column 0 second.
        assert_eq!(parsed.primary_key_columns().collect::<Vec<usize>>(), [1, 0]);
        // `extract_pk` follows key order: b's value, then a's value.
        assert_eq!(
            parsed.extract_pk(&row),
            vec![Value::Integer(20), Value::Integer(10)]
        );
    }

    #[test]
    fn dense_ordinals_are_accepted_and_every_other_shape_refused() {
        for flags in [
            [0, 0, 0].as_slice(),
            [1, 0].as_slice(),
            [1, 2, 0].as_slice(),
            [2, 1, 0].as_slice(),
            [3, 2, 0, 1].as_slice(),
        ] {
            assert!(pk_flags_are_dense_ordinals(flags), "{flags:?} must parse");
        }
        for flags in [
            [2, 0].as_slice(),
            [3, 0].as_slice(),
            [1, 1, 0].as_slice(),
            [255, 64, 0].as_slice(),
            [15, 0, 63, 215, 61, 58, 56, 56, 50].as_slice(),
        ] {
            assert!(!pk_flags_are_dense_ordinals(flags), "{flags:?} must refuse");
        }
    }

    #[test]
    fn header_with_dense_flags_parses() {
        assert!(ParsedDiffSet::parse(&[b'T', 3, 2, 1, 0, b't', 0]).is_ok());
    }

    #[test]
    fn pk_flags_error_carries_table_name_and_position() {
        let data = [b'T', 2, 2, 0, b'm', b'y', b't', b'b', b'l', 0];
        let err = ParsedDiffSet::parse(&data).unwrap_err();
        let ParseError::InvalidPrimaryKeyFlags {
            table_name,
            position,
        } = err
        else {
            panic!("expected InvalidPrimaryKeyFlags, got {err:?}");
        };
        assert_eq!(table_name, "mytbl");
        assert_eq!(position, 2);
    }

    #[test]
    #[should_panic(expected = "pk_flags must hold the dense key ordinals")]
    fn table_schema_new_refuses_flags_the_parser_would_refuse() {
        let _ = TableSchema::new("t", 3, vec![255, 64, 0]);
    }
}
