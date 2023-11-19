pub struct TableDef {
    pub(crate) name: &'static str,
    pub(crate) fields: &'static [(&'static str, &'static str)],
}

pub trait Table {
    type Model;

    fn def() -> TableDef;
}
