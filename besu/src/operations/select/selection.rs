use crate::Sql;

pub trait Selection {
    type T;

    fn arguments(self) -> Vec<Sql<()>>;

    // TODO: Cleanup this
    fn todo(&self) -> &'static str;

    fn from_sql() -> Self::T;
}
