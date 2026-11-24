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
