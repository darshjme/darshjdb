# Formula Engine

DarshJDB's formula engine provides spreadsheet-style computed fields. Formulas are parsed into an AST, evaluated against a record's field values, and recalculated automatically when dependencies change.

## Formula Syntax

Formulas use a syntax familiar to spreadsheet users:

```
IF(AND({Status} = "Done", {Priority} > 3), "High", "Low")
```

Key syntax elements:

- **Field references**: `{Field Name}` -- curly braces around the field name.
- **String literals**: `"hello world"` -- double-quoted, with `\"`, `\\`, `\n`, `\t` escape sequences.
- **Number literals**: `42`, `3.14`, `1.5e10` -- integers, decimals, and scientific notation.
- **Boolean literals**: `TRUE`, `FALSE` -- case-insensitive keywords.
- **Function calls**: `FUNCTION_NAME(arg1, arg2, ...)` -- uppercase by convention, parentheses required.
- **Operators**: Standard arithmetic, comparison, logical, and concatenation operators.
- **Parenthesized expressions**: `(expr)` for explicit grouping.

## Operators

### Precedence Table (Lowest to Highest)

| Precedence | Operators | Associativity | Description |
|---|---|---|---|
| 1 | `OR` | Left | Logical OR |
| 2 | `AND` | Left | Logical AND |
| 3 | `=`, `!=`, `<>` | Left | Equality / inequality |
| 4 | `<`, `<=`, `>`, `>=` | Left | Comparison |
| 5 | `+`, `-`, `&` | Left | Addition, subtraction, string concatenation |
| 6 | `*`, `/`, `%` | Left | Multiplication, division, modulo |
| 7 | `NOT`, `-` (unary) | Right | Logical NOT, numeric negation |

The `&` operator performs string concatenation: `{First} & " " & {Last}` produces `"John Doe"`.

The `<>` operator is an alias for `!=`.

### Type Coercion in Operators

- **Arithmetic** (`+`, `-`, `*`, `/`, `%`): Both operands are coerced to numbers. Strings that look like numbers are auto-promoted. Non-numeric operands produce `#VALUE`.
- **Division/modulo by zero**: Produces `#DIV/0`.
- **Concatenation** (`&`): Both operands are coerced to strings. Numbers render without trailing zeros for integers.
- **Comparison** (`<`, `<=`, `>`, `>=`): Tries numeric comparison first, falls back to string comparison.
- **Equality** (`=`, `!=`): Exact JSON value comparison.
- **Logical** (`AND`, `OR`): Both sides are coerced to booleans (see truthiness rules below).

### Truthiness Rules

| Value | Boolean |
|---|---|
| `true` | true |
| `false` | false |
| `null` | false |
| `0`, `0.0` | false |
| any other number | true |
| `""` (empty string) | false |
| any non-empty string | true |
| arrays, objects | true |

## Error Values

When an operation fails, the formula returns an error string instead of throwing:

| Error | Meaning |
|---|---|
| `#ERROR` | General computation error |
| `#REF` | Invalid field reference |
| `#VALUE` | Type mismatch (e.g. arithmetic on non-numeric value) |
| `#DIV/0` | Division or modulo by zero |

Error values propagate through binary operations: if either operand is an error, the result is that error.

## Built-in Functions

### Logic Functions

| Function | Args | Description |
|---|---|---|
| `AND(a, b, ...)` | 1+ | Returns TRUE if all arguments are truthy |
| `OR(a, b, ...)` | 1+ | Returns TRUE if any argument is truthy |
| `NOT(value)` | 1 | Logical negation |
| `IF(cond, then, else)` | 2-3 | Conditional. If 2 args, else defaults to null |
| `SWITCH(expr, p1, r1, p2, r2, ..., default)` | 3+ | Pattern matching. Compares expr against each pattern, returns matching result. Last odd argument is the default. |

### Text Functions

| Function | Args | Description |
|---|---|---|
| `CONCAT(a, b, ...)` | 1+ | Concatenate all arguments as strings |
| `LEN(text)` | 1 | Character length of the string |
| `LOWER(text)` | 1 | Convert to lowercase |
| `UPPER(text)` | 1 | Convert to uppercase |
| `TRIM(text)` | 1 | Strip leading and trailing whitespace |
| `LEFT(text, count)` | 2 | First N characters |
| `RIGHT(text, count)` | 2 | Last N characters |
| `MID(text, start, count)` | 3 | Substring from 1-based position |
| `FIND(needle, haystack)` | 2 | 1-based position of first occurrence, 0 if not found |
| `SUBSTITUTE(text, old, new [, n])` | 3-4 | Replace occurrences. Optional 4th arg replaces only the Nth occurrence. |

### Math Functions

| Function | Args | Description |
|---|---|---|
| `ROUND(number, decimals)` | 2 | Round to N decimal places |
| `FLOOR(number)` | 1 | Round down to nearest integer |
| `CEIL(number)` | 1 | Round up to nearest integer |
| `ABS(number)` | 1 | Absolute value |
| `MIN(a, b, ...)` | 1+ | Minimum of all numeric arguments |
| `MAX(a, b, ...)` | 1+ | Maximum of all numeric arguments |
| `SUM(a, b, ...)` | 1+ | Sum of all numeric arguments (non-numeric skipped) |
| `AVERAGE(a, b, ...)` | 1+ | Average of numeric arguments. Returns `#DIV/0` if none. |
| `COUNT(a, b, ...)` | 1+ | Count of numeric values only |
| `COUNTA(a, b, ...)` | 1+ | Count of non-null values |

### Date Functions

| Function | Args | Description |
|---|---|---|
| `NOW()` | 0 | Current UTC datetime in RFC 3339 format |
| `TODAY()` | 0 | Current UTC date as YYYY-MM-DD |
| `YEAR(date)` | 1 | Extract year from a date string |
| `MONTH(date)` | 1 | Extract month (1-12) from a date string |
| `DAY(date)` | 1 | Extract day (1-31) from a date string |
| `DATEADD(date, amount, unit)` | 3 | Add time to a date. Units: `days`/`d`, `weeks`/`w`, `months`/`m`, `years`/`y` |
| `DATEDIFF(date1, date2, unit)` | 3 | Difference between two dates. Returns fractional values for weeks/months/years. |

Date functions accept ISO 8601 date strings (`YYYY-MM-DD`) and datetime strings (extracting the date part from the first 10 characters).

### Error Handling Functions

| Function | Args | Description |
|---|---|---|
| `BLANK()` | 0 | Returns null |
| `ERROR(message)` | 1 | Returns `#ERROR: {message}` |
| `ISERROR(value)` | 1 | Returns TRUE if the value is an error string (starts with `#`) |

### Record Functions

| Function | Args | Description |
|---|---|---|
| `RECORD_ID()` | 0 | Returns the current record's entity UUID, or null if unavailable |

## Field References

Field references use curly braces: `{Field Name}`. The name inside the braces is matched against the `field_values` map in the record context. If a referenced field does not exist, the value is `null`.

Cross-table field references use dot notation: `{Table.Field}`. These are used by the dependency graph for tracking formula dependencies across tables.

### Examples

```
{Name}                    -- simple field reference
{Due Date}                -- field name with spaces
{Items.Price}             -- cross-table reference
```

## Dependency Graph

The formula engine maintains a directed acyclic graph (DAG) tracking which formula fields depend on which raw fields. When a raw field changes, the graph determines which formulas need recalculation and in what order.

### How It Works

1. When a formula field is registered, its expression is parsed and all field references are extracted.
2. Forward edges are created: for each dependency D of formula F, `D -> F` means "when D changes, recalculate F."
3. When fields change, the graph performs a BFS to find all transitively affected formula fields.
4. Kahn's algorithm produces a topological sort of the affected subset, ensuring dependencies are recalculated before their dependents.

### Example

Given:
- `Subtotal = {Price} * {Qty}`
- `Tax = {Subtotal} * 0.1`
- `Total = {Subtotal} + {Tax}`

When `Price` changes, the calculation order is: `Subtotal` -> `Tax` -> `Total`.

### Cycle Detection

The graph uses DFS-based cycle detection with path tracking. If a circular dependency is found (e.g. A depends on B, B depends on A), `detect_cycles()` returns the cycle path.

```
A -> B -> C -> A  (cycle detected)
```

Cycles are detected at formula registration time. The engine returns the field names forming the cycle so the user can correct the formula definitions.

### Diamond Dependencies

Diamond patterns are handled correctly:

```
      X
     / \
    A   B
     \ /
      C
```

When X changes, A and B are both recalculated before C. The topological sort ensures C is only computed once, after both A and B have their new values.

## Batch Recalculation

The `recalculate_affected` function handles the full recalculation pipeline for a single entity:

1. Determine recalculation order from the dependency graph.
2. Fetch current field values for the entity from the triple store.
3. Evaluate each affected formula in dependency order, accumulating results into the context so later formulas see updated values.
4. Write all updated values back in a single SQL transaction.

The function returns `RecalcMetrics` with timing and count information:

```json
{
  "duration_ms": 12,
  "fields_recalculated": 3,
  "updates_executed": 3
}
```

## Common Formula Examples

**Conditional status label:**
```
IF(AND({Status} = "Done", {Priority} > 3), "High", "Low")
```

**Full name concatenation:**
```
{First} & " " & {Last}
```

**Concatenation with function:**
```
CONCAT(UPPER({First}), " ", LOWER({Last}))
```

**Days until due:**
```
DATEDIFF(TODAY(), {Due Date}, "days")
```

**Safe division with error handling:**
```
IF(ISERROR({Total} / {Count}), 0, {Total} / {Count})
```

**Pattern matching:**
```
SWITCH({Status}, "A", 1, "B", 2, "C", 3, 0)
```

**Percentage calculation:**
```
ROUND({Completed} / {Total} * 100, 1)
```

**Date arithmetic:**
```
DATEADD({Start Date}, 14, "days")
```
