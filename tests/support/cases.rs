// Each row is independently selectable with cargo test.
macro_rules! cases {
    ($check:ident; $($name:ident($($arg:expr),*)),+ $(,)?) => {$(
        #[test]
        fn $name() {
            $check($($arg),*);
        }
    )+};
}
