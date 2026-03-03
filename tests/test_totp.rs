use bittseeder::web::api::verify_totp;
use totp_rs::{
    Algorithm,
    Secret,
    TOTP
};

const TEST_SECRET: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn make_totp(secret: &str) -> TOTP {
    let bytes = Secret::Encoded(secret.to_string()).to_bytes().unwrap();
    TOTP::new(
        Algorithm::SHA1, 6, 1, 30, bytes,
        Some("BittSeeder".to_string()), "admin".to_string(),
    )
    .unwrap()
}

#[test]
fn valid_code_passes() {
    let totp = make_totp(TEST_SECRET);
    let code = totp.generate_current().unwrap();
    assert!(verify_totp(TEST_SECRET, &code));
}

#[test]
fn wrong_code_fails() {
    let totp = make_totp(TEST_SECRET);
    let code = totp.generate_current().unwrap();
    let last: u32 = code.chars().last().unwrap().to_digit(10).unwrap();
    let bad_last = (last + 1) % 10;
    let bad_code = format!("{}{}", &code[..5], bad_last);
    assert!(!verify_totp(TEST_SECRET, &bad_code));
}

#[test]
fn empty_code_fails() {
    assert!(!verify_totp(TEST_SECRET, ""));
}

#[test]
fn non_numeric_code_fails() {
    assert!(!verify_totp(TEST_SECRET, "abcdef"));
}

#[test]
fn invalid_base32_secret_fails() {
    assert!(!verify_totp("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!", "123456"));
}

#[test]
fn empty_secret_fails() {
    assert!(!verify_totp("", "123456"));
}

#[test]
fn different_secret_fails() {
    let other = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
    let totp = make_totp(TEST_SECRET);
    let code = totp.generate_current().unwrap();
    assert!(!verify_totp(other, &code));
}

#[test]
fn generated_secret_roundtrips() {
    let secret = Secret::generate_secret().to_encoded();
    let secret_str = secret.to_string(); // base32 string
    let bytes = secret.to_bytes().unwrap();
    let totp = TOTP::new(
        Algorithm::SHA1, 6, 1, 30, bytes,
        Some("BittSeeder".to_string()), "admin".to_string(),
    )
    .unwrap();
    let code = totp.generate_current().unwrap();
    assert!(verify_totp(&secret_str, &code));
}