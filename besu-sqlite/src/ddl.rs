use besu_core::{TypeDef, Value};
use besu_schema::{Column, Ddl, Table};

use crate::SQLite;

/// DDL generation for SQLite.
#[derive(Debug, Clone, Copy, Default)]
pub struct SQLiteDdl;

impl Ddl for SQLiteDdl {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn quote(&self, ident: &str) -> String {
        let mut out = String::with_capacity(ident.len() + 2);
        out.push('"');
        for ch in ident.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
        out
    }

    fn type_name(&self, ty: &TypeDef) -> String {
        use besu_core::Dialect;
        SQLite::type_name(ty.clone()).into_owned()
    }

    fn column_sql(&self, column: &Column, inline_pk: bool) -> String {
        let mut sql = format!("{} {}", self.quote(&column.name), self.type_name(&column.ty));

        // SQLite only accepts `AUTOINCREMENT` on `INTEGER PRIMARY KEY`, and it must be
        // written inline.
        if column.auto_increment && column.primary_key {
            sql.push_str(" PRIMARY KEY AUTOINCREMENT");
            return sql;
        }

        if inline_pk && column.primary_key {
            sql.push_str(" PRIMARY KEY");
        }

        if !column.nullable && !column.primary_key {
            sql.push_str(" NOT NULL");
        }

        if column.unique {
            sql.push_str(" UNIQUE");
        }

        if let Some(default) = &column.default {
            sql.push_str(" DEFAULT ");
            sql.push_str(default);
        }

        sql
    }

    fn can_alter_column(&self) -> bool {
        false
    }

    fn drop_index(&self, _table: &str, index: &besu_schema::Index) -> String {
        format!("DROP INDEX {}", self.quote(&index.name))
    }

    fn add_foreign_key(&self, _table: &str, _fk: &besu_schema::ForeignKey) -> String {
        // SQLite has no `ALTER TABLE ... ADD CONSTRAINT`.
        String::new()
    }

    fn drop_foreign_key(&self, _table: &str, _fk: &besu_schema::ForeignKey) -> String {
        String::new()
    }

    fn literal(&self, value: &Value) -> String {
        match value {
            Value::Null => "NULL".into(),
            Value::Bool(v) => if *v { "1" } else { "0" }.into(),
            other => Ddl::literal(self, other),
        }
    }

    fn create_table(&self, table: &Table) -> String {
        let mut sql = String::from("CREATE TABLE ");
        sql.push_str(&self.quote_table(&table.name));
        sql.push_str(" (");

        let primary_key = table.primary_key();
        let inline_pk = primary_key.len() == 1;

        for (i, column) in table.columns.iter().enumerate() {
            if i != 0 {
                sql.push_str(", ");
            }
            sql.push_str(&self.column_sql(column, inline_pk));
        }

        if primary_key.len() > 1 {
            sql.push_str(", PRIMARY KEY (");
            for (i, column) in primary_key.iter().enumerate() {
                if i != 0 {
                    sql.push_str(", ");
                }
                sql.push_str(&self.quote(&column.name));
            }
            sql.push(')');
        }

        for fk in &table.foreign_keys {
            sql.push_str(", ");
            sql.push_str(&self.foreign_key_clause(fk));
        }

        sql.push(')');
        sql
    }
}
