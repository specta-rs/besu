use crate::Table;

// TODO: Does this need the model generic???
/// The definition of a database column.
pub trait Column<M: Table> {
    /// The name of the column. This is used verbatim in the SQL queries.
    const NAME: &'static str;

    /// The underlying type of the column.
    type Type;
}
