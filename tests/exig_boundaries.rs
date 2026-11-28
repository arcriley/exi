//! Round trips for compiled grammars and limits on loading .exig.
#![cfg(feature = "exig")]

use exi::grammar::GrammarSet;
use exi::{ErrorKind, Limits};

include!("support/cases.rs");

fn stable(bytes: &[u8]) {
    let set = GrammarSet::from_exig(bytes).unwrap();
    assert!(set.is_schema_informed());
    let wire = set.to_exig().unwrap();
    let reloaded = GrammarSet::from_exig(&wire).unwrap();
    assert_eq!(reloaded.to_exig().unwrap(), wire);
    assert_eq!(reloaded.digest().unwrap(), set.digest().unwrap());
}
cases!(stable;
    complex(include_bytes!("../fixtures/input/complex.exig")),
    grammar_for_grammars(include_bytes!("../fixtures/input/exig-1.exig")),
    html_profile(include_bytes!("../schemas/xhtml5-1.exig")),
);

fn truncated(bytes: &[u8]) {
    for cut in [0, 1, 4, bytes.len() / 2, bytes.len() - 1] {
        let error = GrammarSet::from_exig(&bytes[..cut]).unwrap_err();
        assert_eq!(
            error.kind(),
            ErrorKind::Truncated,
            "cut {cut} of {}: {error}",
            bytes.len()
        );
    }
}
cases!(truncated;
    truncated_complex(include_bytes!("../fixtures/input/complex.exig")),
    truncated_xhtml(include_bytes!("../schemas/xhtml5-1.exig")),
);

#[test]
fn depth_limit_applies_to_exig_loading() {
    let bytes = include_bytes!("../fixtures/input/complex.exig");
    let error = GrammarSet::from_exig_with(bytes, Limits::new().max_depth(1))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Limit);
}
