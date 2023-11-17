pub struct Table {
    pub(crate) name: &'static str,
    pub(crate) fields: &'static [(&'static str, &'static str)],
}
