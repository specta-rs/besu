pub trait Table {
    fn def() -> TableDef;
}

pub struct TableDef {
    pub(crate) name: &'static str,
    pub(crate) fields: &'static [(&'static str, &'static str)],
}
