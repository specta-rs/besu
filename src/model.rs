use crate::FromValue;

// TODO: Break out
pub trait Model: 'static {
    const NAME: &'static str;

    type Model: FromValue;
}
