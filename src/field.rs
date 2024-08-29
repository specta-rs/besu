use crate::Model;

// TODO: Does this need the model generic???
pub trait Field<M: Model> {
    type Type;
}
