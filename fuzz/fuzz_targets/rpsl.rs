#![no_main]
#![forbid(unsafe_code)]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/rpsl.rs"]
mod rpsl;

fuzz_target!(|bytes: &[u8]| {
    let Ok(input) = std::str::from_utf8(bytes) else {
        return;
    };
    let Ok(object) = rpsl::parse(input) else {
        return;
    };
    assert_eq!(object.text, input);
    assert!(object.source.bytes().all(|byte| !byte.is_ascii_lowercase()));
    assert!(object.key.bytes().all(|byte| !byte.is_ascii_lowercase()));

    let public = object.public_text();
    let reparsed = rpsl::parse(&public).expect("public text must preserve a valid identity");
    assert_eq!(reparsed.source, object.source);
    assert_eq!(reparsed.class, object.class);
    assert_eq!(reparsed.key, object.key);
    assert_eq!(reparsed.public_text(), public);
});
