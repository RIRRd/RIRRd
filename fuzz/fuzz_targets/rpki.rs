#![no_main]
#[allow(dead_code)]
#[path = "../../src/rpki.rs"]
mod rpki;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65537 {
        return;
    }
    if let Ok(table) = rpki::RoaTable::read(data, 65537) {
        for (prefix, origin) in [
            ("192.0.2.0/24", 64496),
            ("2001:db8::/32", 64497),
            ("0.0.0.0/0", 0),
        ] {
            let verdict = table.validate(prefix, origin).unwrap();
            let (covering, _) = table.covering_payloads(prefix).unwrap();
            if verdict == rpki::Validation::Valid {
                assert!(!covering.is_empty());
            }
        }
    }
    if data.len() >= 10 {
        let length = data[0] % 33;
        let origin = u32::from_be_bytes(data[1..5].try_into().unwrap());
        let raw = u32::from_be_bytes(data[5..9].try_into().unwrap());
        let address = raw & u32::MAX.checked_shl(u32::from(32 - length)).unwrap_or(0);
        let ip = std::net::IpAddr::V4(address.into());
        if let Ok(table) = rpki::RoaTable::from_verified_prefixes([(ip, length, origin, data[9])]) {
            let verdict = table.validate(&format!("{ip}/{length}"), origin).unwrap();
            assert_eq!(
                verdict,
                if origin == 0 {
                    rpki::Validation::Invalid
                } else {
                    rpki::Validation::Valid
                }
            );
        }
    }
});
