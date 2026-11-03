//! Bounded BCRYPT-PW authentication for authoritative maintainer objects.

use anyhow::{Context, Result, bail, ensure};
use bcrypt::HashParts;

use crate::rpsl::{MAX_OBJECT_BYTES, Object};

pub const MAX_PASSWORDS: usize = 20;
const MAX_PASSWORD_BYTES: usize = 72;
const MAX_AUTH_FIELDS: usize = 16;
const MAX_COST: u32 = 16;
const MAX_ATTEMPTS: usize = 64;
const MAX_WORK: u32 = 1 << 17;

/// One budget for an entire submission, shared across its maintainer checks.
pub struct AuthBudget {
    attempts_left: usize,
    work_left: u32,
}

impl Default for AuthBudget {
    fn default() -> Self {
        Self {
            attempts_left: MAX_ATTEMPTS,
            work_left: MAX_WORK,
        }
    }
}

impl AuthBudget {
    fn charge(&mut self, cost: u32) -> Result<()> {
        let work = 1 << cost;
        ensure!(
            self.attempts_left > 0 && work <= self.work_left,
            "maintainer authentication work limit exceeded"
        );
        self.attempts_left -= 1;
        self.work_left -= work;
        Ok(())
    }
}

/// Validate authentication attributes before publishing a new maintainer version.
/// Other object classes do not carry maintainer password rules.
pub fn validate_new(object: &Object) -> Result<()> {
    if object.class != "mntner" {
        return Ok(());
    }
    let auth = auth_values(object)?;
    ensure!(
        !auth.is_empty(),
        "maintainer requires a BCRYPT-PW auth attribute"
    );
    for value in auth {
        let value =
            value.ok_or_else(|| anyhow::anyhow!("folded maintainer auth is unsupported"))?;
        bcrypt_hash(value)?;
    }
    Ok(())
}

/// Recognise the pinned upstream dummy-hash replacement profile. Every auth
/// field must be a dummy bcrypt field; mixed real/dummy credentials fail closed.
pub fn dummy_update(object: &Object) -> Result<bool> {
    if object.class != "mntner" {
        return Ok(false);
    }
    let values = auth_values(object)?;
    let is_dummy = |value: Option<&str>| {
        let mut fields = value.unwrap_or_default().split_ascii_whitespace();
        fields
            .next()
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("BCRYPT-PW"))
            && fields.next() == Some("DummyValue")
            && fields.next().is_none()
    };
    let any = values.iter().any(|value| is_dummy(*value));
    ensure!(
        !any || values.iter().all(|value| is_dummy(*value)),
        "mixed dummy and real maintainer credentials"
    );
    Ok(any)
}

/// Charge replacement hashing to the same work budget as password verification.
/// This is called only after the stored object's maintainers have authenticated.
pub fn authorize_dummy_update(passwords: &[String], budget: &mut AuthBudget) -> Result<()> {
    let password = passwords
        .first()
        .context("dummy maintainer credentials require a password")?;
    ensure!(
        passwords.iter().all(|candidate| candidate == password),
        "dummy maintainer credentials require exactly one distinct password"
    );
    valid_replacement_password(password)?;
    budget.charge(bcrypt::DEFAULT_COST)
}

fn valid_replacement_password(password: &str) -> Result<()> {
    ensure!(
        !password.is_empty()
            && password.len() <= MAX_PASSWORD_BYTES
            && !password.as_bytes().contains(&0),
        "invalid replacement password"
    );
    Ok(())
}

/// Materialise an already authorised dummy update before any object is stored.
/// Failed hashing or parsing aborts the enclosing registry transaction.
pub fn materialize_dummy_update(object: &Object, password: &str) -> Result<Object> {
    ensure!(
        dummy_update(object)?,
        "maintainer does not contain dummy credentials"
    );
    valid_replacement_password(password)?;
    let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|_| anyhow::anyhow!("maintainer replacement hashing failed"))?;
    let mut text = String::with_capacity(object.text.len() + hash.len());
    let mut inserted = false;
    for line in object.text.split_inclusive('\n') {
        if line
            .split_once(':')
            .is_some_and(|(name, _)| name.eq_ignore_ascii_case("auth"))
        {
            if !inserted {
                text.push_str(&format!("auth: BCRYPT-PW {hash}\n"));
                inserted = true;
            }
        } else {
            text.push_str(line);
        }
    }
    let result =
        crate::rpsl::parse(&text).map_err(|_| anyhow::anyhow!("invalid replacement maintainer"))?;
    ensure!(
        result.class == object.class && result.source == object.source && result.key == object.key,
        "replacement maintainer identity changed"
    );
    Ok(result)
}

/// Verify supplied candidates against the supported hashes of one stored
/// maintainer. A legacy or unusable hash contributes no authentication method.
pub fn authenticates(
    object: &Object,
    passwords: &[String],
    budget: &mut AuthBudget,
) -> Result<bool> {
    authenticates_existing(object, passwords, budget, false)
}

pub fn authenticates_existing(
    object: &Object,
    passwords: &[String],
    budget: &mut AuthBudget,
    legacy_md5: bool,
) -> Result<bool> {
    if object.class != "mntner" || passwords.is_empty() {
        return Ok(false);
    }
    let mut candidates = Vec::new();
    for password in passwords {
        ensure!(
            !password.is_empty()
                && password.len() <= MAX_PASSWORD_BYTES
                && !password.as_bytes().contains(&0),
            "invalid maintainer password length or encoding"
        );
        let candidate = password.as_str();
        if !candidates.contains(&candidate) {
            ensure!(
                candidates.len() < MAX_PASSWORDS,
                "too many distinct maintainer passwords"
            );
            candidates.push(candidate);
        }
    }

    let mut checked = Vec::new();
    for value in auth_values(object)?.into_iter().flatten() {
        let (hash, cost, md5) = match bcrypt_hash(value) {
            Ok((hash, cost)) => (hash, cost, false),
            Err(_) if legacy_md5 => {
                let mut parts = value.split_ascii_whitespace();
                let scheme = parts.next();
                let Some(hash) = parts.next().filter(|hash| {
                    scheme.is_some_and(|scheme| scheme.eq_ignore_ascii_case("MD5-PW"))
                        && parts.next().is_none()
                        && crate::legacy_md5::valid_hash(hash)
                }) else {
                    continue;
                };
                (hash, 4, true)
            }
            Err(_) => continue,
        };
        if checked.contains(&hash) {
            continue;
        }
        checked.push(hash);
        for password in &candidates {
            budget.charge(cost)?;
            // The length check above guarantees that bcrypt does not discard
            // password bytes. At 72 bytes, bcrypt omits only the implicit NUL,
            // which is necessary to verify hashes generated by existing tools.
            let passed = if md5 {
                crate::legacy_md5::verify(password, hash)
            } else {
                bcrypt::verify(password, hash)
                    .map_err(|_| anyhow::anyhow!("maintainer password verification failed"))?
            };
            if passed {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn auth_values(object: &Object) -> Result<Vec<Option<&str>>> {
    ensure!(
        object.text.len() <= MAX_OBJECT_BYTES,
        "maintainer object exceeds auth input limit"
    );
    let mut lines = object
        .text
        .lines()
        .filter(|line| !line.starts_with(['#', '%']))
        .peekable();
    let mut values = Vec::new();
    while let Some(line) = lines.next() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.eq_ignore_ascii_case("auth") {
            continue;
        }
        ensure!(
            values.len() < MAX_AUTH_FIELDS,
            "too many maintainer auth attributes"
        );
        let folded = lines
            .peek()
            .is_some_and(|line| line.starts_with([' ', '\t', '+']));
        values.push((!folded).then(|| value.split('#').next().unwrap_or_default()));
    }
    Ok(values)
}

fn bcrypt_hash(value: &str) -> Result<(&str, u32)> {
    let mut parts = value.split_ascii_whitespace();
    if !parts
        .next()
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("BCRYPT-PW"))
    {
        bail!("unsupported maintainer authentication scheme");
    }
    let Some(hash) = parts.next().filter(|_| parts.next().is_none()) else {
        bail!("invalid maintainer bcrypt hash");
    };
    ensure!(
        hash.starts_with("$2a$") || hash.starts_with("$2b$") || hash.starts_with("$2y$"),
        "unsupported maintainer bcrypt version"
    );
    ensure!(
        hash.as_bytes()
            .get(4..6)
            .is_some_and(|cost| cost.iter().all(u8::is_ascii_digit)),
        "invalid maintainer bcrypt hash"
    );
    let parsed: HashParts = hash
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid maintainer bcrypt hash"))?;
    let cost = parsed.get_cost();
    ensure!(
        (4..=MAX_COST).contains(&cost),
        "maintainer bcrypt cost must be between 4 and {MAX_COST}"
    );
    Ok((hash, cost))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(password: &str) -> Result<String> {
        Ok(bcrypt::hash_with_salt(password, 4, [7; 16])?.to_string())
    }

    fn maintainer(auth: &str) -> Result<Object> {
        crate::rpsl::parse(&format!("mntner: TEST-MNT\n{auth}\nsource: TEST\n"))
            .map_err(anyhow::Error::msg)
    }

    #[test]
    fn validates_and_verifies_supported_bcrypt_variants() -> Result<()> {
        let encoded = hash("correct secret")?;
        for version in ["$2a$", "$2b$", "$2y$"] {
            let auth = format!(
                "AUTH: bcrypt-pw {} # description",
                encoded.replacen("$2b$", version, 1)
            );
            let object = maintainer(&auth)?;
            validate_new(&object)?;
            let mut budget = AuthBudget::default();
            assert!(!authenticates(&object, &["wrong".into()], &mut budget)?);
            assert!(authenticates(
                &object,
                &["wrong".into(), "correct secret".into()],
                &mut budget
            )?);
        }
        Ok(())
    }

    #[test]
    fn rejects_unusable_new_hashes_and_skips_unusable_existing_auth() -> Result<()> {
        let encoded = hash("valid password")?;
        for invalid in [
            "auth: MD5-PW legacy".to_owned(),
            "auth: PGPKEY-ABCDEF01".to_owned(),
            "auth: BCRYPT-PW DummyValue".to_owned(),
            "auth: BCRYPT-PW $2b$04$malformed".to_owned(),
            format!("auth: BCRYPT-PW {}", encoded.replacen("$04$", "$31$", 1)),
            format!("auth: BCRYPT-PW {}", encoded.replacen("$2b$", "$2x$", 1)),
            format!("auth: BCRYPT-PW {encoded}\n# comment\n+ continuation"),
        ] {
            let object = maintainer(&invalid)?;
            assert!(validate_new(&object).is_err());
            let mut budget = AuthBudget::default();
            assert!(!authenticates(
                &object,
                &["valid password".into()],
                &mut budget
            )?);
            assert_eq!(budget.attempts_left, MAX_ATTEMPTS);
        }
        let mixed = maintainer(&format!("auth: MD5-PW legacy\nauth: BCRYPT-PW {encoded}"))?;
        assert!(validate_new(&mixed).is_err());
        assert!(authenticates(
            &mixed,
            &["valid password".into()],
            &mut AuthBudget::default(),
        )?);
        assert!(validate_new(&maintainer("remarks: no auth")?).is_err());
        Ok(())
    }

    #[test]
    fn checks_password_byte_length_and_nuls_before_bcrypt() -> Result<()> {
        let password = "é".repeat(36);
        let object = maintainer(&format!("auth: BCRYPT-PW {}", hash(&password)?))?;
        let mut budget = AuthBudget::default();
        assert!(authenticates(
            &object,
            std::slice::from_ref(&password),
            &mut budget
        )?);
        for invalid in [format!("{password}x"), "valid\0ignored".into()] {
            assert!(authenticates(&object, &[invalid], &mut budget).is_err());
        }
        assert_eq!(budget.attempts_left, MAX_ATTEMPTS - 1);
        Ok(())
    }

    #[test]
    fn deduplicates_candidates_and_charges_one_submission_budget() -> Result<()> {
        let encoded = hash("correct password")?;
        let object = maintainer(&format!(
            "auth: BCRYPT-PW {encoded}\nauth: BCRYPT-PW {encoded}"
        ))?;
        let mut budget = AuthBudget {
            attempts_left: 2,
            work_left: 32,
        };
        let repeated_wrong = vec!["wrong".into(); 30];
        assert!(!authenticates(&object, &repeated_wrong, &mut budget)?);
        assert_eq!(budget.attempts_left, 1);
        assert!(authenticates(
            &object,
            &["correct password".into()],
            &mut budget
        )?);
        assert!(authenticates(&object, &["correct password".into()], &mut budget).is_err());

        let passwords = (0..=MAX_PASSWORDS)
            .map(|index| index.to_string())
            .collect::<Vec<_>>();
        assert!(authenticates(&object, &passwords, &mut AuthBudget::default()).is_err());
        let mut budget = AuthBudget::default();
        budget.charge(MAX_COST)?;
        budget.charge(MAX_COST)?;
        assert!(budget.charge(4).is_err());
        assert_eq!(budget.attempts_left, MAX_ATTEMPTS - 2);
        Ok(())
    }
}

#[cfg(test)]
mod dummy_tests {
    use super::*;

    #[test]
    fn dummy_replacements_are_bounded_complete_and_materialised() -> Result<()> {
        let object = crate::rpsl::parse("mntner: TEST-MNT\nauth: BCRYPT-PW DummyValue\nAUTH: bcrypt-pw DummyValue # filtered\nmnt-by: TEST-MNT\nsource: TEST\n").map_err(anyhow::Error::msg)?;
        assert!(dummy_update(&object)?);
        assert!(authorize_dummy_update(&[], &mut AuthBudget::default()).is_err());
        assert!(
            authorize_dummy_update(&["one".into(), "two".into()], &mut AuthBudget::default())
                .is_err()
        );
        authorize_dummy_update(&["correct secret".into()], &mut AuthBudget::default())?;
        let result = materialize_dummy_update(&object, "correct secret")?;
        validate_new(&result)?;
        assert_eq!(result.text.matches("auth:").count(), 1);
        assert!(!result.text.contains("DummyValue") && !result.text.contains("correct secret"));
        assert!(authenticates(
            &result,
            &["correct secret".into()],
            &mut AuthBudget::default()
        )?);
        assert!(!authenticates(
            &result,
            &["wrong secret".into()],
            &mut AuthBudget::default()
        )?);
        let real = bcrypt::hash_with_salt("secret", 4, [14; 16])?;
        let mixed = crate::rpsl::parse(&format!("{}auth: BCRYPT-PW {real}\n", object.text))
            .map_err(anyhow::Error::msg)?;
        assert!(dummy_update(&mixed).is_err());
        assert!(materialize_dummy_update(&object, &"x".repeat(73)).is_err());
        let mut exhausted = AuthBudget {
            attempts_left: 1,
            work_left: 1,
        };
        assert!(authorize_dummy_update(&["secret".into()], &mut exhausted).is_err());
        Ok(())
    }
}
