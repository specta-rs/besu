// TODO: Using aliases. Eg. `pub struct TableName(&'static str)`, etc

pub struct TableDef {
    pub(crate) name: &'static str,
    pub(crate) columns: &'static [&'static str],
    pub(crate) column_types: &'static [&'static str],
}

pub trait Table {
    type Model;

    fn def() -> TableDef;
}
