#![no_main]
#![forbid(unsafe_code)]
#[path = "../../src/nrtm3.rs"]
mod nrtm3;
#[path = "../../src/rpsl.rs"]
mod rpsl;
use libfuzzer_sys::fuzz_target;
fuzz_target!(|bytes: &[u8]| {
    for filtered in [false, true] {
        let parsed = if filtered {
            nrtm3::Reader::filtered(bytes, "TEST", 1024 * 1024)
        } else {
            nrtm3::Reader::new(bytes, "TEST", 1024 * 1024)
        };
        if let Ok(reader) = parsed {
            let (first, last) = reader.range();
            let mut previous = None;
            for operation in reader {
                let Ok(operation) = operation else { break };
                assert!(operation.serial >= first && operation.serial <= last);
                assert!(previous.is_none_or(|serial| operation.serial > serial));
                assert_eq!(operation.object.source, "TEST");
                previous = Some(operation.serial);
            }
        }
    }
});
