use crate::Model;

pub trait Table: 'static {
    const NAME: &'static str;

    type Model: Model;
}
