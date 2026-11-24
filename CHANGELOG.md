# Changelog

## Unreleased

### Breaking changes

- Seal `Contains`, `IntoCondition`, `Selection`, `TableRef`, `IntoQueryBody`,
  `IntoSubquery` and conflict targets so downstream code cannot bypass query proofs (#117).
- Right and full joins now widen the accumulated row set as well as the default row type;
  alias, CTE and derived-table column projections preserve schema and outer-join nullability.
  `min` and `max` now project as optional because empty aggregates return SQL `NULL` (#110).
- Ordering, grouping and conflict targets are checked against the query's row set, and
  `between` bounds must use the column's SQL type (#118).
- `BridgeValue::Json` now stores a validated `RawValue` fragment instead of a parsed
  `serde_json::Value`, preserving nested numeric precision, duplicate members and key order
  across request serialization (#121).
- `Vector::to_binary` is fallible and rejects dimensions above 32767. Both PostgreSQL
  encoders surface that error instead of silently truncating components (#122).
- The table derive rejects duplicate SQL column names at compile time (#156).

### Fixes

- Native PostgreSQL parameters now follow the type PostgreSQL infers: native values use
  canonical text for text casts, and decimals bound to floats are converted numerically
  instead of reinterpreting `NUMERIC` wire bytes (#160).

### Internal

- Share statement rendering, execution and row decoding across `Select`, `SetQuery` and
  `DynamicSelect`. `SetQuery` keeps its row-set parameter because it now proves compound
  ordering scope; noisy local build timings did not justify a public API break (#119).
