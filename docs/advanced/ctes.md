# Common table expressions

A common table expression names a query so the rest of the statement can read it as if it
were a table. `Select::cte(name)` produces the handle for a whole-row query, and
`Select::cte_named(name, columns)` — or `SetQuery::cte_named` for a compound — produces one
for any projection. The statement that uses it renders the `WITH` clause itself.

```rust
let adults = db.select(users).filter(users.age.gte(18)).cte("adults");

let rows: Vec<User> = db
    .from(&adults)
    .filter(adults.column(users.age).lt(65))
    .all()
    .await?;
```

```sql
WITH "adults" AS (
    SELECT "users"."id", ... FROM "users" WHERE ("users"."age" >= $1)
)
SELECT "adults"."id", ... FROM "adults" WHERE ("adults"."age" < $2)
```

The expression is defined exactly where it is used, so there is no second statement and no
name to keep in sync. Its parameters come first, in the order the clause renders, and the
statement's own follow — one statement, one round trip.

## Columns

A table handle gets one field per column from `#[derive(Table)]`, because a table's columns
are known when the program is compiled. An expression's name is chosen at the call site, so
there is nothing for the derive to expand. `cte.column(..)` supplies the missing half: it
takes the column handle of the table the expression selects from and returns the same column
qualified by the expression's name.

```rust
adults.column(users.age)     // "adults"."age", typed as the same i32
adults.column(users.bio)     // "adults"."bio", still Option<String>
```

The Rust type, the SQL type and the nullability are the ones the schema declares, and the
expression must be in the query's row set for the call to compile — using a handle the
statement never defined or joined is a compile error.

Two expressions over the same table are the same Rust type, so the compiler cannot tell them
apart. The check is "this kind of expression is in scope", not "this exact one"; a mistake
there is a database error rather than a compile error. For the same reason a statement can
reference two expressions over two different tables but not two over the same one — the row
set would not say which is meant, exactly as it cannot for two joins of one table.

## Projected expressions

A projection that is not a whole table — an aggregate, a join, a subset of columns — has
columns the schema does not name. `cte_named` takes the output names and a type witness for
each, and the handle reads them back by name:

```rust
let totals = db
    .select(posts)
    .pick((posts.author_id, count_all()))
    .cte_named(
        "totals",
        (("author_id", posts.author_id), ("total", Int8)),
    );

let author = totals.column_named("author_id", posts.author_id);
let total = totals.column_named("total", Int8);

let rows: Vec<(i64, i64)> = db
    .from(&totals)
    .pick((author.clone(), total.clone()))
    .filter(total.gt(0_i64))
    .all()
    .await?;
```

```sql
WITH "totals" ("author_id", "total") AS (
    SELECT "posts"."author_id", COUNT(*) FROM "posts"
)
SELECT "totals"."author_id", "totals"."total"
FROM "totals" WHERE ("totals"."total" > $1)
```

A witness is either the type marker itself (`Int8`) or a column of the intended type
(`posts.author_id`), exactly as [`scalar`](../advanced/subqueries.md) accepts. The pairs can be
a tuple — which is how differently typed columns fit in one list — or a homogeneous array or
`Vec`.

The columns are written into the definition as an explicit `("author_id", "total")` list,
because `COUNT(*)` has a different name on every dialect. That is also why the names are
checked: they must be plain identifiers, and there must be one for each projected column. A
mismatch is recorded on the statement that uses the handle, the same way a dialect capability
is, rather than sent to a server.

A compound query is a valid `WITH` body and gets the same constructor. Its output columns take
their names from the first branch:

```rust
let everyone = db
    .select(users)
    .pick(users.id)
    .union(db.select(posts).pick(posts.author_id))
    .cte_named("everyone", (("id", users.id),));

let ids: Vec<i64> = db
    .from(&everyone)
    .pick(everyone.column_named("id", users.id))
    .all()
    .await?;
```

The handle's row type is the projection's Rust type, so `Cte<User>` for a whole-row query and
`Cte<i64>` or `Cte<(i64, i64)>` for a projected one.

## Joining one

```rust
let recent = db
    .select(users)
    .filter(users.age.gte(18))
    .cte("recent");

let rows: Vec<(String, i64)> = db
    .select(posts)
    .inner_join_cte(&recent, recent.column(users.id).eq(posts.author_id))
    .pick((posts.title, count_all()))
    .group_by(posts.title)
    .await?;
```

`inner_join_cte` and `left_join_cte` are the join forms; a left join widens the row type to
`Option<T>` exactly as it does for a table. Joining attaches the definition to the statement,
so the `WITH` clause appears without a separate call. A projected expression joins the same
way, with `column_named` for its columns.

## Using one in a subquery

A statement can define an expression it only mentions inside a subquery with `with_cte`:

```rust
let adults = db.select(users).filter(users.age.gte(18)).cte("adults");

db.select(posts)
    .with_cte(&adults)
    .filter(exists(
        db.from(&adults)
            .correlate(posts)
            .filter(adults.column(users.id).eq(posts.author_id)),
    ))
    .all()
    .await?;
```

The method is `with_cte` rather than `with`, because `Select::with` already means eager
loading a relation.

## Recursive expressions

`WITH RECURSIVE` needs the handle to be readable inside its own definition. `Cte::union_all`
takes the recursive term and builds the definition out of the anchor and that term:

```rust
let tree = db
    .select(nodes)
    .filter(nodes.parent_id.is_null())
    .cte("tree");

let rows: Vec<Node> = tree
    .clone()
    .union_all(
        db.select(nodes)
            .inner_join_cte(&tree, nodes.parent_id.eq(tree.column(nodes.id)))
            .pick(nodes),
    )
    .order_by(nodes.id.asc())
    .all()
    .await?;
```

```sql
WITH RECURSIVE "tree" AS (
    SELECT "nodes"."id", "nodes"."parent_id", "nodes"."name"
      FROM "nodes" WHERE ("nodes"."parent_id" IS NULL)
    UNION ALL
    SELECT "nodes"."id", "nodes"."parent_id", "nodes"."name"
      FROM "nodes" INNER JOIN "tree" ON ("nodes"."parent_id" = "tree"."id")
)
SELECT "tree"."id", "tree"."parent_id", "tree"."name" FROM "tree" ORDER BY "tree"."id" ASC
```

The statement reads the name back out, so `all()` decodes `Node` rather than a union of the
two branches. `.pick(nodes)` on the recursive term is what keeps the branch's row type the
same as the anchor's: a join widens the projection to both sides, and a union requires every
branch to produce the same columns.

`clone()` is needed only because one handle is used twice in one statement; the handle is a
value, not a borrow.

A projected expression can recurse too. Its explicit column list becomes part of the
recursive `WITH` entry, so the recursive term can read the names it declared.

## What is checked

- `cte_named` requires one name per projected column, and the names must be plain
  identifiers. A mismatch or a bad name is recorded on the statement that uses the handle.
- Every branch of a recursive definition must select the same number of columns; the builder
  checks before sending.
- A reference to an expression that is not in the row set does not compile.
- `column_named` checks the Rust type against the witness. The name itself is a value, so the
  database checks that the definition produces it.

## Gotchas

- **The name is written into the SQL, not bound.** It is a `&'static str`, so it is a literal
  at the call site; never build one from user input.
- **A name is not prefixed.** `Database::prefixed` rewrites table names in built statements;
  an expression's name is not a table and is left alone, which is what makes it work under a
  prefix.
- **An expression's name is scoped to its statement.** Two statements that both use
  `"adults"` each define their own; there is nothing shared to get out of step.
- **Read a projected expression with `from`, not `select`.** `db.from(&cte)` attaches the
  definition; a common table expression is not a `TableRef`, because its `WITH` clause has to
  precede the statement.
- **The recursion limit is the database's.** SQLite, PostgreSQL and MySQL each have their own
  behaviour for a runaway recursive term; Besu adds no guard. MySQL stops at
  `cte_max_recursion_depth` (1000 by default).
