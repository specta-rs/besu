use crate::SqlValue;

use super::InsertDefinition;

pub trait InsertValue {
    fn def() -> InsertDefinition;

    fn values(self) -> Vec<SqlValue>;
}

// TODO: Either set `Clone` as a super bound or `InsertValue` or change `values` to take `&self`.

// Required for `Iterator` of `&[]` because it yields `&V` not `V` as the value can't be owned.
impl<V: InsertValue + Clone> InsertValue for &V {
    fn def() -> InsertDefinition {
        V::def()
    }

    fn values(self) -> Vec<SqlValue> {
        V::values(self.clone())
    }
}
