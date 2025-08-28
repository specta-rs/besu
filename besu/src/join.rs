use crate::Sql;

pub trait Join {
    type Output;

    fn join(self) -> Sql<Self::Output>;
}

// TODO: impls

// TODO: `Into` impls

// impl<T1, T2, T3, T4, T5, T6, T7, T8, T9>
//     From<(
//         Sql<T1>,
//         Sql<T2>,
//         Sql<T3>,
//         Sql<T4>,
//         Sql<T5>,
//         Sql<T6>,
//         Sql<T7>,
//         Sql<T8>,
//         Sql<T9>,
//     )> for Sql<(T1, T2, T3, T4, T5, T6, T7, T8, T9)>
// {
//     fn from(
//         value: (
//             Sql<T1>,
//             Sql<T2>,
//             Sql<T3>,
//             Sql<T4>,
//             Sql<T5>,
//             Sql<T6>,
//             Sql<T7>,
//             Sql<T8>,
//             Sql<T9>,
//         ),
//     ) -> Self {
//         todo!();
//     }
// }
