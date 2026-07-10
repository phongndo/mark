#![allow(dead_code)]

//! Stress fixture with non-ASCII text: café λ🚀.

/* outer block comment starts
   /* nested block comment exercises recursive begin/end rules */
   still inside the outer comment
*/

pub fn describe<'a>(input: &'a str) -> Result<String, &'static str> {
    let raw = r###"raw string with "quotes", hashes ##, and a newline
second raw line with emoji 🚀 and lambda λ"###;
    let escaped = "line one\nline two with café and an escaped quote: \"";
    let formatted = format!("{input:?} => {raw}\n{escaped}");
    Ok(formatted)
}

macro_rules! nested_tokens {
    ($name:ident, $value:expr) => {
        pub const $name: &str = stringify!($value);
    };
}

nested_tokens!(UNICODE_NAME, café_λ);
