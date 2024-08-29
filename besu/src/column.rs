use crate::Table;

// TODO: Does this need the model generic???
pub trait Column<M: Table> {
    const NAME: &'static str;

    type Type;
}
