// TODO: This macro should ensure the correct number of args are supplied like `format!`
#[macro_export]
macro_rules! sql {
    ($query:literal $(, $arg:expr )* $(,)?) => {
        (
            {
                let query: &'static str = $query;
                query
            },
            ($($arg,)*),
        )
    };
}
