use super::*;

#[test]
fn roles_contexts_and_persisted_signature_reuse() {
	let domains = [
		Domain::Transfer,
		Domain::Proposal,
		Domain::Prevote,
		Domain::Precommit,
		Domain::ValidatorProof,
		Domain::PeerAuth,
	];
	for role in [KeyRole::Transaction, KeyRole::Validator, KeyRole::Network] {
		let key = SecretKey::generate(role).unwrap();
		let restored = SecretKey::import(&key.export(), role).unwrap();
		assert_eq!(key.public_key(), restored.public_key());
		for domain in domains {
			if domain.role() != role {
				assert!(matches!(
					key.sign(domain, b"chain-bound canonical bytes"),
					Err(Error::Role)
				));
				continue;
			}
			let signature = key.sign(domain, b"chain-bound canonical bytes").unwrap();
			let saved = Signature::from_bytes(SUITE, signature.as_bytes()).unwrap();
			assert!(saved == signature);
			let public = PublicKey::from_bytes(SUITE, role, &key.public_key().to_bytes()).unwrap();
			public.verify(domain, b"chain-bound canonical bytes", &saved).unwrap();
			assert!(public.verify(domain, b"different chain or message", &saved).is_err());
			for other in domains {
				if domain != other {
					assert!(public.verify(other, b"chain-bound canonical bytes", &saved).is_err());
				}
			}
		}
	}
}

#[test]
fn entropy_errors_abort_keygen_and_signing_without_fallback() {
	fn fail(bytes: &mut [u8]) -> Result<()> {
		bytes[0] = 42; // Even partially filled entropy is dropped through a wiping buffer.
		Err(Error::Config("test entropy failure"))
	}
	assert!(SecretKey::generate_with(KeyRole::Validator, fail).is_err());
	let key = SecretKey::generate(KeyRole::Validator).unwrap();
	assert!(key.sign_with(Domain::Prevote, b"message", fail).is_err());
	let a = key
		.sign_with(Domain::Prevote, b"message", |b| {
			b.fill(1);
			Ok(())
		})
		.unwrap();
	let b = key
		.sign_with(Domain::Prevote, b"message", |b| {
			b.fill(2);
			Ok(())
		})
		.unwrap();
	assert!(a != b);
	key.public_key().verify(Domain::Prevote, b"message", &a).unwrap();
	key.public_key().verify(Domain::Prevote, b"message", &b).unwrap();
}

#[test]
fn malformed_keys_signatures_and_profiles_are_rejected() {
	let key = SecretKey::generate(KeyRole::Transaction).unwrap();
	let bytes = key.export();
	assert!(SecretKey::import(&bytes, KeyRole::Network).is_err());
	for index in [0, 8, 10, 12, HEADER_BYTES, SECRET_FILE_BYTES - 1] {
		let mut bad = Zeroizing::new(bytes.to_vec());
		bad[index] ^= 0xff;
		assert!(SecretKey::import(&bad, KeyRole::Transaction).is_err());
	}
	assert!(SecretKey::import(&bytes[..bytes.len() - 1], KeyRole::Transaction).is_err());
	let mut extended = Zeroizing::new(bytes.to_vec());
	extended.push(0);
	assert!(SecretKey::import(&extended, KeyRole::Transaction).is_err());
	let public = key.public_key();
	assert!(PublicKey::from_bytes(65, KeyRole::Transaction, &public.to_bytes()).is_err());
	assert!(PublicKey::from_bytes(SUITE, KeyRole::Transaction, &[0; PUBLIC_KEY_BYTES]).is_err());
	assert!(PublicKey::from_bytes(SUITE, KeyRole::Transaction, &public.to_bytes()[..32]).is_err());
	let signature = key.sign(Domain::Transfer, b"message").unwrap();
	assert!(Signature::from_bytes(65, signature.as_bytes()).is_err());
	assert!(Signature::from_bytes(SUITE, &signature.as_bytes()[..64]).is_err());
	let mut bad = *signature.as_bytes();
	bad[0] ^= 1;
	assert!(public
		.verify(Domain::Transfer, b"message", &Signature::from_bytes(SUITE, &bad).unwrap())
		.is_err());
	// Out-of-range cumulative hint count.
	bad = *signature.as_bytes();
	bad[SIGNATURE_BYTES - 1] = 76;
	assert!(Signature::from_bytes(SUITE, &bad).is_err());
	// A nonzero byte in unused hint padding is a noncanonical encoding.
	bad = *signature.as_bytes();
	bad[SIGNATURE_BYTES - 83..].fill(0);
	bad[SIGNATURE_BYTES - 9] = 1;
	assert!(Signature::from_bytes(SUITE, &bad).is_err());
	// Two identical indices within the first polynomial.
	bad[SIGNATURE_BYTES - 83..].fill(0);
	bad[SIGNATURE_BYTES - 8..].fill(2);
	assert!(Signature::from_bytes(SUITE, &bad).is_err());
}

#[cfg(unix)]
#[test]
fn key_files_are_private_bounded_and_never_overwritten() {
	use std::{
		fs,
		os::unix::fs::{symlink, PermissionsExt},
	};
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("validator.key");
	let key = SecretKey::generate(KeyRole::Validator).unwrap();
	files::save_new(&path, &key).unwrap();
	assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
	assert_eq!(files::load(&path, KeyRole::Validator).unwrap().public_key(), key.public_key());
	assert!(files::save_new(&path, &key).is_err());
	assert!(files::load(&path, KeyRole::Network).is_err());
	let link = dir.path().join("link");
	symlink(&path, &link).unwrap();
	assert!(files::load(&link, KeyRole::Validator).is_err());
	fs::remove_file(&link).unwrap();
	fs::hard_link(&path, &link).unwrap();
	assert!(files::load(&path, KeyRole::Validator).is_err());
	fs::remove_file(&link).unwrap();
	fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
	assert!(files::load(&path, KeyRole::Validator).is_err());
	fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
	fs::write(&path, b"short").unwrap();
	assert!(files::load(&path, KeyRole::Validator).is_err());
	fs::write(&path, vec![0; SECRET_FILE_BYTES + 1]).unwrap();
	assert!(files::load(&path, KeyRole::Validator).is_err());
	assert!(files::load(dir.path(), KeyRole::Validator).is_err());
}

#[test]
fn independent_nist_external_interface_known_answers() {
	use serde_json::Value;
	fn bytes(t: &Value, field: &str) -> Vec<u8> {
		hex::decode(t[field].as_str().unwrap()).unwrap()
	}
	let data: Value =
		serde_json::from_str(include_str!("../../../tests/fixtures/pq/nist-ml-dsa-87.json"))
			.unwrap();
	let mut counts = [0; 3];
	for group in data["keyGen"].as_array().unwrap() {
		for t in group["tests"].as_array().unwrap() {
			let wrapped = SecretKey::generate_with(KeyRole::Transaction, |seed| {
				seed.copy_from_slice(&bytes(t, "seed"));
				Ok(())
			})
			.unwrap();
			let key = &wrapped.key;
			assert_eq!(
				key.public().to_bytes().as_slice(),
				bytes(t, "pk"),
				"keygen pk {}",
				t["tcId"]
			);
			assert!(
				key.secret().to_bytes().as_slice() == bytes(t, "sk"),
				"keygen sk {}",
				t["tcId"]
			);
			counts[0] += 1;
		}
	}
	for group in data["sigGen"].as_array().unwrap() {
		for t in group["tests"].as_array().unwrap() {
			let key = ml::SecretKey::from_bytes(&bytes(t, "sk")).unwrap();
			let mut hedge = SensitiveBytes32::zeroed();
			let randomness = if group["deterministic"] == false {
				hedge.as_mut_bytes().copy_from_slice(&bytes(t, "rnd"));
				Some(&hedge)
			} else {
				None
			};
			let sig =
				key.sign(&bytes(t, "message"), Some(&bytes(t, "context")), randomness).unwrap();
			assert_eq!(sig.as_slice(), bytes(t, "signature"), "siggen {}", t["tcId"]);
			Signature::from_bytes(SUITE, &sig).unwrap();
			counts[1] += 1;
		}
	}
	for group in data["sigVer"].as_array().unwrap() {
		for t in group["tests"].as_array().unwrap() {
			let accepted = ml::PublicKey::from_bytes(&bytes(t, "pk")).is_ok_and(|key| {
				key.verify(&bytes(t, "message"), &bytes(t, "signature"), Some(&bytes(t, "context")))
			});
			assert_eq!(accepted, t["testPassed"].as_bool().unwrap(), "sigver {}", t["tcId"]);
			counts[2] += 1;
		}
	}
	assert_eq!(counts, [3, 6, 15]);
}
