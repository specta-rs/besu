use crate::schema::Column;

pub struct OrderClause {
    pub(crate) table_name: &'static str,
    pub(crate) column_name: &'static str,
    // TODO: `Cow` this and make it less cringe
    pub(crate) suffix: Option<&'static str>,
}

pub fn asc<C: Column>(column: C) -> OrderClause {
    let c = C::def();
    OrderClause {
        table_name: c.table_name,
        column_name: c.name,
        suffix: Some("ASC"),
    }
}

pub fn desc<C: Column>(column: C) -> OrderClause {
    let c = C::def();
    OrderClause {
        table_name: c.table_name,
        column_name: c.name,
        suffix: Some("DESC"),
    }
}

// TODO: Allow user defined clauses
