use pq_signatures::{ml_dsa_65, ml_dsa_87, SensitiveBytes32};
use rand::RngCore;
fn main() {
	macro_rules! signatures {
		($scheme:ident) => {{
			let mut entropy = SensitiveBytes32::zeroed();
			rand::rngs::OsRng.try_fill_bytes(entropy.as_mut_bytes()).unwrap();
			let key = $scheme::Keypair::generate(&mut entropy);
			let msg = b"RINPQC/M2/profile-probe";
			let context = b"RINPQC/M2/TRANSFER";
			let a = key.sign(msg, Some(context), None).unwrap();
			let b = key.sign(msg, Some(context), None).unwrap();
			assert_eq!(a, b);
			assert!(key.public().verify(msg, &a, Some(context)));
			assert!(!key.public().verify(msg, &a, Some(b"RINPQC/M2/VOTE")));
			assert!(!key.public().verify(b"wrong-chain", &a, Some(context)));
			assert!(!key.public().verify(msg, &a[..a.len() - 1], Some(context)));
			let mut bad = a;
			bad[0] ^= 1;
			assert!(!key.public().verify(msg, &bad, Some(context)));
			let mut hedge = SensitiveBytes32::zeroed();
			rand::rngs::OsRng.try_fill_bytes(hedge.as_mut_bytes()).unwrap();
			let hedged = key.sign(msg, Some(context), Some(&hedge)).unwrap();
			assert!(key.public().verify(msg, &hedged, Some(context)));
			println!(
				"PASS {} pk={} signature={} deterministic/hedged, context, tamper, truncation",
				stringify!($scheme),
				$scheme::PUBLICKEYBYTES,
				$scheme::SIGNBYTES
			);
		}};
	}
	signatures!(ml_dsa_65);
	signatures!(ml_dsa_87);
	use clatter::{
		crypto::{cipher::ChaChaPoly, hash::Sha512, kem::rust_crypto_ml_kem::MlKem768},
		handshakepattern::noise_pqxx,
		traits::{Handshaker, Kem},
		PqHandshake,
	};
	let mut rng_a = rand::rngs::OsRng;
	let mut rng_b = rand::rngs::OsRng;
	let key_a = MlKem768::genkey(&mut rng_a).unwrap();
	let key_b = MlKem768::genkey(&mut rng_b).unwrap();
	let prologue = b"RINPQC/M2/transport-probe";
	let mut a = PqHandshake::<MlKem768, MlKem768, ChaChaPoly, Sha512, _>::new(
		noise_pqxx(),
		prologue,
		true,
		Some(key_a),
		None,
		None,
		None,
		&mut rng_a,
	)
	.unwrap();
	let mut b = PqHandshake::<MlKem768, MlKem768, ChaChaPoly, Sha512, _>::new(
		noise_pqxx(),
		prologue,
		false,
		Some(key_b),
		None,
		None,
		None,
		&mut rng_b,
	)
	.unwrap();
	let mut wire = [0; 16384];
	let mut payload = [0; 16384];
	let mut steps = 0;
	while !a.is_finished() || !b.is_finished() {
		assert!(steps < 8);
		if steps % 2 == 0 {
			let n = a.write_message(b"probe", &mut wire).unwrap();
			assert_eq!(b.read_message(&wire[..n], &mut payload).unwrap(), 5);
		} else {
			let n = b.write_message(b"probe", &mut wire).unwrap();
			assert_eq!(a.read_message(&wire[..n], &mut payload).unwrap(), 5);
		}
		steps += 1;
	}
	let mut a = a.finalize().unwrap();
	let mut b = b.finalize().unwrap();
	let n = a.send(b"payment", &mut wire).unwrap();
	let m = b.receive(&wire[..n], &mut payload).unwrap();
	assert_eq!(&payload[..m], b"payment");
	let n = a.send(b"tamper", &mut wire).unwrap();
	wire[0] ^= 1;
	assert!(b.receive(&wire[..n], &mut payload).is_err());
	println!("PASS PQXX ML-KEM-768/ChaChaPoly/SHA-512 handshake ({steps} messages), record roundtrip and tamper rejection; peer-identity binding not tested");
}
